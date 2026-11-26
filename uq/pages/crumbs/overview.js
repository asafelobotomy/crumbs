.pragma library
.import "logic.js" as Logic

// The Storage and cleanup page's overview pie chart and its preferences (`crumbs config`). No Qt
// calls, so Node can test it (tests/uq/). Shared helpers and the patterns are in logic.js.

var USER_TRASH = Logic.USER_TRASH;
var USER_FOR_GOOD = Logic.USER_FOR_GOOD;
var ROW_LIMIT = Logic.ROW_LIMIT;
var NUMBER_KEYS = Logic.NUMBER_KEYS;
var LIST_KEYS = Logic.LIST_KEYS;
var clip = Logic.clip;
var number = Logic.number;
var size = Logic.size;
var envelope = Logic.envelope;
var categoryLabel = Logic.categoryLabel;

// ---- the overview's pie chart -------------------------------------------------------------

/// The slice colours, in the order slices take them (biggest first), for a light and a dark
/// theme; the last is the neutral grey of "Everything else". Checked for colour blindness with
/// the dataviz palette validator.
var PIE_COLORS = {
    light: ["#2a78d6", "#eb6834", "#1baf7a", "#eda100", "#e87ba4", "#008300", "#6250d6"],
    dark: ["#3987e5", "#d95926", "#199e70", "#c98500", "#d55181", "#0ca30c", "#9085e9"],
    otherLight: "#a3a29b",
    otherDark: "#6e6d66"
};
var PIE_SLICES = 7;

/// True for a light text colour ("#e0e0e0"), which means a dark theme.
function isLightColor(color) {
    var m = /^#?([0-9a-f]{2})([0-9a-f]{2})([0-9a-f]{2})/i.exec(typeof color === "string" ? color : "");
    if (!m)
        return false;
    var lum = 0.2126 * parseInt(m[1], 16) + 0.7152 * parseInt(m[2], 16) + 0.0722 * parseInt(m[3], 16);
    return lum > 140;
}

/// The slices of a space view's rows (Logic.spaceView): the biggest categories, each with its
/// share, angles (radians from the top, clockwise) and colour; past PIE_SLICES - 1 of them the
/// rest join one "Everything else" slice. Empty when nothing has a size.
function pieSlices(rows, dark) {
    var list = (Array.isArray(rows) ? rows : []).filter(r => r && number(r.total) !== null && r.total > 0);
    list.sort((a, b) => b.total - a.total);
    var total = 0;
    for (var i = 0; i < list.length; i++)
        total += list[i].total;
    var parts = list;
    if (list.length > PIE_SLICES) {
        var rest = 0;
        for (var j = PIE_SLICES - 1; j < list.length; j++)
            rest += list[j].total;
        parts = list.slice(0, PIE_SLICES - 1).concat([{ label: "Everything else", total: rest, other: true }]);
    }
    var colors = dark ? PIE_COLORS.dark : PIE_COLORS.light;
    var out = [];
    var at = 0;
    for (var k = 0; k < parts.length && total > 0; k++) {
        var share = parts[k].total / total;
        var pct = share * 100;
        out.push({
            label: clip(parts[k].label, 80),
            total: parts[k].total,
            sizeText: size(parts[k].total),
            percentText: pct < 1 ? "<1%" : Math.round(pct) + "%",
            share: share,
            start: at * 2 * Math.PI,
            end: (at + share) * 2 * Math.PI,
            color: parts[k].other === true ? (dark ? PIE_COLORS.otherDark : PIE_COLORS.otherLight) : colors[k]
        });
        at += share;
    }
    return out;
}

/// Which slice the point (x, y) is on, for a ring centred at (cx, cy) between radii `inner` and
/// `outer`, or -1.
function sliceAt(slices, x, y, cx, cy, inner, outer) {
    var dx = x - cx;
    var dy = y - cy;
    var r = Math.sqrt(dx * dx + dy * dy);
    if (r < inner || r > outer)
        return -1;
    var angle = Math.atan2(dx, -dy);
    if (angle < 0)
        angle += 2 * Math.PI;
    for (var i = 0; i < slices.length; i++)
        if (angle >= slices[i].start && angle < slices[i].end)
            return i;
    return -1;
}

