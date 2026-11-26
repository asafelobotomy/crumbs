// Node tests for the Storage and cleanup page's uninstall, apps, preferences and pie chart
// (uninstall.js, apps.js, overview.js). The rest is in logic.test.mjs and page.test.mjs.
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

const dryRun = (actions, notes = []) => ({ code: 0, output: { schema_version: 1, command: "uninstall", data: { plans: [{ app_id: "pacman:x", actions, notes, total_size: 0 }], report: null }, errors: [] } });

test("removalPreview groups what goes, in order, and says what can come back", () => {
  const v = L.removalPreview(dryRun([
    { action: { type: "trash", path: "/home/u/.cache/x" }, size: 2000, group: "cache", reason: "" },
    { action: { type: "package-remove", names: ["x", "x-data"], source: "pacman" }, size: 0, group: "app", reason: "" },
    { action: { type: "trash", path: "/home/u/.config/x" }, size: 1000, group: "settings", reason: "" },
    { action: { type: "edit-file", path: "/home/u/.config/mimeapps.list", remove_lines: ["a"] }, size: 0, group: "launchers", reason: "" },
    { action: { type: "trash", path: "/elsewhere\nx" }, size: null, reason: "" },
  ], ["the app itself stays installed"]), "/home/u");
  assert.equal(v.problem, "");
  assert.deepEqual(v.groups.map((g) => g.key), ["app", "settings", "cache", "launchers", "other"]);
  assert.equal(v.groups[0].items[0], "Package x, x-data");
  assert.equal(v.groups[0].how, "Removed for good");
  assert.equal(v.groups[1].items[0], "~/.config/x");
  assert.equal(v.groups[1].how, "Goes to the trash");
  assert.equal(v.groups[3].how, "Goes to the trash", "an edited file keeps a backup");
  assert.equal(v.groups[4].items[0], "/elsewhere x", "control characters are cut");
  assert.equal(v.totalText, "Frees about 3.0 kB (some sizes unknown)");
  assert.deepEqual(v.notes, ["the app itself stays installed"]);
  assert.equal(L.removalPreview(dryRun([])).totalText, "Nothing to remove.");
  assert.equal(L.removalPreview({ code: 2, output: { data: null, errors: [{ source: "cli", message: "no such app" }] } }).problem, "no such app");
  assert.notEqual(L.removalPreview({ error: "timed out" }).problem, "");
  const many = L.removalPreview(dryRun(Array.from({ length: 80 }, (_, i) => ({ action: { type: "trash", path: `/p/${i}` }, size: 1, group: "data" }))));
  assert.equal(many.groups[0].items.length, 50);
  assert.equal(many.groups[0].more, 30);
  for (const mode of ["all", "keep-data", "leftovers", "nope"]) {
    const c = L.uninstallCommands(mode);
    assert.ok(command(c.preview) && command(c.run) && command(c.close), `${mode}: commands exist`);
    assert.ok(command(c.close).argv.includes("--close-running") && !command(c.run).argv.includes("--close-running"));
    assert.ok(command(c.preview).argv.includes("--dry-run") && !command(c.run).argv.includes("--dry-run"));
  }
  assert.equal(L.uninstallCommands("nope").run, "uninstall");
});

test("configView reads config show, and the category switches change the right lists", () => {
  const v = L.configView({ code: 0, output: { data: { path: "~/.config/crumbs/config.toml", keys: [], values: {
    protect: ["~/Games", 3], exclude: [], appimage_dirs: ["~/Apps"], project_dirs: [], categories_off: ["dev-cache", "cache"], categories_on: ["cache"],
    temp_age_days: 3, log_age_days: 14, build_age_days: 7, snapshots_keep: 3, pkg_cache_keep: 0, journal_max: "1G",
  } } } });
  assert.equal(v.problem, "");
  assert.deepEqual(v.numbers.map((n) => n.key), L.NUMBER_KEYS);
  assert.equal(v.numbers[0].value, 3);
  assert.equal(v.numbers[4].value, 0);
  assert.equal(v.numbers[5].value, "1G");
  assert.deepEqual(v.lists[0].entries, ["~/Games"]);
  assert.equal(L.categoryOn("dev-cache", v), false);
  assert.equal(L.categoryOn("cache", v), true, "categories_on wins");
  assert.equal(L.categoryOn("build-output", v), false, "build-output only when listed on");
  assert.equal(L.categoryOn("logs", v), true);
  assert.deepEqual(L.categorySteps("dev-cache", true, v), [["config-remove", ["categories_off", "dev-cache"]]]);
  assert.deepEqual(L.categorySteps("build-output", true, v), [["config-add", ["categories_on", "build-output"]]]);
  assert.deepEqual(L.categorySteps("cache", false, v), [["config-remove", ["categories_on", "cache"]]]);
  assert.deepEqual(L.categorySteps("logs", false, v), [["config-add", ["categories_off", "logs"]]]);
  for (const [cmd, args] of [...L.categorySteps("dev-cache", true, v), ...L.categorySteps("logs", false, v)]) assert.equal(L.argsProblem(cmd, args), "");
  assert.equal(L.argsProblem("config-set", ["journal_max", "500M"]), "");
  assert.equal(L.argsProblem("config-set", ["temp_age_days", "7"]), "");
  assert.notEqual(L.argsProblem("config-set", ["protect", "7"]), "");
  assert.notEqual(L.argsProblem("config-set", ["temp_age_days", "-1"]), "");
  assert.equal(L.argsProblem("config-add", ["protect", "~/Games"]), "");
  assert.notEqual(L.argsProblem("config-add", ["protect", "Games/x"]), "");
  assert.notEqual(L.argsProblem("config-add", ["temp_age_days", "~/x"]), "");
  assert.equal(L.argsProblem("orphan-delete-system", ["/opt/oldtool"]), "");
  assert.notEqual(L.argsProblem("orphan-delete-system", ["/opt/a/b"]), "");
  assert.notEqual(L.argsProblem("orphan-delete-system", ["/home/u/x"]), "");
  assert.notEqual(L.argsProblem("orphan-delete-system", ["/etc/x"]), "");
  assert.ok(L.configView({ error: "timed out" }).problem !== "");
  assert.deepEqual(L.userCategories().map((c) => c.category), [...L.USER_TRASH, ...L.USER_FOR_GOOD]);
});

