'use strict';

const assert = require('node:assert/strict');
const { EventEmitter } = require('node:events');
const fs = require('node:fs');
const os = require('node:os');
const path = require('node:path');
const { PassThrough } = require('node:stream');
const test = require('node:test');
const vm = require('node:vm');

function fixture(t, { spawn = true } = {}) {
  const root = fs.realpathSync(fs.mkdtempSync(path.join(os.tmpdir(), 'rinzu-vscode-diagnostics-')));
  t.after(() => fs.rmSync(root, { recursive: true, force: true }));
  const workspace = {
    textDocuments: [],
    getConfiguration: () => ({ get: (_name, fallback) => fallback }),
  };
  const vscode = {
    workspace,
    Uri: { file: fsPath => ({ fsPath, scheme: 'file' }) },
    Range: class Range { constructor(...coordinates) { this.coordinates = coordinates; } },
    Diagnostic: class Diagnostic { constructor(range, message) { Object.assign(this, { range, message }); } },
    DiagnosticSeverity: { Error: 0 },
    Location: class Location { constructor(uri, range) { Object.assign(this, { uri, range }); } },
    DiagnosticRelatedInformation: class Related {
      constructor(location, message) { Object.assign(this, { location, message }); }
    },
  };
  const child = Object.assign(new EventEmitter(), {
    stdin: new PassThrough(), stdout: new PassThrough(), stderr: new PassThrough(), exitCode: 0,
  });
  const module = { exports: {} };
  vm.runInNewContext(fs.readFileSync(path.join(__dirname, 'extension.js'), 'utf8'), {
    Buffer,
    module,
    require: name => {
      if (name === 'vscode') return vscode;
      if (name === 'node:child_process') return { spawn: () => child };
      return require(name);
    },
    setTimeout,
    clearTimeout,
  });
  const visible = new Map();
  const output = [];
  const client = new module.exports.SourceClient(
    { append() {}, appendLine: message => output.push(message) },
    {
      set: (uri, diagnostics) => visible.set(uri.fsPath, diagnostics),
      delete: uri => visible.delete(uri.fsPath),
      clear: () => visible.clear(),
    },
  );
  if (spawn) child.emit('spawn');
  t.after(() => { client.dispose(); child.stdout.end(); child.stderr.end(); });
  const file = name => path.join(root, name);
  function open(name) {
    const filename = file(name);
    fs.mkdirSync(path.dirname(filename), { recursive: true });
    fs.writeFileSync(filename, '');
    const document = { uri: vscode.Uri.file(filename), version: 1, languageId: 'rinzu', isClosed: false };
    workspace.textDocuments.push(document);
    return document;
  }
  function close(document) {
    document.isClosed = true;
    workspace.textDocuments.splice(workspace.textDocuments.indexOf(document), 1);
    client.invalidate(false, [document.uri.fsPath]);
  }
  let id = 0;
  const response = (scopes, diagnostics, inputs) => ({
    id: ++id,
    clientEpoch: client.epoch,
    documents: workspace.textDocuments.map(document => ({ path: document.uri.fsPath, version: document.version })),
    result: { diagnosticScopes: scopes, diagnostics, inputs: inputs.map(path => ({ path })) },
  });
  const scope = (name, indices, kind = 'package') => ({ kind, path: file(name), indices });
  const diagnostic = (filename, message, extra = {}) => ({
    code: 'RZ0412',
    primary: { path: file(filename), range: { start: { line: 1, character: 2 }, end: { line: 1, character: 3 } } },
    secondary: [],
    message,
    ...extra,
  });
  return { client, child, visible, output, file, open, close, response, scope, diagnostic };
}

test('reopening a document at the same version replaces the server buffer and BOM', async t => {
  const { client, file, open, close } = fixture(t);
  const first = open('main.rinzu');
  first.getText = () => 'old text';
  const commands = [];
  client.raw = async (method, params) => { commands.push({ method, ...params }); };
  await client.synchronize();

  close(first);
  const reopened = open('main.rinzu');
  reopened.getText = () => 'new text';
  fs.writeFileSync(file('main.rinzu'), '\ufeffnew text');
  await client.synchronize();
  assert.deepEqual(commands, [
    { method: 'open', path: first.uri.fsPath, version: 1, text: 'old text', bom: false },
    { method: 'close', path: first.uri.fsPath, version: 1 },
    { method: 'open', path: reopened.uri.fsPath, version: 1, text: 'new text', bom: true },
  ]);
});

test('a failed spawn rejects requests before writing to the service pipe', async t => {
  const { client, child } = fixture(t, { spawn: false });
  const request = client.raw('initialize');
  const failed = new Error('spawn rinzu ENOENT');
  child.emit('error', failed);
  await assert.rejects(request, failed);
  assert.equal(child.stdin.readableLength, 0);
  assert.equal(client.pending.size, 0);
});

