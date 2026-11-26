.pragma library
// The Storage and cleanup page's pure logic: no Qt calls, so Node can test it
// (tests/uq/logic.test.mjs). Everything crumbs prints is untrusted: it is read defensively, a
// wrong type becomes an empty or "couldn't read" result, lists are capped and strings are cut.

var USER_TRASH = ["cache", "thumbnails", "browser-cache", "dev-cache", "appimage-old", "logs", "app-cache", "build-output"];
var USER_FOR_GOOD = ["trash", "temp", "aur-build", "containers", "flatpak-unused"];
var SYSTEM = ["pkg-cache", "orphan-pkgs", "journal", "coredumps", "system-temp", "flatpak-unused-system", "snapshots"];
/// System categories that a plain system cleanup skips: they run only when named.
var OPT_IN_SYSTEM = ["orphan-pkgs", "flatpak-unused-system", "snapshots"];

/// uQ refuses a longer argument (docs/settings-pages.md).
var ARG_BYTES = 4096;
var ROW_LIMIT = 500;
var NAME_LIMIT = 200;
var MESSAGE_LIMIT = 400;
var HISTORY_LIMIT = 20;

var LABELS = {
    "cache": "App caches",
    "thumbnails": "Thumbnails",
    "trash": "Trash",
    "temp": "Old temporary files",
    "browser-cache": "Browser caches",
    "dev-cache": "Developer caches",
    "flatpak-unused": "Unused Flatpak runtimes",
    "appimage-old": "Old AppImage versions",
    "logs": "Old log files",
    "aur-build": "Package build folders",
    "app-cache": "Caches of web-based apps",
    "containers": "Unused container images and volumes",
    "build-output": "Build folders in your projects",
    "pkg-cache": "Downloaded package files",
    "orphan-pkgs": "Packages nothing needs",
    "journal": "System logs",
    "coredumps": "Crash dumps",
    "system-temp": "System temporary files",
    "flatpak-unused-system": "Unused Flatpak runtimes (whole system)",
    "snapshots": "Old system snapshots"
};

var SOURCES = {
    "pacman": "Package",
    "dpkg": "Package",
    "rpm": "Package",
    "snap": "Snap",
    "flatpak": "Flatpak",
    "appimage": "AppImage",
    "unmanaged": "Installed by hand"
};

var APP_PATTERN = "^(pacman|dpkg|rpm|snap|flatpak|appimage|unmanaged):[^\\x00-\\x1f\\x7f]{1,400}$";
/// The keys `crumbs config` changes (crumbs-core config::edit::KEYS), split by kind.
var NUMBER_KEYS = ["temp_age_days", "log_age_days", "build_age_days", "snapshots_keep", "pkg_cache_keep", "journal_max"];
var LIST_KEYS = ["protect", "exclude", "appimage_dirs", "project_dirs", "categories_off", "categories_on"];
/// A path or pattern (from / or ~/), or a category name.
var LIST_ENTRY = "^(~|~/[^\\x00-\\x1f\\x7f]*|/[^\\x00-\\x1f\\x7f]*|[a-z][a-z-]{0,39})$";

/// The argument patterns of page.toml's [[command]] tables: command id -> argument -> pattern.
/// tests/uq/logic.test.mjs checks that they are the same strings. uQ refuses an argument over
/// ARG_BYTES bytes, which is why the path pattern has no length bound of its own (a bounded
/// repeat of that size is too big for uQ's regex engine).
var PATTERNS = {
    "clean": { "category": "^(" + USER_TRASH.join("|") + ")$" },
    "clean-for-good": { "category": "^(" + USER_FOR_GOOD.join("|") + ")$" },
    "clean-system": { "category": "^(" + SYSTEM.join("|") + ")$" },
    "orphan-trash": { "path": "^/[^\\x00-\\x1f\\x7f]+$" },
    "orphan-delete-system": { "path": "^/opt/[^/\\x00-\\x1f\\x7f]+$" },
    "restore": { "batch": "^[0-9]{1,20}$" },
    "uninstall": { "app": APP_PATTERN },
    "uninstall-keep-data": { "app": APP_PATTERN },
    "uninstall-leftovers": { "app": APP_PATTERN },
    "uninstall-close": { "app": APP_PATTERN },
    "uninstall-keep-data-close": { "app": APP_PATTERN },
    "uninstall-leftovers-close": { "app": APP_PATTERN },
    "preview-uninstall": { "app": APP_PATTERN },
    "preview-keep-data": { "app": APP_PATTERN },
    "preview-leftovers": { "app": APP_PATTERN },
    "config-set": { "key": "^(" + NUMBER_KEYS.join("|") + ")$", "value": "^[0-9]{1,15}[KMGT]?$" },
    "config-add": { "key": "^(" + LIST_KEYS.join("|") + ")$", "value": LIST_ENTRY },
    "config-remove": { "key": "^(" + LIST_KEYS.join("|") + ")$", "value": LIST_ENTRY },
    "config-reset": { "key": "^(" + NUMBER_KEYS.concat(LIST_KEYS).join("|") + ")$" }
};

