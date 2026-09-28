// 실제 Rust registry host가 실행하는 작은 driver. 제품 controller/client만 메모리에서 변환한다.
const assert = require("node:assert/strict");
const fs = require("node:fs");
const path = require("node:path");
const Module = require("node:module");
const root = path.resolve(__dirname, "../../../..");
const req = Module.createRequire(path.join(root, "package.json"));
const ts = req("typescript");
const modules = new Map();
function load(relative) {
  const filename = path.join(root, relative);
  if (modules.has(filename)) return modules.get(filename).exports;
  const module = new Module(filename);
  modules.set(filename, module);
  module.require = (name) => {
    if (name.startsWith(".")) {
      const target = path.resolve(path.dirname(filename), name);
      if (name.endsWith(".json")) return req(target);
      return load(path.relative(root, fs.existsSync(target + ".ts") ? target + ".ts" : path.join(target,"index.ts")));
    }
    return req(name);
  };
  const code = ts.transpileModule(fs.readFileSync(filename, "utf8"), {
    compilerOptions: { target: ts.ScriptTarget.ES2022, module: ts.ModuleKind.CommonJS, esModuleInterop: true },
  }).outputText;
  module._compile(code, filename);
  return module.exports;
}
const { GuardedClient } = load("src/bridge/client.ts");
const { TemplateController } = load("src/app/controller.ts");
const lines = require("node:readline").createInterface({ input: process.stdin });
let serial = 0;
const pending = new Map();
lines.on("line", (line) => {
  const packet = JSON.parse(line);
  const entry = pending.get(packet.serial);
  assert.ok(entry, "response must match the waiting caller");
  pending.delete(packet.serial);
  packet.ok ? entry.resolve(packet.value) : entry.reject(packet.value);
});
function rpc(packet) {
  const id = ++serial;
  return new Promise((resolve, reject) => {
    pending.set(id, { resolve, reject });
    process.stdout.write(JSON.stringify({ serial: id, ...packet }) + "\n");
  });
}
const writeIds = new Set();
let loseSubmit = true;
const client = new GuardedClient({
  listen: async () => () => {},
  invoke: async (name, body) => {
    assert.equal(name, "guarded");
    const command = JSON.parse(new TextDecoder().decode(body));
    const reply = await rpc({ command });
    if (command.action === "submit" && ["create_template", "update_template", "duplicate_template", "tombstone_template"].includes(command.input.kind)) {
      writeIds.add(command.operation);
      if (loseSubmit) { loseSubmit = false; throw Error("synthetic lost accepted submit reply"); }
    }
    return reply;
  },
});
const controller = new TemplateController(client);
const deadline = setTimeout(() => { console.error("M271 controller integration deadline"); process.exit(1); }, 60000);
async function main() {
  const ready = new Promise((resolve) => {
    const stop = controller.subscribe(() => { if (controller.snapshot().ready) { stop(); resolve(); } });
  });
  controller.start();
  await ready;
  if (controller.snapshot().startupLoading) {
    await new Promise((resolve) => {
      const stop = controller.subscribe(() => {
        if (!controller.snapshot().startupLoading) {
          stop();
          resolve();
        }
      });
    });
  }
  controller.setRoot(process.env.WB_UI_PROJECT);
  await controller.open();
  assert.equal(controller.snapshot().project.status, "Ready");
  if (process.env.WB_UI_SCENARIO === "m272") return fieldFlow();
  if (process.env.WB_UI_SCENARIO === "m274") return managementFlow();
  await controller.navigate({ kind: "create" });
  controller.setName("  실제 controller 생성\n ");
  await Promise.all([controller.save(), controller.save()]);
  const first = controller.snapshot().selection;
  assert.equal(first.content.name, "  실제 controller 생성\n ");
  assert.equal(first.content.revision, "1");
  assert.equal(writeIds.size, 1);
  await controller.rename();
  controller.setName("닫기 취소 후 실제 저장");
  await rpc({ nativeClose: true });
  await controller.checkStatus();
  assert.ok(controller.snapshot().prompt.attempt);
  assert.equal((await client.appStatus()).closing, false);
  await controller.decide(false);
  await controller.save();
  await controller.refresh();
  const saved = controller.snapshot().selection;
  assert.equal(saved.content.id, first.content.id);
  assert.equal(saved.content.name, "닫기 취소 후 실제 저장");
  assert.equal(saved.content.revision, "2");
  assert.equal(writeIds.size, 2);
  assert.equal(controller.snapshot().sessions.length, 0);
  assert.equal(controller.snapshot().operations.length, 0);
  await controller.rename();
  controller.setName("명시적으로 버릴 미제출 폼");
  await rpc({ nativeClose: true });
  await controller.checkStatus();
  await controller.decide(true);
  await rpc({ awaitShutdown: true });
  await controller.checkStatus();
  await controller.acknowledgeShutdown();
  const app = await client.appStatus();
  assert.equal(app.closing, true);
  assert.equal(app.normal_exit_allowed, true);
  assert.equal(writeIds.size, 2);
  process.stdout.write(JSON.stringify({ done: true, summary: { id: saved.content.id, name: saved.content.name, revision: saved.content.revision, writes: writeIds.size, retained: app.retained_edits, layer: "production controller/client to mock Tauri registry and real filesystem; no Windows GUI" } }) + "\n");
  clearTimeout(deadline);
  lines.close();
}
async function fieldFlow() {
  await controller.navigate({kind:"create"});
  controller.setName("Field 실제 저장");
  await controller.save();
  await controller.navigate({kind:"new_field"});
  const draft=controller.snapshot().fieldEditor.drafts[0];
  controller.setField("create",{...draft.edit,label:"첫 Field"});
  loseSubmit=true;
  await Promise.all([controller.saveField("create"),controller.saveField("create")]);
  assert.ok(!controller.snapshot().error,"Field create must complete: " + controller.snapshot().error);
  let selected=controller.snapshot().selection;
  assert.equal(selected.content.fields[0].label,"첫 Field");
  assert.equal(selected.content.fields[0].introducedRevision,"2");
  const label=controller.snapshot().fieldEditor.drafts.find(d=>d.property==="label");
  controller.setField("label",{...label.edit,label:"이름 수정 Field"});
  await controller.saveField("label");
  assert.equal(controller.snapshot().selection.content.fields[0].label,"이름 수정 Field");
  assert.equal(writeIds.size,3);
  const expected = [];
  async function apply(property, edit) {
    await controller.reconfirmField(property);
    controller.setField(property, edit);
    await controller.saveField(property);
    assert.ok(!controller.snapshot().error, "closed Field edit must complete");
  }
  for (const kind of ["number","date","time","duration","single_choice","multi_choice","rich_text"]) {
    await controller.navigate({kind:"new_field"});
    const draft=controller.snapshot().fieldEditor.drafts[0];
    const options=[{id:crypto.randomUUID(),label:"처음 선택지"},{id:crypto.randomUUID(),label:"두 번째 선택지"}];
    const initial=kind==="number"?{kind,value:"12345678901234567890.123456789"}:kind==="date"?{kind,value:"2024-02-29"}:kind==="time"?{kind,value:"23:59:59.999"}:kind==="duration"?{kind,milliseconds:"9223372036854775807"}:kind==="single_choice"?{kind,option:options[0].id}:kind==="multi_choice"?{kind,options:options.map(o=>o.id).sort()}:{kind:"unset"};
    controller.setField("create",{...draft.edit,label:kind,configuration:kind.endsWith("choice")?{kind,options}:{kind},default:initial});
    await controller.saveField("create");
    const id=draft.edit.field;
    if (kind.endsWith("choice")) {
      const before=controller.snapshot().selection.content.fields.find(f=>f.id===id);
      controller.startOption();
      const added=controller.snapshot().fieldEditor.drafts.find(d=>d.edit.kind==="add_option");
      assert.ok(added,"active choice must expose an add draft");
      const optionId=added.edit.option.id;
      controller.setField(added.property,{...added.edit,option:{id:optionId,label:"  추가\n선택지 🌿  "}});
      loseSubmit=true;
      await Promise.all([controller.saveField(added.property),controller.saveField(added.property)]);
      assert.ok(!controller.snapshot().error,"option add must cross guarded storage");
      controller.startOption(optionId);
      const rename=controller.snapshot().fieldEditor.drafts.find(d=>d.edit.kind==="rename_option");
      controller.setField(rename.property,{...rename.edit,label:"\n이름 변경 🌱\n"});
      await controller.saveField(rename.property);
      assert.ok(!controller.snapshot().error,"option rename must cross guarded storage");
      const after=controller.snapshot().selection.content.fields.find(f=>f.id===id);
      assert.ok(after.options.some(o=>o.id===optionId&&o.label==="\n이름 변경 🌱\n"),"stable option label readback");
      assert.ok(JSON.stringify(before.default)===JSON.stringify(after.default)&&JSON.stringify(before.initialDefault)===JSON.stringify(after.initialDefault),"option changes must preserve both selected values");
      controller.startOrder();
      controller.moveOrder("options_order",optionId,0);
      controller.cancelOrder("options_order");
      assert.ok(controller.snapshot().fieldEditor.drafts.find(d=>d.property==="options_order").edit.options.join()===after.optionOrder.join(),"order cancel must restore original preview");
      controller.moveOrder("options_order",optionId,0);
      await controller.saveField("options_order");
      const reordered=controller.snapshot().selection.content.fields.find(f=>f.id===id);
      assert.ok(reordered.optionOrder[0]===optionId,"option reorder must save exact active order");
      assert.ok(JSON.stringify(reordered.default)===JSON.stringify(before.default)&&JSON.stringify(reordered.initialDefault)===JSON.stringify(before.initialDefault),"display order must not reorder selected values");
    }
    const file=path.join(process.env.WB_UI_PROJECT,"templates",controller.snapshot().selection.content.id+".json");
    await apply("label",{kind:"field_label",field:id,label:kind+" 변경"});
    await apply("required",{kind:"field_required",field:id,required:true});
    await apply("presentation",{kind:"field_presentation",field:id,token:"display-token"});
    assert.equal(controller.snapshot().selection.content.fields.find(f=>f.id===id).presentation,"display-token");
    const before=fs.readFileSync(file),mtime=fs.statSync(file).mtimeMs;
    await apply("default",kind==="rich_text"?{kind:"keep_default",field:id}:{kind:"default",field:id,value:initial});
    assert.ok(before.equals(fs.readFileSync(file)),"Fresh equal/Keep must preserve exact disk bytes");
    assert.equal(fs.statSync(file).mtimeMs,mtime);
    if(kind!=="rich_text") {
      const next=kind==="number"?{kind,value:"-0.123456789012345678901"}:kind==="date"?{kind,value:"2026-09-12"}:kind==="time"?{kind,value:"00:00:00.000"}:kind==="duration"?{kind,milliseconds:"-9223372036854775808"}:kind==="single_choice"?{kind,option:options[1].id}:{kind,options:[options[1].id]};
      await apply("default",{kind:"default",field:id,value:next});
      assert.ok(JSON.stringify(controller.snapshot().selection.content.fields.find(f=>f.id===id).default)===JSON.stringify(next),"Fresh value readback differs");
    }
    if(kind.endsWith("choice")) {
      const archive=async(option,repair)=>{
        controller.startArchive(option);
        const d=controller.snapshot().fieldEditor.drafts.find(d=>d.edit.kind==="archive_option"&&d.edit.option===option);
        assert.ok(d,"archive confirmation must have an exact option owner");
        controller.setField(d.property,{...d.edit,repair});
        controller.confirmArchive(d.property,true);
        const revision=BigInt(controller.snapshot().selection.content.revision);
        const writes=writeIds.size;
        await controller.saveField(d.property);
        assert.ok(!controller.snapshot().error,"atomic option archive must complete");
        assert.ok(BigInt(controller.snapshot().selection.content.revision)===revision+1n&&writeIds.size===writes+1,"archive and repair must share one revision and request");
        const field=controller.snapshot().selection.content.fields.find(f=>f.id===id);
        assert.ok(field.options.some(o=>o.id===option&&o.lifecycle==="Archived")&&!field.optionOrder.includes(option),"archive lifecycle and order must agree");
        assert.ok(JSON.stringify(field.initialDefault)===JSON.stringify(initial),"archive must keep historical initial selections");
      };
      await archive(options[0].id,null); // current는 두 번째 선택만 참조하고 initial은 첫 선택을 유지한다.
      const extra=controller.snapshot().selection.content.fields.find(f=>f.id===id).optionOrder.find(o=>o!==options[1].id);
      await archive(options[1].id,kind==="single_choice"?{kind,option:extra}:{kind,options:[extra]});
      await archive(extra,{kind:"unset"});
    }
    await apply("default",{kind:"default",field:id,value:{kind:"unset"}});
    const unsetBytes=fs.readFileSync(file);
    await apply("default",{kind:"keep_default",field:id});
    assert.ok(unsetBytes.equals(fs.readFileSync(file)),"Keep after unset must be NoWrite");
    if(kind.endsWith("choice")) {
      controller.startArchive();controller.confirmArchive("archive_field",true);
      await controller.saveField("archive_field");
      assert.ok(!controller.snapshot().error,"Field archive must use the existing guarded save");
      assert.ok(controller.snapshot().fieldEditor.drafts.length===0,"archived Field becomes read only");
    }
    const final=controller.snapshot().selection.content.fields.find(f=>f.id===id);
    assert.equal(final.required,true);
    assert.ok(JSON.stringify(final.initialDefault)===JSON.stringify(initial),"historical initial default changed");
    expected.push(final);
  }
  await controller.navigate({kind:"fields_order"});
  const order=controller.snapshot().selection.content.fieldOrder;
  controller.moveOrder("fields_order",order.at(-1),0);
  await controller.saveField("fields_order");
  assert.ok(!controller.snapshot().error,"Field reorder must save without a selected Field");
  const expectedOrder=controller.snapshot().selection.content.fieldOrder;
  const templateFile=path.join(process.env.WB_UI_PROJECT,"templates",controller.snapshot().selection.content.id+".json");
  const sameBytes=fs.readFileSync(templateFile),sameTime=fs.statSync(templateFile).mtimeMs;
  await controller.saveField("fields_order");
  assert.ok(sameBytes.equals(fs.readFileSync(templateFile))&&sameTime===fs.statSync(templateFile).mtimeMs,"equal Field order must not write or change mtime");
  const finalRevision=controller.snapshot().selection.content.revision;
  await controller.navigate({kind:"close_field"});
  await controller.navigate({kind:"close_project"});
  await rpc({awaitShutdown:true});
  await controller.checkStatus();
  await controller.acknowledgeShutdown();
  await controller.checkStatus();
  await controller.retireProject();
  controller.setRoot(process.env.WB_UI_PROJECT);
  await controller.open();
  await controller.navigate({kind:"select",id:selected.content.id});
  selected=controller.snapshot().selection;
  assert.equal(selected.content.fields.find(f=>f.id===draft.edit.field).label,"이름 수정 Field");
  assert.equal(selected.content.revision,finalRevision);
  assert.ok(JSON.stringify(selected.content.fieldOrder)===JSON.stringify(expectedOrder),"reopened active Field order differs");
  for(const field of expected) assert.ok(JSON.stringify(selected.content.fields.find(f=>f.id===field.id))===JSON.stringify(field),"reopened definition differs");
  await rpc({nativeClose:true});
  await controller.checkStatus();
  await rpc({awaitShutdown:true});
  await controller.checkStatus();
  await controller.acknowledgeShutdown();
  process.stdout.write(JSON.stringify({done:true,summary:{id:selected.content.id,name:selected.content.name,revision:selected.content.revision,writes:writeIds.size,fields:selected.content.fields.length,reopened:true,kinds:8}})+"\n");
  clearTimeout(deadline); lines.close();
}
async function managementFlow() {
  const originalId="99999999-9999-4999-8999-000000000064";
  const templatePath=id=>path.join(process.env.WB_UI_PROJECT,"templates",id+".json");
  const documentPath=path.join(process.env.WB_UI_PROJECT,"documents","99999999-9999-4999-8999-0000000000c8.json");
  const originalBytes=fs.readFileSync(templatePath(originalId)),documentBytes=fs.readFileSync(documentPath);
  await controller.navigate({kind:"select",id:originalId});
  const original=controller.snapshot().selection;
  await controller.navigate({kind:"duplicate"});await controller.navigate({kind:"cancel"});assert.equal(writeIds.size,0);
  await controller.navigate({kind:"duplicate"});await Promise.all([controller.executeTemplateAction(),controller.executeTemplateAction()]);
  const first=controller.snapshot().selection;assert.ok(first.content.id!==originalId,"duplicate identity must be new");assert.equal(writeIds.size,1);
  assert.ok(first.content.name===original.content.name,"duplicate name must come from stored view");
  const compare=(source,copy)=>{
    const mapping=new Map();
    assert.equal(source.content.fields.length,copy.content.fields.length);
    for(const field of source.content.fields) {
      const next=copy.content.fields.find(f=>f.label===field.label);assert.ok(next&&next.id!==field.id,"all Field IDs are remapped");mapping.set(field.id,next.id);
      assert.equal(next.lifecycle,field.lifecycle);assert.equal(next.required,field.required);assert.ok(next.presentation===field.presentation,"presentation preserved");
      for(const option of field.options) {
        const mapped=next.options.find(o=>o.label===option.label);assert.ok(mapped&&mapped.id!==option.id,"all Option IDs are remapped");assert.equal(mapped.lifecycle,option.lifecycle);mapping.set(option.id,mapped.id);
      }
      for(const key of ["default","initialDefault"]) {
        const expected=structuredClone(field[key]);if(expected.kind==="single_choice")expected.option=mapping.get(expected.option);
        if(expected.kind==="multi_choice")expected.options=expected.options.map(id=>mapping.get(id));
        assert.ok(JSON.stringify(next[key])===JSON.stringify(expected),"default owner and reference remapping differs");
      }
      assert.ok(JSON.stringify(next.optionOrder)===JSON.stringify(field.optionOrder.map(id=>mapping.get(id))),"Option order mapping differs");
    }
    assert.ok(JSON.stringify(copy.content.fieldOrder)===JSON.stringify(source.content.fieldOrder.map(id=>mapping.get(id))),"Field order mapping differs");
    return [...mapping.entries()];
  };
  const mapping1=compare(original,first);
  await controller.navigate({kind:"delete"});await controller.navigate({kind:"cancel"});assert.equal(writeIds.size,1);
  await controller.navigate({kind:"delete"});await controller.executeTemplateAction();assert.equal(controller.snapshot().selection.content.lifecycle,"Deleted");
  await controller.navigate({kind:"duplicate"});await controller.executeTemplateAction();const second=controller.snapshot().selection;
  const mapping2=compare(first,second);assert.equal(second.content.lifecycle,"Active");assert.equal(writeIds.size,3);
  await controller.navigate({kind:"select",id:originalId});await controller.navigate({kind:"delete"});await controller.executeTemplateAction();
  assert.ok(controller.snapshot().templateAction.result.deletion.reason==="template_has_documents"&&controller.snapshot().templateAction.result.deletion.count===1,"actual reference summary required");
  assert.equal(writeIds.size,4);assert.equal(controller.snapshot().retainedRefs.length,1);
  await controller.abandon(controller.snapshot().retainedRefs[0].id);await controller.navigate({kind:"cancel"});
  assert.ok(originalBytes.equals(fs.readFileSync(templatePath(originalId)))&&documentBytes.equals(fs.readFileSync(documentPath)),"original and Document bytes must remain unchanged");
  await controller.navigate({kind:"close_project"});await rpc({awaitShutdown:true});await controller.checkStatus();await controller.acknowledgeShutdown();await controller.checkStatus();await controller.retireProject();
  controller.setRoot(process.env.WB_UI_PROJECT);await controller.open();await controller.navigate({kind:"select",id:first.content.id});assert.equal(controller.snapshot().selection.content.lifecycle,"Deleted");
  await controller.navigate({kind:"select",id:second.content.id});assert.equal(controller.snapshot().selection.content.lifecycle,"Active");
  assert.ok(originalBytes.equals(fs.readFileSync(templatePath(originalId)))&&documentBytes.equals(fs.readFileSync(documentPath)),"reopen must preserve original bytes");
  await rpc({nativeClose:true});await controller.checkStatus();await rpc({awaitShutdown:true});await controller.checkStatus();await controller.acknowledgeShutdown();
  process.stdout.write(JSON.stringify({done:true,summary:{id:second.content.id,name:second.content.name,first:first.content.id,original:originalId,mapping1,mapping2,writes:writeIds.size,reopened:true}})+"\n");clearTimeout(deadline);lines.close();
}
main().catch((error) => { console.error("controller integration assertion failed; private values omitted"); console.error(String(error?.stack ?? "").split("\n").filter(line=>line.trimStart().startsWith("at ")).join("\n")); process.exit(1); });
