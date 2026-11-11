// Tests for uq/pages/crumbs: logic.js, page.toml and the files around them.
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

// ---- one test per fixture: it parses, and the plain words come out ----------------------------

test("fixture scan: user categories, plain labels, skipped and unavailable apart", () => {
  const rows = L.categories(fx("scan").data);
  assert.equal(rows.length, 13);
  const by = Object.fromEntries(rows.map((r) => [r.category, r]));
  assert.equal(by.cache.label, "App caches");
  assert.equal(by.cache.items, 2);
  assert.equal(by.cache.sizeText, "1.5 GB");
  assert.equal(by.cache.itemsText, "2 items");
  assert.equal(by.cache.command, "clean");
  assert.equal(by.cache.how, "Goes to the trash");
  assert.equal(by.thumbnails.itemsText, "1 item");
  assert.equal(by.temp.command, "clean-for-good");
  assert.equal(by.temp.how, "Deleted for good");
  assert.equal(by["dev-cache"].skipped, true);
  assert.equal(by.containers.unavailable, "podman and docker not found");
  const view = L.spaceView(ok("scan"));
  assert.equal(view.problem, "");
  assert.deepEqual(view.rows.map((r) => r.category), [
    "cache", "thumbnails", "trash", "temp", "browser-cache", "flatpak-unused", "appimage-old", "logs", "aur-build", "app-cache",
  ]);
  assert.equal(view.unavailable, "Not on this computer: Unused container images and volumes");
  assert.equal(view.switchedOff, "Switched off in crumBS: Developer caches, Build folders in your projects");
});

test("fixture scan-system: system rows, opt-in note, unknown sizes", () => {
  const view = L.spaceView(ok("scan-system"));
  const by = Object.fromEntries(view.rows.map((r) => [r.category, r]));
  assert.deepEqual(Object.keys(by), ["pkg-cache", "orphan-pkgs", "journal", "coredumps", "system-temp"]);
  assert.equal(by.journal.command, "clean-system");
  assert.equal(by.journal.how, "Needs your password and can't be undone");
  assert.equal(by["orphan-pkgs"].how, "Needs your password and can't be undone · Only when you ask");
  assert.equal(by["system-temp"].sizeText, "Size unknown");
  assert.equal(by.journal.sizeText, "229.7 MB");
  assert.equal(view.unavailable, "Not on this computer: Unused Flatpak runtimes (whole system), Old system snapshots");
});

test("fixture orphans: path, size and reason, all cleanable", () => {
  const rows = L.orphansList(fx("orphans").data);
  assert.equal(rows.length, 2);
  assert.equal(rows[0].path, "/home/user/.thunderbird");
  assert.equal(rows[0].sizeText, "12.3 kB");
  assert.match(rows[0].reason, /^looks like thunderbird data/);
  assert.ok(rows.every((r) => r.valid));
});

test("fixture history: batches, plain titles, what can be put back", () => {
  const rows = L.batches(fx("history").data);
  assert.deepEqual(rows.map((r) => r.title), ["Put back", "Removed oldtool", "Cleanup", "Leftovers moved to the trash"]);
  assert.equal(rows[0].time, "2026-11-08 12:43 UTC");
  assert.equal(rows[2].sizeText, "1.4 GB");
  assert.equal(rows[2].itemsText, "2 items");
  // the clean batch has a trashed entry nothing restored; the orphans batch was put back by the
  // first (restore) batch; a restore batch and a package removal have nothing to put back
  assert.deepEqual(rows.map((r) => r.canRestore), [false, false, true, false]);
});

test("a failed restore does not count as put back", () => {
  const data = fx("history").data;
  data.batches[0].entries[0].ok = false;
  assert.equal(L.batches(data)[3].canRestore, true);
});

test("fixture apps: names, sizes, sources and Part of uQ", () => {
  const rows = L.apps(fx("apps").data);
  assert.equal(rows.length, 6);
  assert.equal(rows[0].name, "Firefox Web Browser");
  assert.equal(rows[0].sizeText, "317.0 MB");
  assert.equal(rows[0].sourceText, "Package");
  assert.equal(rows[0].canUninstall, true);
  assert.equal(rows[2].sourceText, "Flatpak");
  assert.equal(rows[5].sizeText, "—");
  assert.equal(rows[5].sourceText, "Installed by hand");
});