/// Shown beside a row while its command runs.
var WORKING = "Working…";
/// The writes that may ask for your password (crumBS gives up after 2 minutes without an answer).
var NEEDS_PASSWORD = ["clean-system", "orphan-delete-system", "uninstall", "uninstall-keep-data", "uninstall-leftovers", "uninstall-close", "uninstall-keep-data-close", "uninstall-leftovers-close"];

/// What a row says while `command` runs.
function workingText(command) {
    return NEEDS_PASSWORD.indexOf(command) >= 0
        ? WORKING + " If a password dialog opened, enter your password there."
        : WORKING;
}

function has(table, key) {
    return typeof key === "string" && Object.prototype.hasOwnProperty.call(table, key);
}

/// A string for display: control characters become spaces, and it is cut to `limit`.
function clip(value, limit) {
    var s = typeof value === "string" ? value : (value === null || value === undefined ? "" : String(value));
    s = s.replace(/[\u0000-\u001f\u007f]/g, " ");
    var n = limit || NAME_LIMIT;
    return s.length > n ? s.slice(0, n - 1) + "…" : s;
}

function number(value) {
    return typeof value === "number" && isFinite(value) && value >= 0 ? value : null;
}

function plural(n, one, many) {
    return n + " " + (n === 1 ? one : many);
}

/// "1.2 GB": base 1000, one decimal, "—" when unknown.
function size(bytes) {
    var n = number(bytes);
    if (n === null)
        return "—";
    if (n < 1000)
        return Math.round(n) + " B";
    var units = ["kB", "MB", "GB", "TB", "PB"];
    var i = -1;
    do {
        n = n / 1000;
        i++;
    } while (n >= 999.95 && i < units.length - 1);
    return n.toFixed(1) + " " + units[i];
}

function isCancelled(result) {
    return !!(result && result.cancelled);
}

/// What a command's callback got, as one shape. `output` is read for every exit code, since
/// crumbs prints a complete envelope when it refuses or fails (E-F13). `cli` is the refusal's
/// message; `error` is the text of a failure that has no envelope (a timeout, say).
function envelope(result) {
    var r = result && typeof result === "object" ? result : {};
    var out = r.output;
    var env = out && typeof out === "object" && !Array.isArray(out) ? out : null;
    var errors = [];
    var cli = null;
    if (env && Array.isArray(env.errors)) {
        for (var i = 0; i < env.errors.length && i < ROW_LIMIT; i++) {
            var e = env.errors[i];
            if (!e || typeof e !== "object" || typeof e.message !== "string")
                continue;
            var entry = { source: clip(e.source, 60), message: clip(e.message, MESSAGE_LIMIT) };
            errors.push(entry);
            if (cli === null && e.source === "cli")
                cli = entry.message;
        }
    }
    var code = typeof r.code === "number" ? r.code : null;
    return {
        ok: code === 0 && env !== null,
        code: code,
        data: env && env.data && typeof env.data === "object" && !Array.isArray(env.data) ? env.data : null,
        errors: errors,
        cli: cli,
        error: typeof r.error === "string" && r.error !== "" ? clip(r.error, MESSAGE_LIMIT) : null
    };
}

/// The command id that cleans `category`, or null for a name this page doesn't know.
function cleanCommandFor(category) {
    if (USER_TRASH.indexOf(category) >= 0)
        return "clean";
    if (USER_FOR_GOOD.indexOf(category) >= 0)
        return "clean-for-good";
    if (SYSTEM.indexOf(category) >= 0)
        return "clean-system";
    return null;
}

function howText(command, category) {
    var parts = [];
    if (command === "clean")
        parts.push("Goes to the trash");
    else if (command === "clean-for-good")
        parts.push("Deleted for good");
    else if (command === "clean-system")
        parts.push("Needs your password and can't be undone");
    if (OPT_IN_SYSTEM.indexOf(category) >= 0)
        parts.push("Only when you ask");
    return parts.join(" · ");
}

function categoryLabel(name) {
    return has(LABELS, name) ? LABELS[name] : clip(name, 60);
}

