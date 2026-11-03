'use strict';
const vscode = require('vscode');
const fs = require('node:fs');
const path = require('node:path');
const { spawn } = require('node:child_process');
const { createInterface } = require('node:readline');

const isSource = document => document.languageId === 'rinzu' && document.uri.scheme === 'file';
const canonical = filename => { try { return fs.realpathSync(filename); } catch { return path.join(fs.realpathSync(path.dirname(filename)), path.basename(filename)); } };
const range = value => new vscode.Range(value.start.line, value.start.character, value.end.line, value.end.character);
const locations = values => values.filter(value => value.path && value.range).map(value => new vscode.Location(vscode.Uri.file(value.path), range(value.range)));
function modified(message = 'Source changed before this response could be used') {
  const error = new Error(message); error.code = 'ContentModified'; return error;
}
function hasBom(filename) {
  let fd;
  try { fd = fs.openSync(filename, 'r'); const bytes = Buffer.alloc(3); return fs.readSync(fd, bytes, 0, 3, 0) === 3 && bytes.equals(Buffer.from([239, 187, 191])); }
  catch { return false; }
  finally { if (fd !== undefined) fs.closeSync(fd); }
}

class SourceClient {
  constructor(output, diagnostics) {
    this.output = output; this.diagnostics = diagnostics;
    this.epoch = 0; this.nextId = 0; this.pending = new Map(); this.sent = new Map();
    this.syncTail = Promise.resolve(); this.diskDirty = false; this.disposed = false;
    const folder = vscode.workspace.workspaceFolders?.[0]?.uri.fsPath;
    const config = vscode.workspace.getConfiguration('rinzu');
    const configured = config.get('workspace', '');
    this.workspace = configured ? path.resolve(folder || process.cwd(), configured) : folder && fs.existsSync(path.join(folder, 'rinzu.toml')) ? folder : undefined;
    const entry = config.get('entry', '');
    this.entry = entry ? path.resolve(folder || process.cwd(), entry) : undefined;
    const args = ['editor', '--stdio']; if (this.workspace) args.push(this.workspace);
    this.child = spawn(config.get('executable', 'rinzu'), args, { cwd: folder, stdio: ['pipe', 'pipe', 'pipe'] });
    this.child.stderr.on('data', bytes => output.append(bytes.toString()));
    this.child.on('error', error => { this.exited = true; output.appendLine(error.message); this.rejectPending(error); });
    this.child.on('exit', (code, signal) => { this.exited = true; this.rejectPending(new Error(`Rinzu editor exited (${code ?? signal})`)); });
    this.child.stdin.on('error', error => this.rejectPending(error));
    this.lines = createInterface({ input: this.child.stdout });
    this.lines.on('line', line => {
      let response; try { response = JSON.parse(line); } catch (error) { this.rejectPending(error); return; }
      const pending = this.pending.get(response.id); if (!pending) return;
      this.pending.delete(response.id); clearTimeout(pending.timer);
      if (response.schema !== 'rinzu-editor/1') { pending.reject(new Error('Unsupported Rinzu editor schema')); return; }
      if (response.error) { const error = new Error(response.error.message); error.code = response.error.code; pending.reject(error); }
      else pending.resolve(response);
    });
    this.ready = this.entry ? this.raw('initialize', { entry: this.entry }) : Promise.resolve();
    this.ready.catch(error => output.appendLine(error.message));
  }
  rejectPending(error) { for (const pending of this.pending.values()) { clearTimeout(pending.timer); pending.reject(error); } this.pending.clear(); }
  raw(method, params = {}) {
    if (this.disposed || this.exited) return Promise.reject(new Error('Rinzu source service is closed'));
    const id = ++this.nextId;
    return new Promise((resolve, reject) => {
      const timer = setTimeout(() => { this.pending.delete(id); reject(new Error('Rinzu editor request timed out')); }, 60000);
      this.pending.set(id, { resolve, reject, timer });
      this.child.stdin.write(JSON.stringify({ id, method, params }) + '\n');
    });
  }
  invalidate(disk = false) {
    ++this.epoch; this.diskDirty ||= disk; this.diagnostics.clear();
  }
  documents() { return vscode.workspace.textDocuments.filter(isSource); }
  synchronize() {
    const work = async () => {
      await this.ready;
      for (let attempt = 0; attempt < 8; ++attempt) {
        const documents = this.documents();
        const desired = new Map(documents.map(document => [canonical(document.uri.fsPath), document]));
        for (const [filename, previous] of this.sent) {
          if (!desired.has(filename)) { await this.raw('close', { path: filename, version: previous.version }); this.sent.delete(filename); }
        }
        if (this.diskDirty) { this.diskDirty = false; await this.raw('refresh'); }
        for (const [filename, document] of desired) {
          const previous = this.sent.get(filename);
          if (previous?.version === document.version) continue;
          const version = document.version;
          const text = document.getText();
          await this.raw(previous ? 'change' : 'open', { path: filename, version, text, bom: previous?.bom ?? hasBom(filename) });
          this.sent.set(filename, { version, bom: previous?.bom ?? hasBom(filename) });
        }
        const current = this.documents();
        if (current.length === this.sent.size && current.every(document => this.sent.get(canonical(document.uri.fsPath))?.version === document.version) && !this.diskDirty) return;
      }
      throw modified('Buffers kept changing during synchronization');
    };
    const result = this.syncTail.then(work); this.syncTail = result.catch(() => {}); return result;
  }
  assertFresh(response) {
    if (response.clientEpoch !== this.epoch) throw modified();
    const current = new Map(this.documents().map(document => [canonical(document.uri.fsPath), document.version]));
    if (current.size !== response.documents.length || response.documents.some(document => current.get(document.path) !== document.version)) throw modified();
    if (response.result.stale) throw modified('Disk snapshot became stale');
  }
  async query(method, document, extra = {}, token) {
    await this.synchronize();
    if (token?.isCancellationRequested || document.isClosed) throw modified();
    const epoch = this.epoch;
    const response = await this.raw(method, { path: canonical(document.uri.fsPath), version: document.version, ...extra });
    response.clientEpoch = epoch; this.assertFresh(response);
    if (token?.isCancellationRequested) throw modified();
    if (method === 'format' || method === 'organizeImports') {
      // VS Code's text model preserves the document EOL when applying edits.
      // Return its actual candidate and avoid repeated edits differing only by EOL.
      const eol = document.eol === vscode.EndOfLine.CRLF ? '\r\n' : '\n';
      response.result.edits = response.result.edits.map(edit => ({ ...edit, newText: edit.newText.replace(/\r\n|\n/g, eol) }))
        .filter(edit => edit.newText !== document.getText());
    }
    return response;
  }
  async applyEdits(document, response) {
    this.assertFresh(response);
    if (!response.result.edits?.length) return true;
    const editor = await vscode.window.showTextDocument(document);
    this.assertFresh(response);
    if (response.result.edits.some(edit => edit.version !== document.version || edit.path !== canonical(document.uri.fsPath))) throw modified();
    return editor.edit(builder => { for (const edit of response.result.edits) builder.replace(range(edit.range), edit.newText); });
  }
  async check(document) {
    const response = await this.query('diagnostics', document);
    this.assertFresh(response);
    const byFile = new Map();
    for (const item of response.result.diagnostics) {
      const primary = item.primary; if (!primary.path) { this.output.appendLine(`${item.code}: ${item.message}`); continue; }
      const diagnostic = new vscode.Diagnostic(primary.range ? range(primary.range) : new vscode.Range(0, 0, 0, 0), item.message, vscode.DiagnosticSeverity.Error);
      diagnostic.code = item.code; diagnostic.source = 'Rinzu';
      diagnostic.relatedInformation = item.secondary.filter(info => info.location.path && info.location.range).map(info => new vscode.DiagnosticRelatedInformation(locations([info.location])[0], info.message));
      const group = byFile.get(primary.path) || []; group.push(diagnostic); byFile.set(primary.path, group);
    }
    this.assertFresh(response); this.diagnostics.clear();
    for (const [filename, items] of byFile) this.diagnostics.set(vscode.Uri.file(filename), items);
    if (response.result.omittedDiagnostics) this.output.appendLine(`${response.result.omittedDiagnostics} additional diagnostics omitted`);
    return response;
  }
  dispose() {
    if (this.disposed) return;
    this.disposed = true; this.child.stdin.end(); this.rejectPending(new Error('Rinzu editor disposed'));
    const child = this.child; setTimeout(() => { if (child.exitCode === null && child.signalCode === null) child.kill(); }, 2000).unref();
  }
}