test("desktop_package_has_no_uninstall", () => {
  const rows = L.apps(fx("apps").data);
  const uq = rows.find((r) => r.id === "pacman:uq");
  const scdr = rows.find((r) => r.id === "pacman:scdr");
  for (const r of [uq, scdr]) {
    assert.equal(r.partOf, "uQ");
    assert.equal(r.canUninstall, false);
  }
  assert.equal(rows.filter((r) => r.partOf === null && r.canUninstall).length, 4);
});

test("fixture clean-ok: freed bytes in plain words", () => {
  const o = L.outcome(ok("clean-ok"));
  assert.equal(o.message, "Freed 1.2 GB");
  assert.equal(o.failed, false);
  assert.deepEqual(o.manual, []);
});

test("a clean with nothing chosen says so", () => {
  const env = fx("clean-ok");
  env.data.report = [];
  assert.equal(L.outcome({ code: 0, output: env }).message, "Nothing to clean up");
});

test("exit5_shows_manual_commands", () => {
  const result = { code: 5, error: "exit 5: administrator rights were not granted", output: fx("clean-exit5") };
  const o = L.outcome(result);
  assert.equal(o.message, "Couldn't get administrator rights");
  assert.equal(o.failed, true);
  assert.deepEqual(o.manual, ["sudo '/usr/bin/journalctl' '--vacuum-size=500M'"]);
  const env = L.envelope(result);
  assert.equal(env.code, 5);
  assert.equal(env.cli, "administrator rights were not granted; nothing that needs them was changed");
});

test("fixture uninstall-refused-desktop: crumBS's refusal, as it is", () => {
  const result = { code: 1, error: "exit 1: refusing", output: fx("uninstall-refused-desktop") };
  const o = L.outcome(result);
  assert.match(o.message, /^refusing: pacman:uq is part of the uQ desktop; re-run with --desktop to remove it/);
  assert.equal(o.failed, true);
});

test("fixture uninstall-running: the refusal verbatim", () => {
  const result = { code: 1, error: "exit 1: Firefox Web Browser is running", output: fx("uninstall-running") };
  assert.equal(L.outcome(result).message, "Firefox Web Browser is running (pid 4242); close it first or re-run with --force");
});

// ---- outcome and envelope --------------------------------------------------------------------

test("outcome words for every exit code", () => {
  const env = (extra) => ({ code: 3, output: { schema_version: 1, command: "clean", data: extra, errors: [] } });
  const partial = L.outcome(env({ plans: [], report: [{ freed: 5, results: [{ ok: false, error: "busy", action: { path: "/x/y", type: "trash" } }] }] }));
  assert.equal(partial.message, "Some items couldn't be removed");
  assert.deepEqual(partial.detail, ["/x/y: busy"]);
  assert.equal(L.outcome({ code: 4, output: fx("clean-ok") }).message, "Cancelled at a prompt");
  assert.equal(L.outcome({ code: 2, error: "exit 2: x", output: { data: null, errors: [{ source: "cli", message: "--json needs --yes" }] } }).message, "--json needs --yes");
  assert.equal(L.outcome({ error: "timed out" }).message, "timed out");
  assert.equal(L.outcome({ code: 1 }).message, "Something went wrong");
  const restored = { code: 0, output: { data: { outcomes: [{ entry_id: "1-0", ok: true, error: null, path: "/a" }, { entry_id: "", ok: false, error: "no matching history entry", path: "x" }] }, errors: [] } };
  assert.equal(L.outcome(restored).message, "Put back 1 item");
});

test("a cancelled run says nothing", () => {
  const o = L.outcome({ error: "Cancelled", cancelled: true });
  assert.equal(o.message, "");
  assert.equal(L.isCancelled({ cancelled: true }), true);
  assert.equal(L.isCancelled({ code: 1 }), false);
});

test("envelope reads the output for every code and never throws on junk", () => {
  const e = L.envelope({ code: 1, error: "x", output: fx("uninstall-running") });
  assert.equal(e.ok, false);
  assert.equal(e.code, 1);
  assert.equal(e.errors.length, 1);
  assert.equal(e.errors[0].source, "cli");
  assert.equal(L.envelope(ok("scan")).ok, true);
  for (const junk of [null, undefined, 5, "text", [], { output: "x" }, { code: 0, output: [1] }, { code: 0, output: { data: 5, errors: "x" } }]) {
    const j = L.envelope(junk);
    assert.equal(j.data, null);
    assert.deepEqual(j.errors, []);
    assert.equal(j.cli, null);
  }
  assert.equal(L.spaceView({ code: 0, output: { data: { categories: "nope" } } }).problem, "crumBS gave an answer this page can't read.");
  assert.equal(L.spaceView({ error: "timed out" }).problem, "timed out");
});

