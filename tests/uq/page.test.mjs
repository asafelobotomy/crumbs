// Tests for the QML side of uq/pages/crumbs: its text, its files, and its test.
//   node --test 'tests/uq/*.test.mjs'
import assert from "node:assert/strict";
import { readFileSync, readdirSync } from "node:fs";
import { dirname, join } from "node:path";
import test from "node:test";
import { fileURLToPath } from "node:url";

import { loadLogic } from "./load-logic.mjs";
import { parseToml } from "./toml-lite.mjs";

const here = dirname(fileURLToPath(import.meta.url));
const repo = join(here, "..", "..");
const pageDir = join(repo, "uq", "pages", "crumbs");
const L = loadLogic(join(pageDir, "logic.js"));
const manifest = parseToml(readFileSync(join(pageDir, "page.toml"), "utf8"));
const fx = (name) => JSON.parse(readFileSync(join(here, "fixtures", `${name}.json`), "utf8"));
const ok = (name) => ({ code: 0, output: fx(name) });
const files = (ext) => readdirSync(pageDir).filter((f) => f.endsWith(ext));
const read = (f) => readFileSync(join(pageDir, f), "utf8");
const command = (id) => manifest.command.find((c) => c.id === id);

test("tst_page.qml lists the commands that confirm", () => {
  const plan = manifest.command.filter((c) => c.confirm).map((c) => c.id).sort();
  const test = readFileSync(join(here, "tst_page.qml"), "utf8");
  const listed = /confirmed:\s*\[([^\]]*)\]/.exec(test);
  assert.ok(listed, "tst_page.qml does not list the commands that confirm");
  assert.deepEqual([...listed[1].matchAll(/"([a-z-]+)"/g)].map((m) => m[1]).sort(), plan);
});

// ---- the page's text and files ---------------------------------------------------------------

const BANNED = /D-?Bus|polkit|JSON|systemctl|scdrd|xzramd|notifd|tailscaled|sched_ext|EEVDF|zram-generator|sysctl|\borg\.[a-z0-9_]+\.[A-Za-z0-9_.]+/i;

function literals(line) {
  return [...line.matchAll(/"((?:[^"\\]|\\.)*)"/g)].map((m) => m[1]);
}

test("page_strings_are_plain", () => {
  const found = [];
  for (const f of files(".qml")) {
    for (const line of read(f).split("\n")) {
      if (/\btext:|placeholderText:|title:|detail:|confirm/.test(line)) found.push(...literals(line));
    }
  }
  // the sentences logic.js can hand to the page
  for (const line of read("logic.js").split("\n")) {
    if (/^\s*\/\//.test(line) || /PATTERNS|pattern|RegExp|\.test\(|\.exec\(/.test(line)) continue;
    found.push(...literals(line).filter((s) => /^[A-Z].* /.test(s) || /…/.test(s)));
  }
  for (const key of ["name"]) found.push(manifest[key]);
  for (const s of manifest.setting) found.push(s.label);
  for (const c of manifest.command) if (c.confirm) found.push(c.confirm);
  assert.ok(found.length > 40, `only ${found.length} strings found`);
  for (const s of found) assert.ok(!BANNED.test(s), `jargon in: ${s}`);
  // product names are fine; the plain labels carry none of the category ids
  for (const label of Object.values(L.LABELS)) assert.ok(!/[a-z]-[a-z]/.test(label.replace(/Flatpak|web-based/g, "")), `label with a raw name: ${label}`);
  // messages of every exit code, too
  for (const code of [0, 1, 2, 3, 4, 5]) assert.ok(!BANNED.test(L.outcome({ code, output: fx("clean-ok") }).message));
});

test("every QML file stays under 400 lines, and the page folder holds what it should", () => {
  for (const f of files(".qml")) assert.ok(read(f).split("\n").length <= 400, `${f} is over 400 lines`);
  assert.deepEqual(readdirSync(pageDir).sort(), [
    "AppsSection.qml", "CategoryRow.qml", "HistorySection.qml", "LeftoversSection.qml", "Outcome.qml",
    "Page.qml", "SpaceSection.qml", "SystemSection.qml", "logic.js", "page.toml",
  ]);
});

test("the QML files use only what logic.js and page.toml provide", () => {
  const exported = new Set(Object.keys(L));
  const ids = new Set(manifest.command.map((c) => c.id));
  for (const f of files(".qml")) {
    const text = read(f);
    for (const m of text.matchAll(/\bLogic\.([A-Za-z_]+)/g)) assert.ok(exported.has(m[1]), `${f}: Logic.${m[1]} does not exist`);
    for (const m of text.matchAll(/(?:\.write\([^,]+,\s*|command\.(?:run|watch)\()"([a-z-]+)"/g)) assert.ok(ids.has(m[1]), `${f}: no command ${m[1]}`);
    assert.ok(!/\bText\s*\{|\bLabel\s*\{|Qt\.createQmlObject/.test(text.replace(/UqLabel\s*\{/g, "")), `${f}: stock text item`);
  }
});

test("only Page.qml's sections read on open", () => {
  const page = read("Page.qml");
  assert.match(page, /space\.refresh\(\)/);
  assert.match(page, /recent\.start\(\)/);
  assert.equal((page.match(/\.refresh\(\)|\.start\(\)/g) ?? []).length, 2);
});

