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
// logic.js holds the shared helpers and patterns; apps.js, uninstall.js and overview.js were split
// from it and import it. MODULES is each by its QML import name; L is all of them, for the tests.
const JS = ["logic.js", "apps.js", "uninstall.js", "overview.js"];
const MODULES = Object.fromEntries(["Logic", "Apps", "Uninstall", "Overview"].map((name, i) => [name, loadLogic(join(pageDir, JS[i]))]));
const L = Object.assign({}, ...Object.values(MODULES));
const manifest = parseToml(readFileSync(join(pageDir, "page.toml"), "utf8"));
const fx = (name) => JSON.parse(readFileSync(join(here, "fixtures", `${name}.json`), "utf8"));
const ok = (name) => ({ code: 0, output: fx(name) });
const files = (ext) => readdirSync(pageDir).filter((f) => f.endsWith(ext));
const read = (f) => readFileSync(join(pageDir, f), "utf8");
const command = (id) => manifest.command.find((c) => c.id === id);

test("StorageHarness.qml lists the commands that confirm", () => {
  const plan = manifest.command.filter((c) => c.confirm).map((c) => c.id).sort();
  const test = readFileSync(join(here, "StorageHarness.qml"), "utf8");
  const listed = /confirmed:\s*\[([^\]]*)\]/.exec(test);
  assert.ok(listed, "StorageHarness.qml does not list the commands that confirm");
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
  // the sentences logic.js and the files split from it can hand to the page
  for (const line of JS.flatMap((f) => read(f).split("\n"))) {
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
    "AppTile.qml", "AppsSection.qml", "CategoryRow.qml", "HistorySection.qml", "LeftoversSection.qml", "ModeSidebar.qml", "Outcome.qml",
    "Page.qml", "PreferencesSection.qml", "RemovalResult.qml", "SpacePie.qml", "SpaceSection.qml", "SystemLeftovers.qml", "SystemSection.qml",
    "UninstallDialog.qml",
    "UninstallPreview.qml", "apps.js", "logic.js", "overview.js", "page.toml", "uninstall.js",
  ]);
});

test("the QML files use only what the .js files and page.toml provide", () => {
  const ids = new Set(manifest.command.map((c) => c.id));
  for (const f of files(".qml")) {
    const text = read(f);
    for (const m of text.matchAll(/\b(Logic|Apps|Uninstall|Overview)\.([A-Za-z_]+)/g)) assert.ok(m[2] in MODULES[m[1]], `${f}: ${m[1]}.${m[2]} does not exist`);
    for (const m of text.matchAll(/(?:\.write\([^,]+,\s*|command\.(?:run|watch)\()"([a-z-]+)"/g)) assert.ok(ids.has(m[1]), `${f}: no command ${m[1]}`);
    assert.ok(!/\bText\s*\{|\bLabel\s*\{|Qt\.createQmlObject/.test(text.replace(/UqLabel\s*\{/g, "")), `${f}: stock text item`);
  }
});

test("only Page.qml's sections read on open", () => {
  const page = read("Page.qml");
  assert.match(page, /space\.refresh\(\)/);
  assert.match(page, /recent\.start\(\)/);
  assert.equal((page.match(/\.refresh\(\)|\.start\(\)/g) ?? []).length, 2);
  // uQ calls a top-level refresh() when it shows a cached page again
  assert.match(page, /^    function refresh\(\) \{/m);
});


test("the page is laid out like Settings' own pages", () => {
  const page = read("Page.qml");
  assert.match(page, /^SettingsPage \{/m);
  assert.deepEqual([...page.matchAll(/SettingsTab \{\s*title: "([^"]+)"/g)].map((m) => m[1]), ["Overview", "Apps", "Leftovers", "History", "Preferences"]);
  assert.match(page, /pageId: "crumbs"/);
  // every [[setting]] row exists, so search can open its tab
  const all = files(".qml").map(read).join("\n");
  for (const s of manifest.setting) assert.match(all, new RegExp(`settingId: "${s.id}"`), `${s.id} has no row`);
  // a long read shows that it is working
  for (const f of ["SpaceSection.qml", "SystemSection.qml", "AppsSection.qml", "LeftoversSection.qml", "SystemLeftovers.qml", "UninstallPreview.qml"])
    assert.match(read(f), /ProgressBar \{\s*indeterminate: true/, `${f} has no progress bar`);
  assert.match(read("AppsSection.qml"), /UqAppIcon \{/);
});