test("untrusted text is plain and cut", () => {
  const long = "x".repeat(5000);
  const data = { orphans: [{ path: "/a/" + long, reason: "r\nline", size: { bytes: 1 } }, { path: "/b‮<i>x</i>", reason: "", size: { bytes: 5 } }, 7, null] };
  const rows = L.orphansList(data);
  assert.equal(rows.length, 2);
  assert.equal(rows[0].shown.length, 200);
  assert.equal(rows[0].path.length, 5003);
  assert.equal(rows[0].valid, false, "over 4096 bytes cannot be passed on");
  assert.equal(rows[0].reason, "r line");
  assert.equal(rows[1].shown, "/b‮<i>x</i>");
  const many = { apps: Array.from({ length: 900 }, (_, i) => ({ id: `pacman:p${i}`, name: `p${i}`, source: "pacman" })) };
  assert.equal(L.apps(many).length, 500);
  assert.equal(L.categories({ categories: Array.from({ length: 900 }, () => ({ category: "cache" })) }).length, 500);
});

test("size", () => {
  assert.equal(L.size(null), "—");
  assert.equal(L.size(undefined), "—");
  assert.equal(L.size("5"), "—");
  assert.equal(L.size(-1), "—");
  assert.equal(L.size(0), "0 B");
  assert.equal(L.size(999), "999 B");
  assert.equal(L.size(1000), "1.0 kB");
  assert.equal(L.size(1234567890), "1.2 GB");
  assert.equal(L.size(999950), "1.0 MB");
  assert.equal(L.size(4096), "4.1 kB");
});

test("filter matches the name, ignoring case", () => {
  const rows = L.apps(fx("apps").data);
  assert.deepEqual(L.filterApps(rows, "FIRE", 200).rows.map((r) => r.id), ["pacman:firefox"]);
  assert.equal(L.filterApps(rows, "", 200).rows.length, 6);
  assert.equal(L.filterApps(rows, "  ", 200).rows.length, 6);
  assert.deepEqual(L.filterApps(rows, "zzz", 200), { rows: [], more: false });
  assert.deepEqual(L.filterApps(rows, "", 2).more, true);
  assert.equal(L.filterApps(rows, "pacman", 200).rows.length, 0, "the id is not searched");
});

test("notes are copied, not changed in place", () => {
  const a = { x: 1 };
  const b = L.withNote(a, "y", 2);
  assert.deepEqual(a, { x: 1 });
  assert.deepEqual(b, { x: 1, y: 2 });
});

// ---- categories and commands -----------------------------------------------------------------

test("category_lists_partition", () => {
  const doc = readFileSync(join(repo, "docs", "json-schema.md"), "utf8");
  const names = (re) => {
    const m = re.exec(doc);
    assert.ok(m, `docs/json-schema.md lost the sentence ${re}`);
    return [...m[1].matchAll(/`([a-z-]+)`/g)].map((x) => x[1]);
  };
  const user = names(/The thirteen user categories: ([^\n]*?)\. The seven system categories/);
  const system = names(/The seven system categories, after them: ([^\n]*?)\.\n/);
  assert.equal(user.length, 13);
  assert.equal(system.length, 7);
  const lists = [L.USER_TRASH, L.USER_FOR_GOOD, L.SYSTEM];
  const all = lists.flat();
  assert.equal(new Set(all).size, all.length, "the three lists overlap");
  assert.deepEqual([...all].sort(), [...user, ...system].sort(), "a documented category is missing from the page, or the page has an extra one");
  assert.deepEqual([...L.USER_TRASH, ...L.USER_FOR_GOOD].sort(), [...user].sort());
  assert.deepEqual([...L.SYSTEM].sort(), [...system].sort());
  for (const name of all) assert.ok(Object.hasOwn(L.LABELS, name), `no plain label for ${name}`);
  assert.equal(Object.keys(L.LABELS).length, 20);
  assert.ok(L.OPT_IN_SYSTEM.every((n) => L.SYSTEM.includes(n)));
});