/// One row per category of a `scan` answer, with plain labels and the command that cleans it.
function categories(scanData) {
    var list = scanData && Array.isArray(scanData.categories) ? scanData.categories : [];
    var out = [];
    for (var i = 0; i < list.length && out.length < ROW_LIMIT; i++) {
        var c = list[i];
        if (!c || typeof c !== "object" || typeof c.category !== "string")
            continue;
        var items = Array.isArray(c.items) ? c.items.length : 0;
        var total = number(c.total) || 0;
        var unknown = number(c.unknown_sizes) || 0;
        var sizeText = size(total);
        if (items > 0 && total === 0 && unknown > 0)
            sizeText = "Size unknown";
        else if (unknown > 0)
            sizeText += " and more";
        var command = cleanCommandFor(c.category);
        out.push({
            category: c.category,
            label: categoryLabel(c.category),
            total: total,
            items: items,
            unknown: unknown,
            skipped: c.skipped === true,
            unavailable: typeof c.unavailable === "string" && c.unavailable !== "" ? clip(c.unavailable, MESSAGE_LIMIT) : null,
            error: typeof c.error === "string" && c.error !== "" ? clip(c.error, MESSAGE_LIMIT) : null,
            command: command,
            sizeText: sizeText,
            itemsText: plural(items, "item", "items"),
            how: howText(command, c.category)
        });
    }
    return out;
}

function emptyView() {
    return { rows: [], unavailable: "", switchedOff: "", problem: "", total: 0, totalText: "" };
}

/// What a space section shows for a `scan` (or `scan --system`) result: the rows to list, a
/// line for what isn't on this computer, a line for what crumbs has switched off, and a
/// `problem` when the answer couldn't be read.
function spaceView(result) {
    var env = envelope(result);
    var view = emptyView();
    if (!env.data || !Array.isArray(env.data.categories)) {
        view.problem = env.cli || env.error || "crumBS gave an answer this page can't read.";
        return view;
    }
    var unavailable = [];
    var off = [];
    var all = categories(env.data);
    for (var i = 0; i < all.length; i++) {
        if (all[i].skipped)
            off.push(all[i].label);
        else if (all[i].unavailable !== null)
            unavailable.push(all[i].label);
        else
            view.rows.push(all[i]);
    }
    // How big each row is next to the biggest (for its bar), and the sum of them all.
    var biggest = 0;
    for (var r = 0; r < view.rows.length; r++) {
        view.total += view.rows[r].total;
        biggest = Math.max(biggest, view.rows[r].total);
    }
    for (var b = 0; b < view.rows.length; b++)
        view.rows[b].share = biggest > 0 ? view.rows[b].total / biggest : 0;
    view.totalText = view.total > 0 ? size(view.total) : "Nothing to free";
    if (unavailable.length > 0)
        view.unavailable = "Not on this computer: " + unavailable.join(", ");
    if (off.length > 0)
        view.switchedOff = "Switched off in crumBS: " + off.join(", ");
    return view;
}

/// The UTF-8 length of `text`, as uQ counts an argument.
function byteLength(text) {
    var n = 0;
    for (var i = 0; i < text.length; i++) {
        var c = text.charCodeAt(i);
        if (c < 0x80) {
            n += 1;
        } else if (c < 0x800) {
            n += 2;
        } else if (c >= 0xd800 && c <= 0xdbff && i + 1 < text.length
                   && text.charCodeAt(i + 1) >= 0xdc00 && text.charCodeAt(i + 1) <= 0xdfff) {
            n += 4;
            i++;
        } else {
            n += 3;
        }
    }
    return n;
}

/// "" when `value` is fine for argument `name` of command `command`, else why not. These are
/// the manifest's patterns; the page asks before it calls run.
function argProblem(command, name, value) {
    if (!has(PATTERNS, command) || !has(PATTERNS[command], name))
        return "That isn't something this page can do.";
    if (typeof value !== "string" || value === "")
        return "Nothing was chosen.";
    if (value.charAt(0) === "-")
        return "That name isn't allowed.";
    if (byteLength(value) > ARG_BYTES)
        return "That name is too long.";
    if (!new RegExp(PATTERNS[command][name]).test(value))
        return "That name isn't allowed.";
    return "";
}

/// "" when `args` are exactly the arguments `command` takes and each is fine, else why not.
function argsProblem(command, args) {
    if (!has(PATTERNS, command))
        return "That isn't something this page can do.";
    var names = Object.keys(PATTERNS[command]);
    if (!Array.isArray(args) || args.length !== names.length)
        return "Nothing was chosen.";
    for (var i = 0; i < names.length; i++) {
        var why = argProblem(command, names[i], args[i]);
        if (why !== "")
            return why;
    }
    return "";
}