// ---- preferences (`crumbs config`) ---------------------------------------------------------

var KEY_TEXT = {
    "temp_age_days": { label: "Temporary files count as old after", unit: "days", min: 1 },
    "log_age_days": { label: "Log files count as old after", unit: "days", min: 1 },
    "build_age_days": { label: "Build folders count as old after", unit: "days", min: 1 },
    "snapshots_keep": { label: "System snapshots to keep", unit: "", min: 1 },
    "pkg_cache_keep": { label: "Versions of each package to keep", unit: "", min: 0 },
    "journal_max": { label: "Keep system logs under", unit: "", min: 0 },
    "protect": { label: "Never touch these folders", help: "crumBS never offers anything inside them." },
    "exclude": { label: "Leave out of cleanups", help: "Folders or patterns a cleanup skips, like ~/.cache/my-project." },
    "appimage_dirs": { label: "Where your AppImages are", help: "Folders crumBS looks in for AppImages." },
    "project_dirs": { label: "Where your projects are", help: "Folders searched for build folders." }
};

/// The settings in a `config show` answer: numbers, lists and the categories switched on.
function configView(result) {
    var env = envelope(result);
    var view = { problem: "", path: "", numbers: [], lists: [], off: [], on: [] };
    var values = env.data && env.data.values && typeof env.data.values === "object" ? env.data.values : null;
    if (values === null) {
        view.problem = env.cli || env.error || "crumBS gave an answer this page can't read.";
        return view;
    }
    view.path = clip(env.data.path, 300);
    for (var i = 0; i < NUMBER_KEYS.length; i++) {
        var k = NUMBER_KEYS[i];
        var v = values[k];
        var isSize = k === "journal_max";
        view.numbers.push({
            key: k,
            label: KEY_TEXT[k].label,
            unit: KEY_TEXT[k].unit,
            min: KEY_TEXT[k].min,
            isSize: isSize,
            value: isSize ? (typeof v === "string" ? clip(v, 20) : "") : (number(v) === null ? KEY_TEXT[k].min : Math.floor(v))
        });
    }
    for (var j = 0; j < 4; j++) {
        var key = LIST_KEYS[j];
        var entries = Array.isArray(values[key]) ? values[key] : [];
        view.lists.push({
            key: key,
            label: KEY_TEXT[key].label,
            help: KEY_TEXT[key].help,
            entries: entries.filter(e => typeof e === "string").slice(0, ROW_LIMIT).map(e => clip(e, 300))
        });
    }
    var strings = list => (Array.isArray(list) ? list : []).filter(e => typeof e === "string");
    view.off = strings(values.categories_off);
    view.on = strings(values.categories_on);
    return view;
}

/// Whether a plain `scan` and `clean` include `category`. `build-output` runs only when
/// categories_on names it; the rest unless categories_off names them and categories_on doesn't.
function categoryOn(category, view) {
    var on = view && Array.isArray(view.on) ? view.on : [];
    var off = view && Array.isArray(view.off) ? view.off : [];
    if (category === "build-output")
        return on.indexOf(category) >= 0;
    return off.indexOf(category) < 0 || on.indexOf(category) >= 0;
}

/// The config changes, as [command, args] pairs in order, that switch `category` on or off.
function categorySteps(category, want, view) {
    var on = view && Array.isArray(view.on) ? view.on : [];
    var off = view && Array.isArray(view.off) ? view.off : [];
    var steps = [];
    if (want) {
        if (off.indexOf(category) >= 0)
            steps.push(["config-remove", ["categories_off", category]]);
        if (category === "build-output" && on.indexOf(category) < 0)
            steps.push(["config-add", ["categories_on", category]]);
    } else {
        if (on.indexOf(category) >= 0)
            steps.push(["config-remove", ["categories_on", category]]);
        if (category !== "build-output" && off.indexOf(category) < 0)
            steps.push(["config-add", ["categories_off", category]]);
    }
    return steps;
}

/// The user categories, with their plain names, for the switches.
function userCategories() {
    return USER_TRASH.concat(USER_FOR_GOOD).map(c => ({ category: c, label: categoryLabel(c) }));
}
