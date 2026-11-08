'use strict';
const vscode = require('vscode');
const fs = require('node:fs');
const path = require('node:path');
const { spawn } = require('node:child_process');
const { createInterface } = require('node:readline');

const isSource = document => document.languageId === 'rinzu' && document.uri.scheme === 'file';
function canonical(filename) {
  try { return fs.realpathSync(filename); }
  catch {
    try { return path.join(fs.realpathSync(path.dirname(filename)), path.basename(filename)); }
    catch { return path.resolve(filename); }
  }
}
function within(root, filename) {
  const relative = path.relative(root, filename);
  return relative !== '..' && !relative.startsWith('..' + path.sep) && !path.isAbsolute(relative);
}
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

function diagnosticsByFile(items, output) {
  const byFile = new Map();
  for (const item of items) {
    const primary = item.primary;
    if (!primary.path) {
      output.appendLine(`${item.code}: ${item.message}`);
      continue;
    }
    const help = (item.suggestions || [])
      .filter(suggestion => suggestion.applicability === 'Explanation' && !suggestion.replacement)
      .map(suggestion => `\nHelp: ${suggestion.message}`).join('');
    const diagnostic = new vscode.Diagnostic(primary.range ? range(primary.range) : new vscode.Range(0, 0, 0, 0), item.message + help, vscode.DiagnosticSeverity.Error);
    diagnostic.code = item.code;
    diagnostic.source = 'Rinzu';
    diagnostic.relatedInformation = item.secondary.flatMap(info =>
      locations([info.location]).map(location => new vscode.DiagnosticRelatedInformation(location, info.message)));
    const key = JSON.stringify([item.code, primary.range || primary.originalBytes, item.message, item.secondary]);
    if (!byFile.has(primary.path)) byFile.set(primary.path, new Map());
    byFile.get(primary.path).set(key, diagnostic);
  }
  return byFile;
}