test('retains unrelated package diagnostics and rechecks both consumers of a changed SDK', async t => {
  const { client, visible, file, open, response, scope, diagnostic } = fixture(t);
  const app = open('app/src/main.rinzu');
  const other = open('other/src/main.rinzu');
  const sdk = open('sdk/src/model.rinzu');
  const sdkPath = sdk.uri.fsPath;
  client.acceptDiagnostics(response([scope('app/rinzu.toml', [0])],
    [diagnostic('app/src/main.rinzu', 'app')], [app.uri.fsPath, sdkPath]), app);
  client.acceptDiagnostics(response([scope('other/rinzu.toml', [0])],
    [diagnostic('other/src/main.rinzu', 'other')], [other.uri.fsPath]), other);
  client.acceptDiagnostics(response([scope('sdk/rinzu.toml', [0])],
    [diagnostic('sdk/src/model.rinzu', 'sdk')], [sdkPath]), sdk);
  assert.equal(visible.size, 3);
  ++sdk.version;
  client.invalidate(false, [sdkPath]);
  assert.deepEqual([...visible.keys()], [other.uri.fsPath]);

  const checked = [];
  client.query = async (_method, document) => {
    checked.push(document.uri.fsPath);
    const manifest = document === sdk ? 'sdk/rinzu.toml' : 'app/rinzu.toml';
    return response([scope(manifest, [])], [], [document.uri.fsPath, sdkPath]);
  };
  await client.checkOpen(other, assert.fail);
  assert.deepEqual(checked, [app.uri.fsPath, sdkPath]);
  assert.deepEqual([...visible.keys()], [file('other/src/main.rinzu')]);
});

test('merges duplicate diagnostics and keeps the scope until its last target closes', t => {
  const { client, visible, file, open, close, response, scope, diagnostic } = fixture(t);
  const app = open('app/src/main.rinzu');
  const second = open('app/src/second.rinzu');
  const sdk = open('sdk/src/model.rinzu');
  const item = diagnostic('shared.rinzu', 'uncovered', {
    suggestions: [
      { applicability: 'Explanation', message: 'handle the remaining Errors', replacement: null },
      { applicability: 'MachineApplicable', message: 'replace this', replacement: {} },
    ],
    secondary: [{ location: { path: app.uri.fsPath, range: {
      start: { line: 2, character: 0 }, end: { line: 2, character: 8 },
    } }, message: 'the Error originates here' }],
  });
  const packageInputs = [app.uri.fsPath, second.uri.fsPath];
  for (const target of [app, second]) {
    client.acceptDiagnostics(response([scope('app/rinzu.toml', [0])], [item], packageInputs), target);
  }
  client.acceptDiagnostics(response([scope('sdk/rinzu.toml', [0])], [item], [sdk.uri.fsPath]), sdk);
  const shared = file('shared.rinzu');
  assert.equal(visible.get(shared).length, 1);
  assert.equal(visible.get(shared)[0].message, 'uncovered\nHelp: handle the remaining Errors');
  assert.equal(visible.get(shared)[0].relatedInformation[0].location.uri.fsPath, app.uri.fsPath);
  close(sdk);
  assert.equal(visible.get(shared).length, 1);
  close(app);
  assert.ok(client.needsCheck(second.uri.fsPath));
  client.acceptDiagnostics(response([scope('app/rinzu.toml', [0])], [item], [second.uri.fsPath]), second);
  assert.equal(visible.get(shared).length, 1);
  close(second);
  assert.ok(!visible.has(shared));
  assert.equal(client.scopes.size, 0);
});

test('rejects late snapshots and late requests for the same scope', t => {
  const { client, visible, open, response, scope, diagnostic } = fixture(t);
  const app = open('app/main.rinzu');
  const second = open('app/second.rinzu');
  const inputs = [app.uri.fsPath, second.uri.fsPath];
  const old = response([scope('app/rinzu.toml', [0])], [diagnostic('app/main.rinzu', 'old')], inputs);
  const current = response([scope('app/rinzu.toml', [])], [], inputs);
  client.acceptDiagnostics(current, second);
  client.acceptDiagnostics(old, app);
  assert.equal(visible.size, 0);
  ++app.version;
  client.invalidate(false, [app.uri.fsPath]);
  assert.throws(() => client.acceptDiagnostics(old, app), { code: 'ContentModified' });
  assert.equal(visible.size, 0);
});

