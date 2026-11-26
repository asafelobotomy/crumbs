.pragma library
.import "logic.js" as Logic

// The Storage and cleanup page's lists of apps, leftovers and history batches. No Qt calls, so
// Node can test it (tests/uq/). Shared helpers are in logic.js.

var ROW_LIMIT = Logic.ROW_LIMIT;
var MESSAGE_LIMIT = Logic.MESSAGE_LIMIT;
var HISTORY_LIMIT = Logic.HISTORY_LIMIT;
var SOURCES = Logic.SOURCES;
var has = Logic.has;
var clip = Logic.clip;
var number = Logic.number;
var plural = Logic.plural;
var size = Logic.size;
var argProblem = Logic.argProblem;

/// What the Leftovers section lists, from an `orphans` answer.
function orphansList(data) {
    var list = data && Array.isArray(data.orphans) ? data.orphans : [];
    var out = [];
    for (var i = 0; i < list.length && out.length < ROW_LIMIT; i++) {
        var f = list[i];
        if (!f || typeof f !== "object" || typeof f.path !== "string")
            continue;
        out.push({
            path: f.path,
            shown: clip(f.path),
            bytes: f.size && number(f.size.bytes) !== null ? f.size.bytes : null,
            sizeText: size(f.size ? f.size.bytes : null),
            reason: clip(f.reason, MESSAGE_LIMIT),
            valid: argProblem("orphan-trash", "path", f.path) === ""
        });
    }
    return out;
}

/// "2026-11-08 12:43 UTC" from an RFC 3339 time.
function shortTime(text) {
    var m = /^(\d{4})-(\d{2})-(\d{2})T(\d{2}):(\d{2})/.exec(typeof text === "string" ? text : "");
    return m ? m[1] + "-" + m[2] + "-" + m[3] + " " + m[4] + ":" + m[5] + " UTC" : clip(text, 40);
}

function appName(id) {
    var name = id.slice(id.indexOf(":") + 1);
    var slash = name.lastIndexOf("/");
    return slash >= 0 && slash < name.length - 1 ? name.slice(slash + 1) : name;
}

function batchTitle(label) {
    if (label === "clean")
        return "Cleanup";
    if (label === "orphans")
        return "Leftovers moved to the trash";
    if (label === "restore")
        return "Put back";
    return "Removed " + clip(appName(label));
}

/// The recent cleanups of a `history` answer (at most 20). `canRestore` is true when the batch
/// has an entry that went to the trash (or was edited) and that no later, successful restore
/// entry has put back.
function batches(data) {
    var list = data && Array.isArray(data.batches) ? data.batches : [];
    var restored = {};
    for (var i = 0; i < list.length && i < HISTORY_LIMIT; i++) {
        var es = list[i] && Array.isArray(list[i].entries) ? list[i].entries : [];
        for (var j = 0; j < es.length && j < ROW_LIMIT; j++)
            if (es[j] && es[j].action === "restore" && es[j].ok === true && typeof es[j].restores === "string")
                restored[es[j].restores] = true;
    }
    var out = [];
    for (var k = 0; k < list.length && out.length < HISTORY_LIMIT; k++) {
        var b = list[k];
        if (!b || typeof b !== "object" || typeof b.batch !== "string")
            continue;
        var entries = Array.isArray(b.entries) ? b.entries : [];
        var can = false;
        for (var m = 0; m < entries.length && m < ROW_LIMIT; m++) {
            var e = entries[m];
            if (e && e.ok === true && (e.action === "trash" || e.action === "edit-file" || e.action === "edit_file")
                    && typeof e.id === "string" && restored[e.id] !== true)
                can = true;
        }
        var label = typeof b.label === "string" ? b.label : "";
        var items = number(b.items) || 0;
        out.push({
            batch: b.batch,
            label: clip(label),
            title: batchTitle(label),
            time: shortTime(b.time),
            items: items,
            itemsText: plural(items, "item", "items"),
            sizeText: size(number(b.size)),
            canRestore: can && argProblem("restore", "batch", b.batch) === ""
        });
    }
    return out;
}