class SourceClient {
  constructor(output, diagnostics) {
    this.output = output; this.diagnostics = diagnostics;
    this.epoch = 0; this.nextId = 0; this.pending = new Map(); this.sent = new Map();
    this.syncTail = Promise.resolve(); this.diskDirty = false; this.disposed = false;
    this.scoped = undefined;
    this.scopes = new Map();
    this.scopeVersions = new Map();
    this.documentScopes = new Map();
    this.resolve = new Set();
    const folder = vscode.workspace.workspaceFolders?.[0]?.uri.fsPath;
    const config = vscode.workspace.getConfiguration('rinzu');
    const configured = config.get('workspace', '');
    this.workspace = configured ? path.resolve(folder || process.cwd(), configured) : undefined;
    const entry = config.get('entry', '');
    this.entry = entry ? path.resolve(folder || process.cwd(), entry) : undefined;
    const args = ['editor', '--stdio']; if (this.workspace) args.push(this.workspace);
    this.child = spawn(config.get('executable', 'rinzu'), args, { cwd: folder, stdio: ['pipe', 'pipe', 'pipe'] });
    this.spawned = new Promise((resolve, reject) => {
      this.child.once('spawn', resolve);
      this.child.once('error', reject);
    });
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
    this.ready = this.entry ? this.raw('initialize', { entry: this.entry }) : this.spawned;
    this.ready.catch(error => output.appendLine(error.message));
  }
  rejectPending(error) { for (const pending of this.pending.values()) { clearTimeout(pending.timer); pending.reject(error); } this.pending.clear(); }
  async raw(method, params = {}) {
    if (this.disposed || this.exited) throw new Error('Rinzu source service is closed');
    await this.spawned;
    if (this.disposed || this.exited) throw new Error('Rinzu source service is closed');
    const id = ++this.nextId;
    return new Promise((resolve, reject) => {
      const timer = setTimeout(() => { this.pending.delete(id); reject(new Error('Rinzu editor request timed out')); }, 60000);
      this.pending.set(id, { resolve, reject, timer });
      this.child.stdin.write(JSON.stringify({ id, method, params }) + '\n');
    });
  }
  invalidate(disk = false, filenames = [], structural = false) {
    ++this.epoch;
    this.diskDirty ||= disk;
    this.scopeVersions.clear();
    if (!this.scoped) {
      this.diagnostics.clear();
      return;
    }
    const changed = filenames.map(canonical);
    const manifests = structural ? changed.filter(filename => path.basename(filename) === 'rinzu.toml') : [];
    const open = new Set(this.documents().map(document => canonical(document.uri.fsPath)));
    const files = new Set();
    for (const [filename] of this.documentScopes) {
      if (!open.has(filename)) this.documentScopes.delete(filename);
      else if (manifests.some(manifest => within(path.dirname(manifest), filename))) this.resolve.add(filename);
    }
    for (const filename of this.resolve) if (!open.has(filename)) this.resolve.delete(filename);
    for (const scope of this.scopes.values()) {
      const affected = !changed.length || changed.some(filename =>
        scope.path === filename || scope.inputs.has(filename) ||
        (structural && (within(filename, scope.path) ||
          (scope.kind !== 'file' && (scope.unresolved || within(path.dirname(scope.path), filename))) ||
          [...scope.inputs].some(input => within(filename, input)))));
      if (affected && scope.byFile) {
        for (const file of scope.byFile.keys()) files.add(file);
        scope.byFile = null;
      }
    }
    this.dropUnusedScopes(files);
    this.publish(files);
  }
  documents() { return vscode.workspace.textDocuments.filter(document => isSource(document) && !document.isClosed); }
  dropUnusedScopes(files) {
    const used = new Set([...this.documentScopes.values()].flatMap(document => document.keys));
    for (const [key, scope] of this.scopes) {
      if (used.has(key)) continue;
      for (const file of scope.byFile?.keys() || []) files.add(file);
      this.scopes.delete(key);
    }
  }
  publish(files) {
    for (const filename of files) {
      const combined = new Map();
      for (const scope of this.scopes.values()) {
        for (const [key, diagnostic] of scope.byFile?.get(filename) || []) {
          if (!combined.has(key)) combined.set(key, diagnostic);
        }
      }
      const uri = vscode.Uri.file(filename);
      if (combined.size) this.diagnostics.set(uri, [...combined.values()]);
      else this.diagnostics.delete(uri);
    }
    this.onScopesChanged?.();
  }
  needsCheck(filename) {
    const document = this.documentScopes.get(filename);
    return !document || this.resolve.has(filename) || document.keys.some(key => !this.scopes.get(key)?.byFile);
  }
  async checkOpen(active, report) {
    if (this.disposed) return;
    const documents = this.documents();
    if (this.scoped === undefined && !documents.includes(active)) active = documents[0];
    if (active && isSource(active)) {
      const index = documents.indexOf(active);
      if (index > 0) documents.unshift(...documents.splice(index, 1));
    }
    for (const document of documents) {
      if (document.isClosed) continue;
      if (!this.scoped && document !== active) continue;
      if (this.scoped && !this.needsCheck(canonical(document.uri.fsPath))) continue;
      try { await this.check(document); }
      catch (error) { report(error); }
    }
  }
  synchronize() {
    const work = async () => {
      await this.ready;
      for (let attempt = 0; attempt < 8; ++attempt) {
        const documents = this.documents();
        const desired = new Map(documents.map(document => [canonical(document.uri.fsPath), document]));
        for (const [filename, previous] of this.sent) {
          // VS Code may restart the version counter when a file is reopened.
          if (desired.get(filename) !== previous.document) {
            await this.raw('close', { path: filename, version: previous.version });
            this.sent.delete(filename);
          }
        }
        if (this.diskDirty) { this.diskDirty = false; await this.raw('refresh'); }
        for (const [filename, document] of desired) {
          const previous = this.sent.get(filename);
          if (previous?.version === document.version) continue;
          const version = document.version;
          const text = document.getText();
          const bom = previous?.bom ?? hasBom(filename);
          await this.raw(previous ? 'change' : 'open', { path: filename, version, text, bom });
          this.sent.set(filename, { document, version, bom });
        }
        const current = this.documents();
        if (current.length === this.sent.size && !this.diskDirty && current.every(document => {
          const sent = this.sent.get(canonical(document.uri.fsPath));
          return sent?.document === document && sent.version === document.version;
        })) return;
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
    this.acceptDiagnostics(response, document);
    return response;
  }
  acceptDiagnostics(response, document) {
    this.assertFresh(response);
    const result = response.result;
    if (!Array.isArray(result.diagnosticScopes)) {
      this.scoped = false;
      this.scopes.clear();
      this.documentScopes.clear();
      this.resolve.clear();
      this.diagnostics.clear();
      this.onScopesChanged?.();
      for (const [file, items] of diagnosticsByFile(result.diagnostics, this.output)) {
        this.diagnostics.set(vscode.Uri.file(file), [...items.values()]);
      }
      if (result.omittedDiagnostics) this.output.appendLine(`${result.omittedDiagnostics} additional diagnostics omitted`);
      return;
    }
    this.scoped = true;
    const filename = canonical(document.uri.fsPath);
    if ((this.documentScopes.get(filename)?.version ?? 0) > response.id) return;
    const files = new Set();
    const keys = [];
    const separate = new Set(result.diagnosticScopes.filter(scope => scope.kind === 'file').map(scope => scope.path));
    for (const scope of result.diagnosticScopes) {
      const key = JSON.stringify([scope.kind, scope.path]);
      keys.push(key);
      if ((this.scopeVersions.get(key) ?? 0) > response.id) continue;
      const items = scope.indices.map(index => result.diagnostics[index]);
      const inputs = new Set(scope.kind === 'file' ? [scope.path] :
        result.inputs.map(input => input.path).filter(input => !separate.has(input)));
      const unresolved = Boolean(scope.omitted) || items.some(item =>
        item.code.startsWith('RZ06') || item.code === 'RZ0202' || item.code === 'RZ0203');
      const byFile = diagnosticsByFile(items, this.output);
      const previous = this.scopes.get(key);
      for (const file of previous?.byFile?.keys() || []) files.add(file);
      for (const file of byFile.keys()) files.add(file);
      this.scopes.set(key, { kind: scope.kind, path: scope.path, inputs, unresolved, byFile });
      this.scopeVersions.set(key, response.id);
      if (scope.omitted) this.output.appendLine(`${scope.path}: ${scope.omitted} additional diagnostics omitted`);
      if (scope.kind !== 'file') {
        for (const [target, selection] of this.documentScopes) {
          if (target !== filename && selection.keys.includes(key) &&
              previous?.inputs.has(target) !== inputs.has(target)) this.resolve.add(target);
        }
      }
    }
    this.documentScopes.set(filename, { keys, version: response.id });
    this.resolve.delete(filename);
    this.dropUnusedScopes(files);
    this.publish(files);
  }
  dispose() {
    if (this.disposed) return;
    this.disposed = true;
    if (!this.exited) this.child.stdin.end();
    this.rejectPending(new Error('Rinzu editor disposed'));
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
  let checkTail = Promise.resolve();
  const checkOpen = () => {
    checkTail = checkTail.then(() => client.checkOpen(vscode.window.activeTextEditor?.document, report)).catch(report);
    return checkTail;
  };
  const schedule = () => {
    clearTimeout(timer);
    timer = setTimeout(checkOpen, 180);
  };
  const changed = (disk, files, structural) => { client.invalidate(disk, files, structural); schedule(); };
  const safe = async (operation, fallback) => { try { return await operation(); } catch (error) { report(error); return fallback; } };
  context.subscriptions.push(output, diagnostics, client,
    { dispose() { clearTimeout(timer); } },
    vscode.workspace.onDidOpenTextDocument(document => { if (isSource(document)) changed(false, [document.uri.fsPath], document.isDirty); }),
    vscode.workspace.onDidChangeTextDocument(event => { if (isSource(event.document) && event.contentChanges.length) changed(false, [event.document.uri.fsPath], !fs.existsSync(event.document.uri.fsPath)); }),
    vscode.workspace.onDidCloseTextDocument(document => { if (isSource(document)) changed(false, [document.uri.fsPath], !fs.existsSync(document.uri.fsPath)); }),
    vscode.workspace.onDidSaveTextDocument(document => { if (isSource(document)) changed(true, [document.uri.fsPath]); }),
    vscode.workspace.onDidRenameFiles(event => changed(true, event.files.flatMap(file => [file.oldUri.fsPath, file.newUri.fsPath]), true)),
    vscode.window.onDidChangeActiveTextEditor(editor => { if (editor && isSource(editor.document)) schedule(); }),
    vscode.languages.registerDefinitionProvider(selector, { provideDefinition: (document, position, token) => safe(async () => locations((await client.query('definition', document, { position }, token)).result.locations), []) }),
    vscode.languages.registerReferenceProvider(selector, { provideReferences: (document, position, referenceContext, token) => safe(async () => locations((await client.query('references', document, { position, includeDeclaration: referenceContext.includeDeclaration }, token)).result.locations), []) }),
    vscode.languages.registerCompletionItemProvider(selector, { provideCompletionItems: (document, position, token) => safe(async () => {
      const response = await client.query('completion', document, { position }, token);
      const prefix = document.lineAt(position.line).text.slice(0, position.character).match(/[A-Za-z_][A-Za-z0-9_.]*$/)?.[0] || '';
      const kinds = { function: vscode.CompletionItemKind.Function, variable: vscode.CompletionItemKind.Variable, parameter: vscode.CompletionItemKind.Variable, interface: vscode.CompletionItemKind.Interface, class: vscode.CompletionItemKind.Class, enum: vscode.CompletionItemKind.Enum, type: vscode.CompletionItemKind.TypeParameter, typeParameter: vscode.CompletionItemKind.TypeParameter, constant: vscode.CompletionItemKind.Constant, import: vscode.CompletionItemKind.Module, keyword: vscode.CompletionItemKind.Keyword };
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
      client.invalidate(true);
      return checkOpen();
    }),
    vscode.commands.registerCommand('rinzu.organizeImports', async () => {
      const document = vscode.window.activeTextEditor?.document;
      if (document && isSource(document)) return safe(async () => client.applyEdits(document, await client.query('organizeImports', document)), false);
    })
  );
  const watcher = vscode.workspace.createFileSystemWatcher('**/*.{rinzu,toml,lock,h,hpp}');
  const watch = watcher => {
    watcher.onDidChange(uri => changed(true, [uri.fsPath]));
    watcher.onDidCreate(uri => changed(true, [uri.fsPath], true));
    watcher.onDidDelete(uri => changed(true, [uri.fsPath], true));
    return watcher;
  };
  const external = new Map();
  client.onScopesChanged = () => {
    const roots = (vscode.workspace.workspaceFolders || []).map(folder => canonical(folder.uri.fsPath));
    const directories = new Set();
    for (const scope of client.scopes.values()) {
      for (const input of [scope.path, ...scope.inputs]) {
        if (!roots.some(root => within(root, input))) directories.add(path.dirname(input));
      }
    }
    for (const [directory, watcher] of external) {
      if (directories.has(directory)) continue;
      watcher.dispose();
      external.delete(directory);
    }
    for (const directory of directories) {
      if (!external.has(directory)) {
        external.set(directory, watch(vscode.workspace.createFileSystemWatcher(new vscode.RelativePattern(directory, '*'))));
      }
    }
  };
  context.subscriptions.push(watch(watcher), { dispose() { for (const watcher of external.values()) watcher.dispose(); } });
  schedule();
  return { client, diagnostics, range };
}
function deactivate() { active?.dispose(); active = undefined; }
module.exports = { activate, deactivate, SourceClient };