test('re-resolves a loose file when a manifest is created and keeps legacy responses working', t => {
  const { client, visible, output, file, open, response, scope, diagnostic } = fixture(t);
  const app = open('app/main.rinzu');
  const item = diagnostic('app/main.rinzu', 'loose');
  client.acceptDiagnostics(response([scope('app/main.rinzu', [0], 'file')], [item], [app.uri.fsPath]), app);
  client.invalidate(true, [file('app/rinzu.toml')], true);
  assert.ok(client.needsCheck(app.uri.fsPath));
  client.acceptDiagnostics(response([scope('app/rinzu.toml', [])], [], [app.uri.fsPath]), app);
  assert.equal(visible.size, 0);
  assert.equal(client.scopes.size, 1);
  const legacy = response(undefined, [item], [app.uri.fsPath]);
  legacy.result.omittedDiagnostics = 2;
  client.acceptDiagnostics(legacy, app);
  assert.equal(visible.get(app.uri.fsPath)[0].message, 'loose');
  assert.deepEqual(output, ['2 additional diagnostics omitted']);
  client.invalidate(false, [app.uri.fsPath]);
  assert.equal(visible.size, 0);
});

test('a new candidate rechecks unresolved imports while preserving other packages', t => {
  const { client, visible, file, open, response, scope, diagnostic } = fixture(t);
  const app = open('app/src/main.rinzu');
  const unrelated = open('other/src/main.rinzu');
  client.acceptDiagnostics(response([scope('app/rinzu.toml', [0])],
    [diagnostic('app/src/main.rinzu', 'missing import', { code: 'RZ0603' })], [app.uri.fsPath]), app);
  client.acceptDiagnostics(response([scope('other/rinzu.toml', [0])],
    [diagnostic('other/src/main.rinzu', 'still invalid')], [unrelated.uri.fsPath]), unrelated);
  client.invalidate(true, [file('sdk/src/new.rinzu')], true);
  assert.ok(client.needsCheck(app.uri.fsPath));
  assert.ok(!client.needsCheck(unrelated.uri.fsPath));
  assert.deepEqual([...visible.keys()], [unrelated.uri.fsPath]);
});

test('shares hover requests and checks cancellation and document freshness per caller', async t => {
  const { client, open, close, response } = fixture(t);
  const document = open('main.rinzu');
  const position = { line: 1, character: 4 };
  client.ready = Promise.resolve({ result: { methods: ['hover'] } });
  let resolve, requests = 0;
  client.query = () => {
    ++requests;
    return new Promise(done => { resolve = done; });
  };
  const cancelled = { isCancellationRequested: false };
  const one = client.hover(document, position, cancelled);
  const two = client.hover(document, position, { isCancellationRequested: false });
  await new Promise(done => setImmediate(done));
  assert.equal(requests, 1);
  cancelled.isCancellationRequested = true;
  const result = response([], [], []);
  result.result.hover = { label: 'await pending' };
  resolve(result);
  assert.equal(await one, undefined);
  assert.equal((await two).label, 'await pending');

  const stale = client.hover(document, position, { isCancellationRequested: false });
  await new Promise(done => setImmediate(done));
  close(document);
  open('main.rinzu');
  resolve(result);
  await assert.rejects(stale, { code: 'ContentModified' });
});

test('hover does not query an older driver that lacks the capability', async t => {
  const { client, open } = fixture(t);
  client.ready = Promise.resolve({ result: { methods: ['definition'] } });
  client.query = () => assert.fail('unsupported hover was sent');
  assert.equal(await client.hover(open('main.rinzu'), { line: 0, character: 0 }), undefined);
});

test('checks the active buffer before other buffers without building a hover index', async t => {
  const { client, open, response, scope } = fixture(t);
  const background = open('background.rinzu');
  const active = open('active.rinzu');
  const requests = [];
  client.query = async (method, document, params) => {
    assert.equal(method, 'diagnostics');
    assert.equal(params, undefined);
    requests.push(document);
    const name = path.basename(document.uri.fsPath);
    return response([scope(name, [], 'file')], [], [document.uri.fsPath]);
  };
  await client.checkOpen(active, assert.fail);
  assert.deepEqual(requests, [active, background]);
});

test('hover retries capability discovery after a transient timeout', async t => {
  const { client, open, response } = fixture(t);
  const document = open('main.rinzu');
  const position = { line: 0, character: 0 };
  let probes = 0;
  client.raw = async method => {
    assert.equal(method, 'initialize');
    if (++probes === 1) throw new Error('Rinzu editor request timed out');
    return { result: { methods: ['hover'] } };
  };
  client.query = async () => {
    const reply = response([], [], []);
    reply.result.hover = { label: 'value' };
    return reply;
  };
  await assert.rejects(client.hover(document, position), /timed out/);
  assert.equal((await client.hover(document, position)).label, 'value');
  assert.equal(probes, 2);
});