let active;
function activate(context) {
  const output = vscode.window.createOutputChannel('Rinzu');
  const diagnostics = vscode.languages.createDiagnosticCollection('rinzu');
  const client = new SourceClient(output, diagnostics); active = client;
  const selector = { language: 'rinzu', scheme: 'file' };
  let timer;
  const report = error => { if (error.code !== 'ContentModified') output.appendLine(error.stack || error.message); };
  const schedule = () => {
    clearTimeout(timer); timer = setTimeout(() => {
      const document = vscode.window.activeTextEditor?.document;
      if (document && isSource(document)) client.check(document).catch(report);
    }, 180);
  };
  const changed = disk => { client.invalidate(disk); schedule(); };
  const safe = async (operation, fallback) => { try { return await operation(); } catch (error) { report(error); return fallback; } };
  context.subscriptions.push(output, diagnostics, client,
    { dispose() { clearTimeout(timer); } },
    vscode.workspace.onDidOpenTextDocument(document => { if (isSource(document)) changed(false); }),
    vscode.workspace.onDidChangeTextDocument(event => { if (isSource(event.document)) changed(false); }),
    vscode.workspace.onDidCloseTextDocument(document => { if (isSource(document)) changed(false); }),
    vscode.workspace.onDidSaveTextDocument(document => { if (isSource(document)) changed(true); }),
    vscode.workspace.onDidRenameFiles(() => changed(true)),
    vscode.window.onDidChangeActiveTextEditor(editor => { if (editor && isSource(editor.document)) schedule(); }),
    vscode.languages.registerDefinitionProvider(selector, { provideDefinition: (document, position, token) => safe(async () => locations((await client.query('definition', document, { position }, token)).result.locations), []) }),
    vscode.languages.registerReferenceProvider(selector, { provideReferences: (document, position, referenceContext, token) => safe(async () => locations((await client.query('references', document, { position, includeDeclaration: referenceContext.includeDeclaration }, token)).result.locations), []) }),
    vscode.languages.registerCompletionItemProvider(selector, { provideCompletionItems: (document, position, token) => safe(async () => {
      const response = await client.query('completion', document, { position }, token);
      const prefix = document.lineAt(position.line).text.slice(0, position.character).match(/[A-Za-z_][A-Za-z0-9_.]*$/)?.[0] || '';
      const kinds = { function: vscode.CompletionItemKind.Function, variable: vscode.CompletionItemKind.Variable, parameter: vscode.CompletionItemKind.Variable, class: vscode.CompletionItemKind.Class, enum: vscode.CompletionItemKind.Enum, type: vscode.CompletionItemKind.TypeParameter, typeParameter: vscode.CompletionItemKind.TypeParameter, constant: vscode.CompletionItemKind.Constant, import: vscode.CompletionItemKind.Module, keyword: vscode.CompletionItemKind.Keyword };
      return response.result.items.filter(item => item.label.startsWith(prefix)).map(item => {
        const completion = new vscode.CompletionItem(item.label, kinds[item.kind]);
        completion.range = new vscode.Range(position.line, position.character - prefix.length, position.line, position.character);
        completion.detail = response.result.syntaxOnly ? 'Rinzu syntax only (current code has errors)' : 'Rinzu'; return completion;
      });
    }, []) }, '.'),
    vscode.languages.registerDocumentFormattingEditProvider(selector, { provideDocumentFormattingEdits: (document, _options, token) => safe(async () => {
      const response = await client.query('format', document, {}, token); client.assertFresh(response);
      return response.result.edits.map(edit => vscode.TextEdit.replace(range(edit.range), edit.newText));
    }, []) }),
    vscode.commands.registerCommand('rinzu.refresh', async () => {
      client.invalidate(true); const document = vscode.window.activeTextEditor?.document;
      if (document && isSource(document)) return client.check(document);
    }),
    vscode.commands.registerCommand('rinzu.organizeImports', async () => {
      const document = vscode.window.activeTextEditor?.document;
      if (document && isSource(document)) return safe(async () => client.applyEdits(document, await client.query('organizeImports', document)), false);
    })
  );
  const watcher = vscode.workspace.createFileSystemWatcher('**/*.{rinzu,toml,lock}');
  context.subscriptions.push(watcher, watcher.onDidChange(() => changed(true)), watcher.onDidCreate(() => changed(true)), watcher.onDidDelete(() => changed(true)));
  schedule();
  return { client, diagnostics, range };
}
function deactivate() { active?.dispose(); active = undefined; }
module.exports = { activate, deactivate, SourceClient };
