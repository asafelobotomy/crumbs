// What the Storage and cleanup page's QML tests share: the page in a Loader, a fake ctx
// (FakeContext.qml) answering every command from fixtures/, and finders for what it shows.
import QtQuick

Item {
    id: root

    /// The fake ctx and the Loader holding the page, for the tests.
    readonly property alias fake: fake
    readonly property alias loader: loader
    width: 700
    height: 900

    readonly property url pageUrl: Qt.resolvedUrl("../../uq/pages/crumbs/Page.qml")

    FakeContext {
        id: fake
        confirmed: ["clean", "clean-for-good", "clean-system", "orphan-trash", "orphan-delete-system", "restore", "uninstall", "uninstall-keep-data", "uninstall-leftovers", "uninstall-close", "uninstall-keep-data-close", "uninstall-leftovers-close"]
    }

    Loader {
        id: loader
        anchors.fill: parent
    }

    function fixture(name) {
        const xhr = new XMLHttpRequest();
        xhr.open("GET", Qt.resolvedUrl("fixtures/" + name + ".json"), false);
        xhr.send();
        return JSON.parse(xhr.responseText);
    }

    function answer(name) {
        return { code: 0, output: fixture(name) };
    }

    function defaults() {
        return {
            "scan": answer("scan"),
            "scan-system": answer("scan-system"),
            "orphans": answer("orphans"),
            "history": answer("history"),
            "apps": answer("apps"),
            "clean": answer("clean-ok"),
            "clean-for-good": answer("clean-ok"),
            "clean-system": answer("clean-ok"),
            "orphan-trash": answer("clean-ok"),
            "restore": { code: 0, output: { schema_version: 1, command: "restore", data: { outcomes: [{ entry_id: "1-0", error: null, ok: true, path: "/a" }] }, errors: [] } },
            "uninstall": answer("clean-ok"),
            "uninstall-keep-data": answer("clean-ok"),
            "uninstall-leftovers": answer("clean-ok"),
            "uninstall-close": answer("clean-ok"),
            "uninstall-keep-data-close": answer("clean-ok"),
            "uninstall-leftovers-close": answer("clean-ok"),
            "preview-uninstall": answer("uninstall-preview"),
            "preview-keep-data": answer("uninstall-preview"),
            "preview-leftovers": answer("uninstall-preview"),
            "orphans-system": { code: 0, output: { schema_version: 1, command: "orphans", errors: [], data: { orphans: [
                { path: "/opt/oldtool", kind: "system-data", scope: "system-tree", confidence: "low", reason: "no package owns it", shared: false, preselected: false, mime_lines: [], size: { bytes: 4096, files: 1, errors: 0 } }] } } },
            "orphan-delete-system": answer("clean-ok"),
            "config": answer("config"),
            "config-set": answer("config"),
            "config-add": answer("config"),
            "config-remove": answer("config"),
            "config-reset": answer("config")
        };
    }

    /// Shows the tab with this title, as a click on it would.
    function openTab(title) {
        const tab = loader.item.tabList.find(t => t.title === title);
        loader.item.show(tab);
        if (title === "Apps") {
            // Categories start folded but the biggest; the tests look at every app.
            const apps = all(loader.item, []).find(i => i.folded !== undefined && i.toggle !== undefined);
            if (apps)
                apps.folded = { Internet: true, Multimedia: true, Graphics: true, Office: true, Games: true, Development: true, Utilities: true, System: true, Other: true };
        }
        return tab;
    }

    function all(item, out) {
        out.push(item);
        for (let i = 0; i < item.children.length; ++i)
            all(item.children[i], out);
        return out;
    }

    function isButton(item) {
        return typeof item.clicked === "function" && item.text !== undefined;
    }

    // A text item that isn't a button.
    function findLabel(item, text) {
        const hits = all(item, []).filter(i => i.text === text && !isButton(i));
        return hits.length > 0 ? hits[0] : null;
    }

    function findLabels(item, text) {
        return all(item, []).filter(i => i.text === text && !isButton(i));
    }

    function findButton(item, text) {
        const hits = all(item, []).filter(i => isButton(i) && i.text === text);
        return hits.length > 0 ? hits[0] : null;
    }

    function findButtons(item, text) {
        return all(item, []).filter(i => isButton(i) && i.text === text);
    }

    function runs(id) {
        return fake.calls.filter(c => c[0] === "run" && (id === undefined || c[1] === id));
    }

    function shows(text) {
        return () => findLabel(loader.item, text) !== null;
    }

    function idle() {
        return loader.item.busy === false;
    }
}
