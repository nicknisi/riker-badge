import test from "node:test";
import assert from "node:assert/strict";
import vm from "node:vm";
import { DRAFT_SCRIPT } from "../src/draft_view.ts";
function fixture() {
  const elements = new Map<string, any>();
  const element = () => ({
    value: "",
    disabled: false,
    checked: false,
    children: [] as any[],
    textContent: "",
    append(...v: any[]) {
      this.children.push(...v);
    },
    replaceChildren() {
      this.children = [];
    },
    setAttribute() {},
    contains() {
      return false;
    },
  });
  const $ = (id: string) => {
    if (!elements.has(id)) elements.set(id, element());
    return elements.get(id);
  };
  const calls: any[] = [];
  const ctx = vm.createContext({
    $,
    text: (id: string, value: string) => {
      $(id).textContent = value;
    },
    id: "recording",
    document: { createElement: element },
    window: {},
    navigator: {},
    setTimeout,
    clearTimeout,
    api: (path: string, options: any) =>
      new Promise((resolve, reject) =>
        calls.push({ path, body: JSON.parse(options.body), resolve, reject }),
      ),
  });
  vm.runInContext(DRAFT_SCRIPT, ctx);
  const run = (source: string) => vm.runInContext(source, ctx);
  run(
    `draftRows=['A','B'].map(id=>({id,created_at:1,revision:0,payload:JSON.stringify([{angle:'One',text:id+' one'},{angle:'Two',text:id+' two'},{angle:'Three',text:id+' three'}])}));showDrafts(draftRows[0]);`,
  );
  return { $, calls, run };
}
const tick = () => new Promise((resolve) => setImmediate(resolve));
test("draft saves capture generation and serialize revisions rather than racing stale snapshots", async () => {
  const f = fixture();
  f.run("draftEditors[0].value='first edit'");
  const first = f.run("saveEdits()");
  await tick();
  f.run("draftEditors[0].value='second edit'");
  const second = f.run("saveEdits()");
  f.run("showDrafts(draftRows[1])");
  assert.equal(f.calls.length, 1);
  assert.equal(f.calls[0].body.revision, 0);
  f.calls[0].resolve({ revision: 1 });
  await first;
  await tick();
  assert.equal(f.calls[1].path, "/api/recordings/recording/drafts/A");
  assert.equal(f.calls[1].body.revision, 1);
  assert.equal(f.calls[1].body.texts[0], "second edit");
  f.calls[1].resolve({ revision: 2 });
  await second;
  assert.equal(
    f.run("JSON.parse(draftRows[0].payload)[0].text"),
    "second edit",
  );
});
test("history switching waits for save and preserves edits; conflicts keep the current editor", async () => {
  const f = fixture();
  f.run("draftEditors[0].value='must not disappear'");
  f.$("draftHistory").value = "B";
  const change = f.$("draftHistory").onchange();
  await tick();
  assert.equal(f.run("editSession.id"), "A");
  assert.equal(f.run("draftEditors[0].disabled"), true);
  f.calls[0].resolve({ revision: 1 });
  await change;
  assert.equal(f.run("editSession.id"), "B");
  f.$("draftHistory").value = "A";
  await f.$("draftHistory").onchange();
  assert.equal(f.run("draftEditors[0].value"), "must not disappear");
  f.run("draftEditors[0].value='conflicting local edit'");
  f.$("draftHistory").value = "B";
  const conflict = f.$("draftHistory").onchange();
  await tick();
  f.calls[1].reject(Error("Changed elsewhere"));
  await conflict;
  assert.equal(f.run("editSession.id"), "A");
  assert.equal(f.run("draftEditors[0].value"), "conflicting local edit");
  assert.equal(f.run("draftEditors[0].disabled"), false);
});
test("credential patterns disable export without forbidding manually verified links or mentions", () => {
  const f = fixture();
  f.run(
    "draftEditors[0].value='cfast_fake-key';draftEditors[0].oninput();clearTimeout(saveTimer)",
  );
  assert.equal(
    f.run("$('draftOptions').children[0].children[3].children[1].disabled"),
    true,
  );
  f.run(
    "draftEditors[0].value='Verified @speaker https://example.com';draftEditors[0].oninput();clearTimeout(saveTimer)",
  );
  assert.equal(
    f.run("$('draftOptions').children[0].children[3].children[1].disabled"),
    false,
  );
});
