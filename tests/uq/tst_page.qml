// The Storage and cleanup page against a fake ctx (FakeContext.qml): what it asks crumbs for, what
// it shows, and what it never does. Offscreen:
//   QML_XHR_ALLOW_FILE_READ=1 QT_QPA_PLATFORM=offscreen qmltestrunner -import tests/uq/stub -input tests/uq
// QML_XHR_ALLOW_FILE_READ lets fixture() read the files in fixtures/.
import QtQuick
import QtTest

Item {
    id: root
    width: 700
    height: 900

    readonly property url pageUrl: Qt.resolvedUrl("../../uq/pages/crumbs/Page.qml")

    FakeContext {
        id: fake
        confirmed: ["clean", "clean-for-good", "clean-system", "orphan-trash", "restore", "uninstall"]
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
            "uninstall": answer("clean-ok")
        };
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

    function findButton(item, text) {
        const hits = all(item, []).filter(i => isButton(i) && i.text === text);
        return hits.length > 0 ? hits[0] : null;
    }

    function findButtons(item, text) {
        return all(item, []).filter(i => isButton(i) && i.text === text);
    }

    // The button that sits in the same row as the label `labelText`.
    function rowButton(labelText, buttonText) {
        const label = findLabel(loader.item, labelText);
        verify(label !== null, "a row labelled " + labelText);
        const button = findButton(label.parent, buttonText);
        verify(button !== null, labelText + " has a " + buttonText + " button");
        return button;
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

    TestCase {
        name: "StoragePage"
        when: windowShown

        function init() {
            fake.calls = [];
            fake.confirms = true;
            fake.commands = root.defaults();
            loader.source = "";
            loader.source = root.pageUrl;
            verify(loader.status === Loader.Ready, "the page loads");
            loader.item.ctx = fake.ctx;
        }

        function test_scan_on_open_only() {
            tryVerify(root.shows("App caches"));
            tryVerify(root.shows("Cleanup"));
            compare(root.runs().map(c => c[1]).sort(), ["history", "scan"]);
            verify(fake.calls.some(c => c[0] === "watch" && c[1] === "history"), "history is followed, not polled");
            compare(root.runs("scan"), [["run", "scan", []]]);
        }

        function test_rows_say_what_they_are() {
            tryVerify(root.shows("App caches"));
            verify(root.findLabel(loader.item, "1.5 GB · 2 items") !== null);
            verify(root.findLabel(loader.item, "Deleted for good") !== null);
            verify(root.findLabel(loader.item, "Not on this computer: Unused container images and volumes") !== null);
            verify(root.findLabel(loader.item, "Developer caches") === null, "a category that is switched off has no row");
            // nothing system-wide is read until asked
            compare(root.runs("scan-system").length, 0);
            compare(root.runs("orphans").length, 0);
            compare(root.runs("apps").length, 0);
        }

        function test_clean_routes_by_category() {
            tryVerify(root.shows("Browser caches"));
            root.rowButton("Browser caches", "Clean up").clicked();
            tryVerify(root.shows("Freed 1.2 GB"));
            tryVerify(root.idle);
            compare(root.runs("clean"), [["run", "clean", ["browser-cache"]]]);

            tryVerify(root.shows("Trash"));
            root.rowButton("Trash", "Clean up").clicked();
            tryVerify(root.idle);
            compare(root.runs("clean-for-good"), [["run", "clean-for-good", ["trash"]]]);

            root.findButton(loader.item, "Check system space").clicked();
            tryVerify(root.shows("System logs"));
            root.rowButton("System logs", "Clean up…").clicked();
            tryVerify(root.idle);
            compare(root.runs("clean-system"), [["run", "clean-system", ["journal"]]]);
            // the system scan ran once to look, and again after the cleanup
            compare(root.runs("scan-system").length, 2);
        }

        function test_exit5() {
            const c = root.defaults();
            c["clean-system"] = { code: 5, error: "exit 5: administrator rights were not granted", output: root.fixture("clean-exit5") };
            fake.commands = c;
            root.findButton(loader.item, "Check system space").clicked();
            tryVerify(root.shows("System logs"));
            root.rowButton("System logs", "Clean up…").clicked();
            tryVerify(root.shows("Couldn't get administrator rights"));
            const manual = "sudo '/usr/bin/journalctl' '--vacuum-size=500M'";
            verify(root.findLabel(loader.item, manual) !== null);
            compare(root.findLabel(loader.item, manual).visible, false, "the commands are under Advanced details");
            root.findButton(loader.item, "Advanced details").clicked();
            compare(root.findLabel(loader.item, manual).visible, true);
        }

        function test_uninstall_refused_running() {
            const c = root.defaults();
            c["uninstall"] = { code: 1, error: "exit 1: Firefox Web Browser is running", output: root.fixture("uninstall-running") };
            fake.commands = c;
            root.findButton(loader.item, "List apps").clicked();
            tryVerify(root.shows("Firefox Web Browser"));
            root.rowButton("Firefox Web Browser", "Uninstall…").clicked();
            tryVerify(root.shows("Firefox Web Browser is running (pid 4242); close it first or re-run with --force"));
            compare(root.runs("uninstall"), [["run", "uninstall", ["pacman:firefox"]]]);
            tryVerify(root.idle);
        }

        function test_part_of_uq_has_no_button() {
            root.findButton(loader.item, "List apps").clicked();
            tryVerify(root.shows("Part of uQ"));
            const label = root.findLabel(loader.item, "Part of uQ");
            verify(root.findButton(label.parent, "Uninstall…") === null || root.findButton(label.parent, "Uninstall…").visible === false);
            compare(root.findButtons(loader.item, "Uninstall…").filter(b => b.visible).length, 4, "the other four apps can be uninstalled");
        }

        function test_filter_matches_the_name() {
            root.findButton(loader.item, "List apps").clicked();
            tryVerify(root.shows("Firefox Web Browser"));
            const field = root.all(loader.item, []).filter(i => i.placeholderText === "Filter by name")[0];
            verify(field !== undefined);
            field.text = "FIRE";
            tryVerify(() => root.findLabel(loader.item, "Notes") === null);
            verify(root.findLabel(loader.item, "Firefox Web Browser") !== null);
        }

        function test_cancel_is_silent() {
            tryVerify(root.shows("App caches"));
            fake.confirms = false;
            const button = root.rowButton("App caches", "Clean up");
            button.clicked();
            tryVerify(() => root.runs("clean").length === 1);
            tryVerify(root.idle);
            wait(50);
            const texts = root.all(loader.item, []).filter(i => typeof i.text === "string" && !root.isButton(i)).map(i => i.text);
            compare(texts.filter(t => /Cancel|Freed|wrong|administrator|Working/.test(t)), []);
            compare(root.runs("scan").length, 1, "nothing was read again");
            verify(root.rowButton("App caches", "Clean up").enabled);
        }

        function test_failed_write_shows_why() {
            const c = root.defaults();
            c["clean"] = { code: 1, error: "exit 1: refusing", output: { schema_version: 1, command: "clean", data: { plans: [], report: null }, errors: [{ source: "cli", message: "refusing: some sources failed" }] } };
            fake.commands = c;
            tryVerify(root.shows("App caches"));
            root.rowButton("App caches", "Clean up").clicked();
            tryVerify(root.shows("refusing: some sources failed"));
        }

        function test_failed_read_shows_the_error() {
            const c = root.defaults();
            c["scan"] = { error: "timed out" };
            fake.commands = c;
            loader.source = "";
            loader.source = root.pageUrl;
            loader.item.ctx = fake.ctx;
            tryVerify(root.shows("timed out"));
            verify(root.findLabel(loader.item, "Couldn't look for space you can free") !== null);
        }

        function test_untrusted_path_plain() {
            const path = "/home/user/.config/<i>x</i>‮evil";
            const c = root.defaults();
            c["orphans"] = { code: 0, output: { schema_version: 1, command: "orphans", errors: [], data: { orphans: [
                { path: path, kind: "config", confidence: "medium", reason: "<b>not</b> installed", shared: false, preselected: false, mime_lines: [], size: { bytes: 2048, files: 1, errors: 0 } }] } } };
            fake.commands = c;
            root.findButton(loader.item, "Find leftovers").clicked();
            tryVerify(root.shows(path));
            compare(root.findLabel(loader.item, path).textFormat, Text.PlainText);
            verify(root.findLabel(loader.item, "<b>not</b> installed") !== null);
            root.rowButton(path, "Move to trash").clicked();
            tryVerify(root.idle);
            compare(root.runs("orphan-trash"), [["run", "orphan-trash", [path]]]);
        }

        function test_one_write_at_a_time() {
            tryVerify(root.shows("App caches"));
            let held = null;
            const real = fake.ctx;
            loader.item.ctx = {
                theme: real.theme,
                command: {
                    run: (id, args, callback) => {
                        if (id === "clean") {
                            fake.calls.push(["run", id, args]);
                            held = callback;
                        } else {
                            real.command.run(id, args, callback);
                        }
                    },
                    watch: real.command.watch,
                    unwatch: real.command.unwatch
                },
                dbus: real.dbus,
                openApp: real.openApp
            };
            root.rowButton("App caches", "Clean up").clicked();
            verify(held !== null, "the cleanup is running");
            verify(root.findLabel(loader.item, "Working…") !== null);
            const buttons = root.findButtons(loader.item, "Clean up");
            verify(buttons.length > 1);
            for (const b of buttons)
                compare(b.enabled, false);
            // a second click, if it got through, must not start a second write
            root.rowButton("Thumbnails", "Clean up").clicked();
            compare(root.runs("clean").length, 1);
            held(root.answer("clean-ok"));
            tryVerify(root.idle);
            tryVerify(root.shows("Freed 1.2 GB"));
            verify(root.rowButton("Thumbnails", "Clean up").enabled);
        }

        function test_history_put_back() {
            tryVerify(root.shows("Cleanup"));
            compare(root.rowButton("Cleanup", "Put back").visible, true);
            compare(root.findButton(root.findLabel(loader.item, "Removed oldtool").parent, "Put back").visible, false);
            root.rowButton("Cleanup", "Put back").clicked();
            tryVerify(root.shows("Put back 1 item"));
            tryVerify(root.idle);
            compare(root.runs("restore"), [["run", "restore", ["1794141850000"]]]);
        }

        function test_a_change_to_the_history_reads_it_again() {
            tryVerify(root.shows("Cleanup"));
            const c = root.defaults();
            c["history"] = { code: 0, output: { schema_version: 1, command: "history", errors: [], data: { batches: [] } } };
            fake.commands = c;
            fake.change("history");
            tryVerify(root.shows("Nothing cleaned up yet."));
        }

        function test_search_highlight_reaches_the_rows() {
            tryVerify(root.shows("App caches"));
            loader.item.highlight = "apps";
            const rows = root.all(loader.item, []).filter(i => i.settingId === "apps");
            compare(rows.length, 1);
            compare(rows[0].flashing, true);
        }
    }
}
