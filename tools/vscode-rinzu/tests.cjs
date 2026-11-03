'use strict';
const vscode = require('vscode');
const assert = require('node:assert/strict');
const fs = require('node:fs');
const path = require('node:path');

async function run() {
  const root = vscode.workspace.workspaceFolders[0].uri.fsPath;
  const evidence = { schema:'rinzu-editor-host-tests/1', editorVersion:vscode.version, platform:process.platform, architecture:process.arch, workspace:root, cases:[] };
  const uri = name => vscode.Uri.file(path.join(root,'src',name));
  const open = async name => { const document = await vscode.workspace.openTextDocument(uri(name)); await vscode.window.showTextDocument(document); return document; };
  const replace = async (document,text) => { const editor = await vscode.window.showTextDocument(document); assert.equal(await editor.edit(builder => builder.replace(new vscode.Range(0,0,document.lineCount,0),text)),true); };
  const at = (document,needle) => { const index = document.getText().indexOf(needle); assert.ok(index>=0,needle); return document.positionAt(index); };
  const test = async (name,body) => {
    const start=Date.now(); const row={ name };
    try { row.details=await body(); row.status='passed'; }
    catch (error) { row.status='failed'; row.error=error.stack; }
    row.milliseconds=Date.now()-start; evidence.cases.push(row); console.log(`RINZU_EDITOR_TEST ${JSON.stringify(row)}`);
  };
  const main=await open('main.rinzu');
  assert.equal(main.languageId,'rinzu');
  const extension=vscode.extensions.getExtension('rinzu-local.rinzu-source');assert.ok(extension);
  const api=await extension.activate();const client=api.client;
  const valid='// 😀日本\r\nimport model;fn main(){let n:Int=model.value();}';
  let model,logic;
  try {
    await test('BOM CRLF Unicode diagnostic in a real text editor',async()=>{
      assert.equal(main.getText().startsWith('\ufeff'),false);
      const response=await client.check(main);const error=response.result.diagnostics.find(d=>d.code==='RZ0301');assert.ok(error);
      const editorDiagnostics=api.diagnostics.get(main.uri);assert.ok(editorDiagnostics?.length);
      const start=main.offsetAt(api.range(error.primary.range).start);
      assert.equal(error.primary.originalBytes.start,Buffer.byteLength(main.getText().slice(0,start))+3);
      assert.deepEqual(error.primary.range.start,{line:1,character:main.positionAt(start).character});
      assert.equal(main.eol,vscode.EndOfLine.CRLF);
      return {utf16:error.primary.range.start,originalBytes:error.primary.originalBytes,diagnostic:error.code,visibleDiagnostics:editorDiagnostics.length,bomExcluded:true};
    });
    await test('Unsaved dependency changes real diagnostics definition and completion providers',async()=>{
      model=await open('model.rinzu');await replace(model,'fn value()->Int{return 42;}fn hidden()->Int{return 99;}export value;');
      assert.ok(model.isDirty);assert.ok(fs.readFileSync(model.uri.fsPath,'utf8').includes('String'));
      assert.deepEqual((await client.check(main)).result.diagnostics,[]);
      const definitions=await vscode.commands.executeCommand('vscode.executeDefinitionProvider',main.uri,at(main,'value()'));
      assert.equal(definitions[0].uri.fsPath,model.uri.fsPath);
      const completions=await vscode.commands.executeCommand('vscode.executeCompletionItemProvider',main.uri,new vscode.Position(0,0));
      assert.ok(completions.items.some(item=>item.label==='model.value'));assert.ok(!completions.items.some(item=>item.label==='model.hidden'));
      return {dirtyDependency:model.isDirty,diskType:'String',overlayType:'Int',definition:definitions[0].uri.fsPath,publicCompletion:true,privateCompletion:false};
    });
    await test('Generic closure shadowing and references use actual registered providers',async()=>{
      logic=await open('logic.rinzu');
      const text='fn id<T>(a:T)->T{return a;}fn test()->Int{let item:Int=7;let read=fn(item:Int)->Int{return item;};let capture=fn()->Int{return item;};let text="item";return id<Int>(read(capture()));}// item\n';
      await replace(logic,text);assert.deepEqual((await client.check(logic)).result.diagnostics,[]);
      const capture=await vscode.commands.executeCommand('vscode.executeDefinitionProvider',logic.uri,at(logic,'item;};let text'));
      assert.equal(logic.offsetAt(capture[0].range.start),text.indexOf('item:Int=7'));
      const shadow=await vscode.commands.executeCommand('vscode.executeDefinitionProvider',logic.uri,at(logic,'item;};let capture'));
      assert.equal(logic.offsetAt(shadow[0].range.start),text.indexOf('fn(item:Int)')+3);
      const references=await vscode.commands.executeCommand('vscode.executeReferenceProvider',logic.uri,at(logic,'item:Int=7'));
      assert.equal(references.length,2);
      const generic=await vscode.commands.executeCommand('vscode.executeDefinitionProvider',logic.uri,at(logic,'id<Int>'));assert.equal(logic.offsetAt(generic[0].range.start),3);
      return {references:references.length,shadowing:true,closureCapture:true,genericSourceOffset:3,dirty:logic.isDirty};
    });
    await test('Invalid unsaved source discards success navigation then repairs',async()=>{
      await replace(model,'fn value( {');
      const response=await client.check(main);assert.equal(response.result.syntaxOnly,true);assert.ok(response.result.diagnostics.length);
      const definitions=await vscode.commands.executeCommand('vscode.executeDefinitionProvider',main.uri,at(main,'value()'));assert.deepEqual(definitions,[]);
      const edits=await vscode.commands.executeCommand('vscode.executeFormatDocumentProvider',model.uri,{tabSize:4,insertSpaces:true});assert.ok(!edits?.length);
      await replace(model,'fn value()->Int{return 42;}export value;');assert.deepEqual((await client.check(main)).result.diagnostics,[]);
      return {invalidDiagnostics:response.result.diagnostics.length,noOldDefinition:true,noInvalidFormat:true,repaired:true};
    });
    await test('Formatting and import command edit buffers without saving BOM or literals',async()=>{
      const text='// 😀日本\r\nimport z; import a; import z; import model;\r\n// 保持\r\nfn main(){let n:Int=model.value();let raw=r##"日本\r\n\0"##;}';
      await replace(main,text);
      const response=await client.query('organizeImports',main);assert.ok(response.result.edits.length);assert.equal(await client.applyEdits(main,response),true);
      assert.ok(main.getText().includes('import a;\r\nimport model;\r\nimport z;'));assert.ok(main.getText().includes('// 保持'));assert.ok(main.getText().includes('r##"日本\r\n\0"##'));
      const edits=await vscode.commands.executeCommand('vscode.executeFormatDocumentProvider',main.uri,{tabSize:4,insertSpaces:true});assert.ok(edits.length>0);
      const editor=await vscode.window.showTextDocument(main);assert.equal(await editor.edit(builder=>{for(const edit of edits)builder.replace(edit.range,edit.newText);}),true);
      assert.deepEqual((await client.query('format',main)).result.edits,[]);assert.ok(main.isDirty);assert.equal(fs.readFileSync(main.uri.fsPath).subarray(0,3).toString('hex'),'efbbbf');
      assert.ok(fs.readFileSync(main.uri.fsPath,'utf8').includes('let n:Int=model.value();}'));
      await replace(main,valid);return {formattedViaProvider:true,organizedViaGuardedClient:true,idempotent:true,dirty:true,diskUnchanged:true};
    });
    await test('Old server reply and edits are rejected after dependent buffer version changes',async()=>{
      await replace(main,valid);
      const oldEpoch=client.epoch;const old=await client.query('format',main);assert.ok(old.result.edits.length);const oldMainVersion=main.version;
      await replace(model,'fn value()->String{return "changed";}export value;');assert.equal(main.version,oldMainVersion);
      assert.throws(()=>client.assertFresh(old),error=>error.code==='ContentModified');
      await assert.rejects(client.applyEdits(main,old),error=>error.code==='ContentModified');assert.equal(main.getText(),valid);
      assert.ok(!api.diagnostics.get(main.uri)?.length);
      // The real server still has the last synchronized snapshot. Receive that
      // old payload after the editor edit and apply the production freshness gate.
      const late=await client.raw('diagnostics',{path:main.uri.fsPath,version:main.version});late.clientEpoch=oldEpoch;
      assert.throws(()=>client.assertFresh(late),error=>error.code==='ContentModified');
      await replace(model,'fn value()->Int{return 42;}export value;');assert.deepEqual((await client.check(main)).result.diagnostics,[]);
      return {mainVersionUnchanged:oldMainVersion,dependencyChanged:true,rejectedOldEdit:true,rejectedLateRealReply:true,staleDiagnosticsCleared:true};
    });
    await test('File move refreshes open editor identity and definition target',async()=>{
      await replace(model,'fn value()->Int{return 42;}export value;');
      assert.equal(await model.save(),true);
      const edit=new vscode.WorkspaceEdit();edit.renameFile(model.uri,uri('fresh.rinzu'));assert.equal(await vscode.workspace.applyEdit(edit),true);
      const fresh=await open('fresh.rinzu');await replace(main,'// 😀日本\r\nimport fresh;fn main(){let n:Int=fresh.value();}');client.invalidate(true);
      assert.deepEqual((await client.check(main)).result.diagnostics,[]);
      const definitions=await vscode.commands.executeCommand('vscode.executeDefinitionProvider',main.uri,at(main,'value()'));assert.equal(definitions[0].uri.fsPath,fresh.uri.fsPath);
      assert.ok(!fs.existsSync(path.join(root,'src/model.rinzu')));assert.ok(![...client.sent.keys()].some(filename=>filename.endsWith('/model.rinzu')));
      return {newPath:fresh.uri.fsPath,definitionMoved:true,oldOverlayRemoved:true};
    });
    await test('Explicit file entry initializes the actual adapter and navigates path imports',async()=>{
      const config=vscode.workspace.getConfiguration('rinzu');const original=config.get('entry');
      const standalone=path.join(root,'src/path-entry.rinzu');const dependency=path.join(root,'src/path-model.rinzu');
      const format=text=>{const result=require('node:child_process').spawnSync(config.get('executable'),['fmt','-'],{input:text,encoding:'utf8'});assert.equal(result.status,0,result.stderr);return result.stdout;};
      const disk=format('fn value()->String{return "disk";}fn hidden()->Int{return 9;}export value;');fs.writeFileSync(dependency,disk);
      fs.writeFileSync(standalone,format('import "./path-model.rinzu" as model;fn main(){let n:Int=model.value();}'));
      await config.update('entry','src/path-entry.rinzu',vscode.ConfigurationTarget.Workspace);
      const {SourceClient}=require('./extension.js');const fileClient=new SourceClient({append(){},appendLine(){}},api.diagnostics);
      try {
        const entryDocument=await open('path-entry.rinzu');const modelDocument=await open('path-model.rinzu');
        assert.ok((await fileClient.check(entryDocument)).result.diagnostics.length);
        await replace(modelDocument,format('fn value()->Int{return 42;}export value;'));
        const response=await fileClient.check(entryDocument);assert.deepEqual(response.result.diagnostics,[]);
        const definition=await fileClient.query('definition',entryDocument,{position:{line:at(entryDocument,'value()').line,character:at(entryDocument,'value()').character}});
        assert.equal(definition.result.locations[0].path,dependency);
        const completions=await fileClient.query('completion',entryDocument,{position:{line:0,character:0}});
        assert.ok(completions.result.items.some(item=>item.label==='model.value'));assert.ok(!completions.result.items.some(item=>item.label==='model.hidden'));
        assert.equal(fs.readFileSync(dependency,'utf8'),disk);
        return {entry:fileClient.entry,pathDefinition:definition.result.locations[0].path,unsavedDependency:true,diskUnchanged:true};
      } finally {fileClient.dispose();await config.update('entry',original,vscode.ConfigurationTarget.Workspace);}
    });
    await test('Missing configured executable fails promptly and clears pending requests',async()=>{
      const config=vscode.workspace.getConfiguration('rinzu');const original=config.get('executable');
      await config.update('executable',path.join(root,'missing-rinzu'),vscode.ConfigurationTarget.Workspace);
      const {SourceClient}=require('./extension.js');
      const faulty=new SourceClient({append(){},appendLine(){}},api.diagnostics);
      try {await assert.rejects(faulty.raw('initialize'));await assert.rejects(faulty.raw('initialize'));assert.equal(faulty.pending.size,0);assert.equal(faulty.exited,true);}
      finally {faulty.dispose();await config.update('executable',original,vscode.ConfigurationTarget.Workspace);}
      return {failedSpawn:true,pendingRequests:0,subsequentQueryRejected:true};
    });
  } finally {
    client.dispose();evidence.finishedUtc=new Date().toISOString();
    const output=process.env.RINZU_EDITOR_HOST_EVIDENCE || path.join(root,'host-evidence.json');fs.writeFileSync(output,JSON.stringify(evidence,null,2)+'\n');
  }
  assert.equal(evidence.cases.filter(row=>row.status==='failed').length,0,JSON.stringify(evidence,null,2));
}
module.exports={run};