test("cleanCommandFor routes by what a cleanup does", () => {
  for (const c of L.USER_TRASH) assert.equal(L.cleanCommandFor(c), "clean");
  for (const c of L.USER_FOR_GOOD) assert.equal(L.cleanCommandFor(c), "clean-for-good");
  for (const c of L.SYSTEM) assert.equal(L.cleanCommandFor(c), "clean-system");
  for (const c of ["", "nope", "--force", "constructor", "__proto__", "CACHE", null, undefined, 5, ["cache"]]) assert.equal(L.cleanCommandFor(c), null);
  const odd = L.categories({ categories: [{ category: "new-thing", items: [{}], total: 5 }] })[0];
  assert.equal(odd.command, null);
  assert.equal(odd.label, "new-thing");
});

test("no_dangerous_flags", () => {
  const banned = ["--force", "--desktop", "--permanent", "-i", "--desktop-id", "--interactive"];
  for (const c of manifest.command) {
    for (const flag of banned) assert.ok(!c.argv.includes(flag), `${c.id} argv holds ${flag}`);
    assert.equal(c.argv[0], "/usr/bin/crumbs");
    for (const a of c.args ?? []) for (const flag of banned) assert.ok(!new RegExp(a.pattern).test(flag), `${c.id}.${a.name} accepts ${flag}`);
  }
  for (const f of [...files(".qml"), ...files(".js")]) {
    assert.ok(!/["']--(force|desktop|permanent|interactive)["']|["']-i["']/.test(read(f)), `${f} names a dangerous flag`);
  }
  // the three changing commands that take a category can never be handed one of them
  assert.ok(L.argProblem("clean", "category", "--force") !== "");
});

// ---- the manifest ----------------------------------------------------------------------------

test("manifest: the page, its settings and its programs", () => {
  assert.equal(manifest.id, "crumbs");
  assert.equal(manifest.name, "Storage and cleanup");
  assert.equal(manifest.icon, "user-trash-full");
  assert.deepEqual(manifest.requires, { bins: ["crumbs"] });
  assert.equal(manifest.dbus, undefined);
  assert.deepEqual(manifest.setting.map((s) => [s.id, s.label]), [
    ["space", "Space you can free"],
    ["system-space", "System space"],
    ["leftovers", "Leftovers of removed apps"],
    ["history", "Recent cleanups"],
    ["apps", "Apps and their size"],
  ]);
  for (const s of manifest.setting) assert.ok(s.keywords?.length > 0, `${s.id} has no keywords`);
  assert.deepEqual(manifest.command.map((c) => c.id), [
    "scan", "scan-system", "orphans", "history", "apps", "clean", "clean-for-good", "clean-system", "orphan-trash", "restore", "uninstall",
  ]);
  const argvTail = (id) => command(id).argv.slice(1).join(" ");
  assert.equal(argvTail("scan"), "scan --json");
  assert.equal(argvTail("scan-system"), "scan --system --json");
  assert.equal(argvTail("clean"), "clean --yes --json");
  assert.equal(argvTail("clean-for-good"), "clean --yes --json");
  assert.equal(argvTail("clean-system"), "clean --system --yes --json");
  assert.equal(argvTail("orphans"), "orphans --json");
  assert.equal(command("orphan-trash").argv.at(-1), "--select", "--select is the last fixed argument");
  assert.equal(argvTail("orphan-trash"), "orphans --clean --yes --json --select");
  assert.equal(argvTail("history"), "history --limit 20 --json");
  assert.equal(argvTail("restore"), "restore --json");
  assert.equal(argvTail("apps"), "apps list --sort size --json");
  assert.equal(argvTail("uninstall"), "uninstall --yes --json");
  assert.deepEqual(command("history").watch, { files: ["~/.local/state/crumbs/history.jsonl"] });
  for (const c of manifest.command) assert.equal(c.output, "json");
  const timeouts = Object.fromEntries(manifest.command.map((c) => [c.id, c.timeout_ms]));
  assert.deepEqual(timeouts, {
    scan: 180000, "scan-system": 180000, orphans: 120000, history: 30000, apps: 120000,
    clean: 600000, "clean-for-good": 600000, "clean-system": 600000, "orphan-trash": 300000, restore: 300000, uninstall: 600000,
  });
});

test("patterns_match_manifest", () => {
  const seen = new Set();
  for (const c of manifest.command) {
    for (const a of c.args ?? []) {
      seen.add(`${c.id}.${a.name}`);
      assert.equal(a.pattern, L.PATTERNS[c.id]?.[a.name], `${c.id}.${a.name}: logic.js and page.toml differ`);
    }
    assert.equal((c.args ?? []).length, Object.keys(L.PATTERNS[c.id] ?? {}).length, `${c.id}: argument count differs`);
  }
  assert.equal(seen.size, 6);
  assert.equal(command("clean").args[0].pattern, `^(${L.USER_TRASH.join("|")})$`);
  assert.equal(command("clean-for-good").args[0].pattern, `^(${L.USER_FOR_GOOD.join("|")})$`);
  assert.equal(command("clean-system").args[0].pattern, `^(${L.SYSTEM.join("|")})$`);
});

const HOSTILE = ["-rf", "--force", "a\nb", "", "x".repeat(5000), "../x", "$(id)", "/x\ny", "/x\u0000y", "/" + "x".repeat(5000), "-", "1\n", "12345678901234567890123"];
// What uQ enforces besides the pattern: never a leading "-", and at most 4096 bytes.
const uqAccepts = (pattern, v) => v !== "" && v[0] !== "-" && Buffer.byteLength(v) <= 4096 && new RegExp(pattern).test(v);

test("rejects_hostile_args", () => {
  for (const [id, args] of Object.entries(L.PATTERNS)) {
    for (const [name, pattern] of Object.entries(args)) {
      for (const bad of HOSTILE) {
        // "$(id)" is data where a pattern allows any text; no shell is ever involved
        const accepted = uqAccepts(pattern, bad);
        assert.equal(L.argProblem(id, name, bad) === "", accepted, `${id}.${name}: logic.js and the manifest disagree on ${JSON.stringify(bad.slice(0, 20))}`);
        if (["-rf", "--force", "a\nb", "", "x".repeat(5000), "../x", "-"].includes(bad)) assert.equal(accepted, false, `${id}.${name} accepts ${JSON.stringify(bad.slice(0, 20))}`);
      }
    }
  }
  assert.notEqual(L.argProblem("orphan-trash", "path", "/x\ny"), "");
  assert.equal(L.argProblem("orphan-trash", "path", "/home/u/.thunderbird"), "");
  assert.equal(L.argProblem("orphan-trash", "path", "/home/u/<i>‮"), "");
  assert.equal(L.argProblem("uninstall", "app", "pacman:firefox"), "");
  assert.equal(L.argProblem("uninstall", "app", "appimage:~/Applications/Foo-1.0.AppImage"), "");
  assert.notEqual(L.argProblem("uninstall", "app", "firefox"), "");
  assert.notEqual(L.argProblem("uninstall", "app", "pacman:"), "");
  assert.equal(L.argProblem("restore", "batch", "1794141850000"), "");
  assert.notEqual(L.argProblem("restore", "batch", "17x"), "");
  assert.notEqual(L.argProblem("nope", "x", "y"), "");
  assert.notEqual(L.argProblem("clean", "other", "cache"), "");
  assert.notEqual(L.argsProblem("clean", []), "");
  assert.notEqual(L.argsProblem("clean", ["cache", "logs"]), "");
  assert.equal(L.argsProblem("clean", ["cache"]), "");
  // a category can't be cleaned under the wrong question
  assert.notEqual(L.argsProblem("clean", ["trash"]), "");
  assert.notEqual(L.argsProblem("clean-for-good", ["cache"]), "");
  assert.notEqual(L.argsProblem("clean-system", ["cache"]), "");
});

test("confirms_match_plan", () => {
  const plan = {
    clean: "Clean up %s? Items go to the trash, and crumBS can put them back.",
    "clean-for-good": "Clean up %s? This deletes them for good.",
    "clean-system": "Clean up %s for the whole system? This needs your password and can't be undone.",
    "orphan-trash": "Move %s to the trash?",
    uninstall: "Uninstall %s and remove its leftovers? A removed package can't be put back by crumBS.",
    restore: "Put back the files from this cleanup?",
  };
  for (const c of manifest.command) assert.equal(c.confirm, plan[c.id], `${c.id}: confirm`);
  for (const id of Object.keys(plan)) assert.ok(command(id), `${id} is missing`);
  // every command with arguments that changes something asks; the reads take no arguments
  for (const c of manifest.command) assert.equal(Boolean(c.args), Boolean(c.confirm), `${c.id}: args and confirm go together`);
});