/// The menu groups the Apps tab sorts apps into, in the order shown (crumbs' `category` of an
/// app, "Other" when it has none), each with the theme icon of its section header.
var APP_GROUPS = {
    "Internet": "applications-internet",
    "Multimedia": "applications-multimedia",
    "Graphics": "applications-graphics",
    "Office": "applications-office",
    "Games": "applications-games",
    "Development": "applications-development",
    "Utilities": "applications-utilities",
    "System": "applications-system",
    "Other": "applications-other"
};
var APP_GROUP_ORDER = ["Internet", "Multimedia", "Graphics", "Office", "Games", "Development", "Utilities", "System", "Other"];

/// `rows` (from `apps`/`filterApps`) as sections: [{key, title, icon, count, countText, total,
/// totalText, rows}] in APP_GROUP_ORDER, empty groups left out, each group's apps biggest first.
/// An older crumbs says no category at all: then the apps are sorted by where they came from
/// (Package, Flatpak, ...) instead, so the page never shows one heap called "Other".
function groupApps(rows) {
    var known = false;
    for (var i = 0; i < rows.length; i++)
        if (rows[i].category !== "Other")
            known = true;
    var buckets = {};
    var order = [];
    for (var j = 0; j < rows.length; j++) {
        var r = rows[j];
        var key = known ? r.category : r.sourceText;
        if (!has(buckets, key)) {
            buckets[key] = [];
            order.push(key);
        }
        buckets[key].push(r);
    }
    if (known)
        order = APP_GROUP_ORDER.filter(k => has(buckets, k));
    else
        order.sort();
    return order.map(k => {
        var list = buckets[k].slice().sort((a, b) => (b.installSize || 0) - (a.installSize || 0));
        var total = 0;
        for (var m = 0; m < list.length; m++)
            total += list[m].installSize || 0;
        return {
            key: k,
            title: k === "Other" ? "Other apps" : k,
            icon: known ? APP_GROUPS[k] : "applications-other",
            count: list.length,
            countText: plural(list.length, "app", "apps"),
            total: total,
            totalText: size(total),
            rows: list
        };
    });
}

/// The apps of an `apps list` answer: id, plain name, size and where it came from. `partOf` is
/// "uQ" for the desktop's own packages, which the page never offers to remove.
function apps(data) {
    var list = data && Array.isArray(data.apps) ? data.apps : [];
    var out = [];
    for (var i = 0; i < list.length && out.length < ROW_LIMIT; i++) {
        var a = list[i];
        if (!a || typeof a !== "object" || typeof a.id !== "string")
            continue;
        var name = (typeof a.display_name === "string" && a.display_name !== "") ? a.display_name : a.name;
        var part = typeof a.part_of === "string" && a.part_of !== "" ? clip(a.part_of, 40) : null;
        out.push({
            id: a.id,
            name: clip(name) || clip(a.id),
            installSize: number(a.install_size),
            sizeText: size(a.install_size),
            source: typeof a.source === "string" ? a.source : "",
            sourceText: has(SOURCES, a.source) ? SOURCES[a.source] : "Other",
            partOf: part,
            icon: typeof a.icon === "string" ? clip(a.icon, 200) : "",
            category: has(APP_GROUPS, a.category) ? a.category : "Other",
            canUninstall: part === null && argProblem("uninstall", "app", a.id) === ""
        });
    }
    return out;
}

/// The apps whose name contains `query`, ignoring case, at most `limit` of them; `more` says
/// whether the list was cut.
function filterApps(list, query, limit) {
    var q = typeof query === "string" ? query.trim().toLowerCase() : "";
    var max = limit || ROW_LIMIT;
    var rows = [];
    var more = false;
    for (var i = 0; i < list.length; i++) {
        if (q !== "" && list[i].name.toLowerCase().indexOf(q) < 0)
            continue;
        if (rows.length >= max) {
            more = true;
            break;
        }
        rows.push(list[i]);
    }
    return { rows: rows, more: more };
}
