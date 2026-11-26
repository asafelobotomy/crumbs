.pragma library
.import "logic.js" as Logic

// The Storage and cleanup page's uninstall: the preview of what goes, the commands, and the
// report afterwards. No Qt calls, so Node can test it (tests/uq/). Shared helpers are in logic.js.

var ROW_LIMIT = Logic.ROW_LIMIT;
var MESSAGE_LIMIT = Logic.MESSAGE_LIMIT;
var has = Logic.has;
var clip = Logic.clip;
var number = Logic.number;
var plural = Logic.plural;
var size = Logic.size;
var isCancelled = Logic.isCancelled;
var envelope = Logic.envelope;
var reportsOf = Logic.reportsOf;
var freedBy = Logic.freedBy;
var manualOf = Logic.manualOf;
var isRunningRefusal = Logic.isRunningRefusal;
var refusedOutright = Logic.refusedOutright;
var outcome = Logic.outcome;

// ---- uninstall preview --------------------------------------------------------------------

/// The groups of an uninstall plan (crumbs' `group` on each action), in the order shown.
var GROUPS = [
    { key: "app", title: "The app itself" },
    { key: "data", title: "Your files and data" },
    { key: "settings", title: "Settings" },
    { key: "cache", title: "Caches" },
    { key: "launchers", title: "Menu entries, icons and file types" },
    { key: "other", title: "Other" }
];
var PREVIEW_ITEMS = 50;

/// One planned action as a line of text: its path, or what a package or command removes.
function actionText(action, home) {
    if (!action || typeof action !== "object")
        return "";
    if (typeof action.path === "string")
        return clip(shortPath(action.path, home), 300);
    if (action.type === "package-remove" && Array.isArray(action.names))
        return clip("Package " + action.names.filter(n => typeof n === "string").join(", "), 300);
    if (typeof action.program === "string")
        return clip([action.program].concat(Array.isArray(action.args) ? action.args : []).join(" "), 300);
    return "";
}

/// `/home/user/x` as `~/x` when `home` is the home folder.
function shortPath(path, home) {
    if (typeof home === "string" && home !== "" && (path === home || path.indexOf(home + "/") === 0))
        return "~" + path.slice(home.length);
    return path;
}

/// What an `uninstall --dry-run` answer removes, grouped: {problem, groups, totalText, notes,
/// empty}. Each group says whether its items go to the trash or are gone for good.
function removalPreview(result, home) {
    var env = envelope(result);
    var view = { problem: "", groups: [], totalText: "", notes: [], empty: true };
    var plans = env.data && Array.isArray(env.data.plans) ? env.data.plans : null;
    if (plans === null || plans.length === 0 || !plans[0] || typeof plans[0] !== "object") {
        view.problem = env.cli || env.error || "crumBS gave an answer this page can't read.";
        return view;
    }
    var plan = plans[0];
    var actions = Array.isArray(plan.actions) ? plan.actions : [];
    var byKey = {};
    var total = 0;
    var unknown = false;
    for (var i = 0; i < actions.length && i < ROW_LIMIT; i++) {
        var a = actions[i];
        if (!a || typeof a !== "object")
            continue;
        var key = typeof a.group === "string" && GROUPS.some(g => g.key === a.group) ? a.group : "other";
        if (!byKey[key])
            byKey[key] = { count: 0, bytes: 0, forGood: false, items: [] };
        var g = byKey[key];
        var bytes = number(a.size);
        g.count++;
        g.bytes += bytes || 0;
        total += bytes || 0;
        unknown = unknown || bytes === null;
        var type = a.action && typeof a.action.type === "string" ? a.action.type : "";
        if (type !== "trash" && type !== "edit-file")
            g.forGood = true;
        if (g.items.length < PREVIEW_ITEMS) {
            var text = actionText(a.action, home);
            if (text !== "")
                g.items.push(text);
        }
    }
    for (var k = 0; k < GROUPS.length; k++) {
        var grp = byKey[GROUPS[k].key];
        if (!grp)
            continue;
        view.groups.push({
            key: GROUPS[k].key,
            title: GROUPS[k].title,
            countText: plural(grp.count, "item", "items"),
            sizeText: grp.bytes > 0 ? size(grp.bytes) : "",
            how: grp.forGood ? "Removed for good" : "Goes to the trash",
            forGood: grp.forGood,
            items: grp.items,
            more: grp.count - grp.items.length
        });
    }
    view.empty = view.groups.length === 0;
    view.totalText = view.empty ? "Nothing to remove." : "Frees about " + size(total) + (unknown ? " (some sizes unknown)" : "");
    var notes = Array.isArray(plan.notes) ? plan.notes : [];
    for (var n = 0; n < notes.length && n < 10; n++)
        if (typeof notes[n] === "string" && notes[n] !== "")
            view.notes.push(clip(notes[n], MESSAGE_LIMIT));
    return view;
}