test("apps carry their icon name", () => {
  const rows = L.apps({ apps: [{ id: "pacman:firefox", name: "firefox", display_name: "Firefox", icon: "firefox", source: "pacman" }, { id: "pacman:x", name: "x", icon: 7 }] });
  assert.equal(rows[0].icon, "firefox");
  assert.equal(rows[1].icon, "");
});

test("apps are sorted into menu categories, biggest first, empty groups left out", () => {
  const app = (name, category, bytes) => ({ id: `pacman:${name}`, name, source: "pacman", install_size: bytes, category });
  const rows = L.apps({ apps: [app("a", "Games", 10), app("b", "Internet", 5), app("c", "Games", 30), app("d", null, 1), app("e", "Nonsense", 2)] });
  assert.equal(rows[2].category, "Games");
  assert.equal(rows[3].category, "Other");
  assert.equal(rows[4].category, "Other", "a category this page doesn't know is Other");
  const groups = L.groupApps(rows);
  assert.deepEqual(groups.map((g) => g.key), ["Internet", "Games", "Other"]);
  assert.deepEqual(groups[1].rows.map((r) => r.name), ["c", "a"]);
  assert.equal(groups[1].countText, "2 apps");
  assert.equal(groups[1].total, 40);
  assert.equal(groups[2].title, "Other apps");
});

test("with no categories at all (an older crumbs) apps are sorted by source instead", () => {
  const rows = L.apps({ apps: [{ id: "flatpak:x", name: "x", source: "flatpak" }, { id: "pacman:y", name: "y", source: "pacman" }] });
  assert.deepEqual(L.groupApps(rows).map((g) => g.title), ["Flatpak", "Package"]);
});

test("a restore that put nothing back says why, not 'Something went wrong'", () => {
  const env = {
    schema_version: 1,
    command: "restore",
    data: { outcomes: [{ entry_id: "", error: "no matching history entry", ok: false, path: "999" }] },
    errors: [],
  };
  const o = L.outcome({ code: 2, output: env });
  assert.equal(o.failed, true);
  assert.equal(o.message, "Couldn't put this back: no matching history entry");
});

test("rows that may ask for a password say so while they work", () => {
  assert.equal(L.workingText("clean"), "Working…");
  assert.equal(L.workingText(""), "Working…");
  for (const id of ["uninstall", "uninstall-close", "clean-system", "orphan-delete-system"])
    assert.match(L.workingText(id), /^Working… If a password dialog opened, enter your password there\.$/);
  for (const id of L.NEEDS_PASSWORD) assert.ok(command(id), `${id} is a command`);
});

// ---- the uninstall dialog and the overview's pie ---------------------------------------------

const ran = (code, actions, results, extra = {}) => ({ code, output: { schema_version: 1, command: "uninstall", errors: extra.errors ?? [], data: {
  plans: [{ app_id: "pacman:x", actions, notes: [], total_size: 0 }],
  report: [{ batch: "1", freed: extra.freed ?? 3000, results }], ...(extra.data ?? {}) } } });