function reportsOf(data) {
    var out = [];
    var report = data && Array.isArray(data.report) ? data.report : [];
    for (var i = 0; i < report.length && i < ROW_LIMIT; i++)
        if (report[i] && typeof report[i] === "object")
            out.push(report[i]);
    return out;
}

function freedBy(data) {
    var total = 0;
    var reports = reportsOf(data);
    for (var i = 0; i < reports.length; i++) {
        var freed = number(reports[i].freed);
        if (freed !== null)
            total += freed;
    }
    return total;
}

function failuresOf(data) {
    var out = [];
    var reports = reportsOf(data);
    for (var i = 0; i < reports.length; i++) {
        var results = Array.isArray(reports[i].results) ? reports[i].results : [];
        for (var j = 0; j < results.length && out.length < 20; j++) {
            var r = results[j];
            if (!r || r.ok === true || typeof r.error !== "string")
                continue;
            var path = r.action && typeof r.action.path === "string" ? clip(r.action.path) + ": " : "";
            out.push(path + clip(r.error, MESSAGE_LIMIT));
        }
    }
    return out;
}

function manualOf(data) {
    var out = [];
    var list = data && Array.isArray(data.manual_commands) ? data.manual_commands : [];
    for (var i = 0; i < list.length && out.length < 50; i++)
        if (typeof list[i] === "string")
            out.push(clip(list[i], MESSAGE_LIMIT));
    return out;
}

function restoredCount(data) {
    var n = 0;
    var list = data && Array.isArray(data.outcomes) ? data.outcomes : [];
    for (var i = 0; i < list.length && i < ROW_LIMIT; i++)
        if (list[i] && list[i].ok === true && typeof list[i].entry_id === "string" && list[i].entry_id !== "")
            n++;
    return n;
}

function isRestore(data) {
    return !!data && Array.isArray(data.outcomes);
}

function doneMessage(data) {
    if (isRestore(data)) {
        var n = restoredCount(data);
        return n > 0 ? "Put back " + plural(n, "item", "items") : "Nothing in this cleanup could be put back";
    }
    var reports = reportsOf(data);
    if (reports.length === 0)
        return "Nothing to clean up";
    return "Freed " + size(freedBy(data));
}

/// True when crumbs refused an uninstall because the app is open ("X is running (pid 1)").
function isRunningRefusal(result) {
    var env = envelope(result);
    return env.code === 1 && env.cli !== null && /^.{1,200} is running \(pid [0-9, ]+\)/.test(env.cli);
}

/// crumbs refused or couldn't start (exit 1, 2 or 5, or no exit code): nothing was changed.
function refusedOutright(result) {
    var code = envelope(result).code;
    return code === null || code === 1 || code === 2 || code === 5;
}

/// The first failure a `restore` answer reports, or null. A restore that put nothing back exits
/// 2 with no `errors`, only its outcomes (a batch that is gone, say).
function restoreFailure(data) {
    var list = isRestore(data) ? data.outcomes : [];
    for (var i = 0; i < list.length && i < ROW_LIMIT; i++)
        if (list[i] && list[i].ok !== true && typeof list[i].error === "string" && list[i].error !== "")
            return "Couldn't put this back: " + clip(list[i].error, MESSAGE_LIMIT);
    return null;
}

/// What to say beside a row after a write: {message, manual, detail, failed}. `manual` holds the
/// commands to run by hand (exit 5) and `detail` what failed, both for Advanced details.
function outcome(result) {
    var env = envelope(result);
    if (isRunningRefusal(result)) {
        // crumbs' own text points at --force, which this page never uses.
        var open = env.cli.replace(/;.*$/, "");
        return { message: open + ". Close it, or let crumBS ask it to close.", manual: [], detail: [], failed: true };
    }
    var res = { message: "", manual: [], detail: [], failed: false };
    if (isCancelled(result))
        return res;
    var fallback = env.cli || restoreFailure(env.data) || env.error || "Something went wrong";
    if (env.code === 0) {
        res.message = doneMessage(env.data);
    } else if (env.code === 3) {
        res.message = isRestore(env.data) ? "Some items couldn't be put back" : "Some items couldn't be removed";
        res.failed = true;
        res.detail = failuresOf(env.data);
    } else if (env.code === 4) {
        res.message = "Cancelled at a prompt";
    } else if (env.code === 5) {
        res.message = "Couldn't get administrator rights";
        res.failed = true;
        res.manual = manualOf(env.data);
    } else {
        res.message = fallback;
        res.failed = true;
    }
    return res;
}

/// `notes` with `note` stored under `key`, as a new object so a binding to it updates.
function withNote(notes, key, note) {
    var out = {};
    for (var k in notes)
        if (has(notes, k))
            out[k] = notes[k];
    out[key] = note;
    return out;
}