/// The preview and the removal for each way of uninstalling.
var UNINSTALL_MODES = {
    "all": { preview: "preview-uninstall", run: "uninstall", close: "uninstall-close" },
    "keep-data": { preview: "preview-keep-data", run: "uninstall-keep-data", close: "uninstall-keep-data-close" },
    "leftovers": { preview: "preview-leftovers", run: "uninstall-leftovers", close: "uninstall-leftovers-close" }
};

function uninstallCommands(mode) {
    return has(UNINSTALL_MODES, mode) ? UNINSTALL_MODES[mode] : UNINSTALL_MODES["all"];
}

/// What one finished uninstall did, for the uninstall dialog: {status, message, failed, running,
/// freed, groups, failures, manual}. `status` is "removed" (all of it went), "partial" (some
/// items stayed), "refused" (crumbs changed nothing: the app is open, say), "failed" (no
/// administrator rights, or crumbs couldn't run) or "cancelled" (the person said no). `groups`
/// are what was removed, sorted like the preview; `failures` what stayed, with why.
function removalReport(result, home) {
    var env = envelope(result);
    var rep = { status: "", message: "", failed: false, running: false, freed: 0, freedText: "", groups: [], failures: [], manual: [] };
    if (isCancelled(result)) {
        rep.status = "cancelled";
        rep.message = "Not uninstalled: you said no.";
        return rep;
    }
    if (isRunningRefusal(result)) {
        rep.status = "refused";
        rep.running = true;
        rep.failed = true;
        rep.message = outcome(result).message;
        return rep;
    }
    var plans = env.data && Array.isArray(env.data.plans) ? env.data.plans : [];
    var plan = plans.length > 0 && plans[0] && typeof plans[0] === "object" ? plans[0] : null;
    var actions = plan && Array.isArray(plan.actions) ? plan.actions : [];
    var reports = reportsOf(env.data);
    var results = reports.length > 0 && Array.isArray(reports[0].results) ? reports[0].results : [];
    var byKey = {};
    for (var i = 0; i < results.length && i < ROW_LIMIT; i++) {
        var r = results[i];
        if (!r || typeof r !== "object")
            continue;
        var text = actionText(r.action, home);
        if (r.ok !== true) {
            if (rep.failures.length < 20)
                rep.failures.push((text !== "" ? text + ": " : "") + clip(typeof r.error === "string" ? r.error : "not removed", MESSAGE_LIMIT));
            continue;
        }
        var planned = actions[i] && typeof actions[i] === "object" ? actions[i] : {};
        var key = typeof planned.group === "string" && GROUPS.some(g => g.key === planned.group) ? planned.group : "other";
        if (!byKey[key])
            byKey[key] = { count: 0, items: [] };
        byKey[key].count++;
        if (text !== "" && byKey[key].items.length < PREVIEW_ITEMS)
            byKey[key].items.push(text);
    }
    for (var k = 0; k < GROUPS.length; k++) {
        var grp = byKey[GROUPS[k].key];
        if (grp)
            rep.groups.push({ key: GROUPS[k].key, title: GROUPS[k].title, countText: plural(grp.count, "item", "items"), items: grp.items, more: grp.count - grp.items.length });
    }
    rep.freed = freedBy(env.data);
    rep.freedText = size(rep.freed);
    if (env.code === 0) {
        rep.status = "removed";
        rep.message = rep.groups.length > 0 ? "Removed · freed " + rep.freedText : "Nothing needed removing.";
    } else if (env.code === 3) {
        rep.status = "partial";
        rep.failed = true;
        rep.message = "Some items couldn't be removed · freed " + rep.freedText;
    } else {
        rep.status = refusedOutright(result) && env.code !== 5 ? "refused" : "failed";
        rep.failed = true;
        rep.message = outcome(result).message;
        rep.manual = manualOf(env.data);
    }
    return rep;
}

/// The words over a finished queue: how many apps went, and the space freed.
function queueSummary(reports) {
    var gone = 0;
    var freed = 0;
    for (var i = 0; i < reports.length; i++) {
        if (!reports[i])
            continue;
        if (reports[i].status === "removed" || reports[i].status === "partial")
            gone++;
        freed += reports[i].freed || 0;
    }
    if (reports.length === 1)
        return gone === 1 ? "Done · freed " + size(freed) : "Nothing was uninstalled";
    return "Uninstalled " + gone + " of " + plural(reports.length, "app", "apps") + " · freed " + size(freed);
}