test("removalReport lists what went, grouped like the preview, and what stayed", () => {
  const actions = [
    { action: { type: "package-remove", names: ["x"], source: "pacman" }, size: 0, group: "app" },
    { action: { type: "trash", path: "/home/u/.cache/x" }, size: 2000, group: "cache" },
    { action: { type: "trash", path: "/home/u/.config/x" }, size: 1000, group: "settings" },
  ];
  const results = [
    { action: actions[0].action, ok: true, error: null },
    { action: actions[1].action, ok: true, error: null },
    { action: actions[2].action, ok: false, error: "busy\nnow" },
  ];
  const r = L.removalReport(ran(3, actions, results), "/home/u");
  assert.equal(r.status, "partial");
  assert.equal(r.failed, true);
  assert.deepEqual(r.groups.map((g) => [g.title, g.items]), [["The app itself", ["Package x"]], ["Caches", ["~/.cache/x"]]]);
  assert.deepEqual(r.failures, ["~/.config/x: busy now"]);
  assert.equal(r.message, "Some items couldn't be removed · freed 3.0 kB");
  const all = L.removalReport(ran(0, actions.slice(0, 2), results.slice(0, 2)), "");
  assert.equal(all.status, "removed");
  assert.equal(all.message, "Removed · freed 3.0 kB");
  assert.equal(L.removalReport(ran(0, [], [])).message, "Nothing needed removing.");
});

test("removalReport: a no, an open app, a refusal and no password each say so", () => {
  const no = L.removalReport({ error: "Cancelled", cancelled: true });
  assert.equal(no.status, "cancelled");
  assert.equal(no.failed, false);
  const open = L.removalReport({ code: 1, output: fx("uninstall-running") });
  assert.equal(open.status, "refused");
  assert.equal(open.running, true);
  assert.ok(!/--force/.test(open.message));
  const refused = L.removalReport({ code: 2, output: fx("uninstall-refused-desktop") });
  assert.equal(refused.status, "refused");
  assert.equal(refused.running, false);
  const five = L.removalReport({ code: 5, output: { data: { plans: [], report: null, manual_commands: ["sudo 'pacman' '-Rn' 'x'"] }, errors: [] } });
  assert.equal(five.status, "failed");
  assert.equal(five.message, "Couldn't get administrator rights");
  assert.deepEqual(five.manual, ["sudo 'pacman' '-Rn' 'x'"]);
  assert.equal(L.removalReport(null).failed, true, "junk never throws, and says it failed");
});

test("queueSummary counts the apps that went and the space freed", () => {
  const gone = { status: "removed", freed: 1000 };
  const part = { status: "partial", freed: 500 };
  const no = { status: "cancelled", freed: 0 };
  assert.equal(L.queueSummary([gone, part, no, null]), "Uninstalled 2 of 4 apps · freed 1.5 kB");
  assert.equal(L.queueSummary([gone]), "Done · freed 1.0 kB");
  assert.equal(L.queueSummary([no]), "Nothing was uninstalled");
});

test("pieSlices: biggest first, the rest in Everything else, angles add up", () => {
  const rows = L.spaceView(ok("scan")).rows;
  const s = L.pieSlices(rows, false);
  assert.equal(s.length, 7);
  assert.deepEqual(s.slice(0, 3).map((x) => x.label), ["App caches", "Browser caches", "Caches of web-based apps"]);
  assert.equal(s[6].label, "Everything else");
  assert.equal(s[6].color, L.PIE_COLORS.otherLight);
  assert.equal(s[0].color, L.PIE_COLORS.light[0]);
  assert.equal(L.pieSlices(rows, true)[0].color, L.PIE_COLORS.dark[0]);
  assert.equal(s[0].start, 0);
  assert.ok(Math.abs(s[6].end - 2 * Math.PI) < 1e-9);
  assert.equal(s[0].percentText, "56%");
  const few = L.pieSlices([{ label: "A", total: 3 }, { label: "B", total: 1 }, { label: "C", total: 0 }], false);
  assert.deepEqual(few.map((x) => [x.label, x.percentText]), [["A", "75%"], ["B", "25%"]]);
  assert.deepEqual(L.pieSlices([], false), []);
  assert.deepEqual(L.pieSlices(null, false), []);
});

test("sliceAt finds the slice under the pointer, only on the ring", () => {
  const s = L.pieSlices([{ label: "A", total: 3 }, { label: "B", total: 1 }], false);
  assert.equal(L.sliceAt(s, 100, 10, 100, 100, 50, 100), 0, "straight up is the first slice");
  assert.equal(L.sliceAt(s, 20, 100, 100, 100, 50, 100), 1, "the last quarter, to the left");
  assert.equal(L.sliceAt(s, 100, 100, 100, 100, 50, 100), -1, "the hole in the middle");
  assert.equal(L.isLightColor("#f0f0f0"), true);
  assert.equal(L.isLightColor("#202020"), false);
  assert.equal(L.isLightColor(undefined), false);
});
