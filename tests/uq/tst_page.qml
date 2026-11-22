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

    TestCase {
        name: "StoragePage"
        when: windowShown

        // The button that sits in the same row as the label `labelText`.
        function rowButton(labelText, buttonText) {
            const labels = root.findLabels(loader.item, labelText);
            verify(labels.length > 0, "a row labelled " + labelText);
            // An app's button is in its tile's row, one level above the label's own column. The
            // pie's legend repeats a category's name, without a button.
            let button = null;
            for (const label of labels)
                button = button || root.findButton(label.parent, buttonText) || root.findButton(label.parent.parent, buttonText);
            verify(button !== null, labelText + " has a " + buttonText + " button");
            return button;
        }

        function appsSection() {
            return root.all(loader.item, []).find(i => i.dialog !== undefined && i.queue !== undefined);
        }

        /// The uninstall dialog's content (a popup, so not under the page's own items).
        function dialogBody() {
            return appsSection().dialog.contentItem;
        }

        function inDialog(text) {
            return () => root.findLabels(dialogBody(), text).some(l => l.visible);
        }

        function dialogButton(text) {
            const b = root.findButtons(dialogBody(), text).find(i => i.visible);
            verify(b !== undefined, "the dialog has a visible " + text + " button");
            return b;
        }

        /// Opens Firefox's uninstall dialog, waits for the plan, and presses its Uninstall.
        function uninstallFirefox() {
            rowButton("Firefox Web Browser", "Uninstall…").clicked();
            tryVerify(() => appsSection().dialog.opened);
            tryVerify(inDialog("Frees about 362.0 MB"));
            const go = dialogButton("Uninstall");
            tryVerify(() => go.enabled);
            go.clicked();
        }

        function init() {
            fake.calls = [];
            fake.confirms = true;
            fake.hold = [];
            fake.held = {};
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
            rowButton("Browser caches", "Clean up").clicked();
            tryVerify(root.shows("Freed 1.2 GB"));
            tryVerify(root.idle);
            compare(root.runs("clean"), [["run", "clean", ["browser-cache"]]]);

            tryVerify(root.shows("Trash"));
            rowButton("Trash", "Clean up").clicked();
            tryVerify(root.idle);
            compare(root.runs("clean-for-good"), [["run", "clean-for-good", ["trash"]]]);

            root.findButton(loader.item, "Check system space").clicked();
            tryVerify(root.shows("System logs"));
            rowButton("System logs", "Clean up…").clicked();
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
            rowButton("System logs", "Clean up…").clicked();
            tryVerify(root.shows("Couldn't get administrator rights"));
            const manual = "sudo '/usr/bin/journalctl' '--vacuum-size=500M'";
            verify(root.findLabel(loader.item, manual) !== null);
            compare(root.findLabel(loader.item, manual).visible, false, "the commands are under Advanced details");
            // the System logs row's own button (every row has a hidden one)
            // (the pie's legend says "System logs" too; the row's label is the one with a button)
            const details = root.findLabels(loader.item, "System logs").map(l => root.findButton(l.parent.parent.parent.parent, "Advanced details")).find(b => b !== null);
            verify(details !== undefined);
            details.clicked();
            compare(root.findLabel(loader.item, manual).visible, true);
        }

        function test_uninstall_refused_running() {
            const c = root.defaults();
            c["uninstall"] = { code: 1, error: "exit 1: Firefox Web Browser is running", output: root.fixture("uninstall-running") };
            fake.commands = c;
            root.openTab("Apps");
            tryVerify(root.shows("Firefox Web Browser"));
            uninstallFirefox();
            tryVerify(inDialog("Firefox Web Browser is running (pid 4242). Close it, or let crumBS ask it to close."));
            compare(root.runs("uninstall"), [["run", "uninstall", ["pacman:firefox"]]]);
            tryVerify(root.idle);
            tryVerify(inDialog("Nothing was uninstalled"));
            // The dialog stays open, with a button that closes the app first.
            dialogButton("Close it and uninstall").clicked();
            tryVerify(() => root.runs("uninstall-close").length === 1);
            compare(root.runs("uninstall-close"), [["run", "uninstall-close", ["pacman:firefox"]]]);
            tryVerify(root.idle);
            tryVerify(inDialog("Removed · freed 1.2 GB"));
        }

        function test_partial_uninstall_lists_what_went_and_what_stayed() {
            let listed = 0;
            const c = root.defaults();
            c["apps"] = () => {
                listed++;
                const r = root.answer("apps");
                if (listed > 1)
                    r.output.data.apps = r.output.data.apps.filter(a => a.id !== "pacman:firefox");
                return r;
            };
            const partial = root.fixture("clean-ok");
            partial.data.report[0].results.push({ action: { path: "/home/user/.mozilla", type: "trash" }, error: "busy", history_id: "1-2", ok: false });
            c["uninstall"] = { code: 3, error: "exit 3: some failed", output: partial };
            fake.commands = c;
            root.openTab("Apps");
            tryVerify(root.shows("Firefox Web Browser"));
            uninstallFirefox();
            tryVerify(inDialog("Some items couldn't be removed · freed 1.2 GB"));
            verify(inDialog("/home/user/.cache/foo\n/home/user/.cache/bar")(), "what was removed is listed");
            verify(inDialog("/home/user/.mozilla: busy")(), "what stayed is listed, with why");
            compare(root.runs("apps").length, 1, "the list waits for OK");
            dialogButton("OK").clicked();
            tryVerify(() => !appsSection().dialog.visible);
            tryVerify(() => root.findLabel(loader.item, "Firefox Web Browser") === null);
        }

        function test_part_of_uq_has_no_button() {
            root.openTab("Apps");
            const partOf = () => root.all(loader.item, []).find(i => typeof i.text === "string" && i.text.startsWith("Part of uQ") && !root.isButton(i));
            tryVerify(() => partOf() !== undefined);
            const label = partOf();
            verify(root.findButton(label.parent.parent, "Uninstall…") === null || root.findButton(label.parent.parent, "Uninstall…").visible === false);
            compare(root.findButtons(loader.item, "Uninstall…").filter(b => b.visible).length, 4, "the other four apps can be uninstalled");
        }

        function test_filter_matches_the_name() {
            root.openTab("Apps");
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
            const button = rowButton("App caches", "Clean up");
            button.clicked();
            tryVerify(() => root.runs("clean").length === 1);
            tryVerify(root.idle);
            wait(50);
            const texts = root.all(loader.item, []).filter(i => typeof i.text === "string" && !root.isButton(i)).map(i => i.text);
            compare(texts.filter(t => /Cancel|Freed|wrong|administrator|Working/.test(t)), []);
            compare(root.runs("scan").length, 1, "nothing was read again");
            verify(rowButton("App caches", "Clean up").enabled);
        }

        function test_failed_write_shows_why() {
            const c = root.defaults();
            c["clean"] = { code: 1, error: "exit 1: refusing", output: { schema_version: 1, command: "clean", data: { plans: [], report: null }, errors: [{ source: "cli", message: "refusing: some sources failed" }] } };
            fake.commands = c;
            tryVerify(root.shows("App caches"));
            rowButton("App caches", "Clean up").clicked();
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
            rowButton(path, "Move to trash").clicked();
            tryVerify(root.idle);
            compare(root.runs("orphan-trash"), [["run", "orphan-trash", [path]]]);
        }

        function test_an_uninstall_says_where_the_password_goes() {
            fake.hold = ["uninstall"];
            root.openTab("Apps");
            tryVerify(root.shows("Firefox Web Browser"));
            uninstallFirefox();
            verify(fake.held.uninstall !== undefined, "the uninstall is running");
            tryVerify(inDialog("Uninstalling Firefox Web Browser…"));
            verify(inDialog("Working… If a password dialog opened, enter your password there.")());
            verify(root.all(dialogBody(), []).some(i => i.indeterminate === true && i.visible), "a moving progress bar");
            compare(root.findButtons(dialogBody(), "OK").filter(b => b.visible).length, 0, "no way to close it while it runs");
            fake.held.uninstall({ code: 5, error: "exit 5", output: { schema_version: 1, command: "uninstall", errors: [{ source: "cli", message: "administrator rights were not granted; nothing that needs them was changed" }], data: { plans: [], report: null, manual_commands: [] } } });
            tryVerify(inDialog("Couldn't get administrator rights"));
            tryVerify(root.idle);
        }

        function test_one_write_at_a_time() {
            tryVerify(root.shows("App caches"));
            fake.hold = ["clean"];
            rowButton("App caches", "Clean up").clicked();
            verify(fake.held.clean !== undefined, "the cleanup is running");
            verify(root.findLabel(loader.item, "Working…") !== null);
            const buttons = root.findButtons(loader.item, "Clean up");
            verify(buttons.length > 1);
            for (const b of buttons)
                compare(b.enabled, false);
            // a second click, if it got through, must not start a second write
            rowButton("Thumbnails", "Clean up").clicked();
            compare(root.runs("clean").length, 1);
            fake.held.clean(root.answer("clean-ok"));
            tryVerify(root.idle);
            tryVerify(root.shows("Freed 1.2 GB"));
            verify(rowButton("Thumbnails", "Clean up").enabled);
        }

        function test_a_clean_that_ends_during_a_scan_scans_again() {
            tryVerify(root.shows("App caches"));
            tryVerify(root.idle);
            const before = root.runs("scan").length;
            fake.hold = ["scan", "clean"];
            rowButton("App caches", "Clean up").clicked();
            verify(fake.held.clean !== undefined, "the cleanup is running");
            root.findButton(loader.item, "Refresh").clicked();
            verify(fake.held.scan !== undefined, "a scan started before the cleanup ended");
            const early = fake.held.scan;
            fake.held.clean(root.answer("clean-ok"));
            // the re-read after the cleanup waits for the scan in flight, then scans once more
            wait(50);
            compare(root.runs("scan").length, before + 1);
            fake.hold = [];
            early(root.answer("scan"));
            tryVerify(() => root.runs("scan").length === before + 2);
        }

        function test_history_put_back() {
            root.openTab("History");
            tryVerify(root.shows("Cleanup"));
            compare(rowButton("Cleanup", "Put back").visible, true);
            compare(root.findButton(root.findLabel(loader.item, "Removed oldtool").parent, "Put back").visible, false);
            rowButton("Cleanup", "Put back").clicked();
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

        function test_apps_load_when_their_tab_opens() {
            tryVerify(root.shows("App caches"));
            compare(root.runs("apps").length, 0);
            root.openTab("Apps");
            tryVerify(() => root.runs("apps").length === 1);
            tryVerify(root.shows("Firefox Web Browser"));
            const icons = root.all(loader.item, []).filter(i => i.label === "Firefox Web Browser" && i.found !== undefined);
            compare(icons.length, 1, "the app has an icon");
            compare(icons[0].name, "firefox");
            // opening the tab again doesn't list them again
            root.openTab("Overview");
            root.openTab("Apps");
            wait(50);
            compare(root.runs("apps").length, 1);
        }

        function test_uninstall_preview_says_what_goes_before_anything_goes() {
            root.openTab("Apps");
            tryVerify(root.shows("Firefox Web Browser"));
            rowButton("Firefox Web Browser", "Uninstall…").clicked();
            tryVerify(inDialog("Frees about 362.0 MB"));
            compare(root.runs("preview-uninstall"), [["run", "preview-uninstall", ["pacman:firefox"]]]);
            for (const t of ["Uninstall Firefox Web Browser", "The app itself", "Your files and data", "Caches", "Menu entries, icons and file types", "Removed for good"])
                verify(inDialog(t)(), t);
            compare(root.runs("uninstall").length, 0, "nothing is removed by looking");
            // keep settings and data: a new plan, and that choice is what runs
            const keep = root.all(dialogBody(), []).filter(i => i.text === "The app and its caches; keep my settings and data" && i.visible)[0];
            verify(keep !== undefined);
            keep.clicked();
            tryVerify(() => root.runs("preview-keep-data").length === 1);
            tryVerify(inDialog("Frees about 362.0 MB"));
            tryVerify(() => dialogButton("Uninstall").enabled);
            dialogButton("Uninstall").clicked();
            tryVerify(root.idle);
            compare(root.runs("uninstall-keep-data"), [["run", "uninstall-keep-data", ["pacman:firefox"]]]);
            compare(root.runs("uninstall").length, 0);
            tryVerify(inDialog("Done · freed 1.2 GB"));
            // Cancel on a fresh dialog changes nothing.
            dialogButton("OK").clicked();
            tryVerify(() => !appsSection().dialog.visible);
        }

        function test_selected_apps_are_uninstalled_one_after_another() {
            root.openTab("Apps");
            tryVerify(root.shows("Firefox Web Browser"));
            const boxes = root.all(loader.item, []).filter(i => i.checkable === true && i.visible && /^Add .* to the apps to uninstall$/.test(i.Accessible.name || ""));
            compare(boxes.length, 4, "every app crumBS may remove has a tick box; Part of uQ has none");
            for (const name of ["Firefox Web Browser", "Notes"]) {
                const box = boxes.find(b => b.Accessible.name === "Add " + name + " to the apps to uninstall");
                box.toggle();
                box.toggled();
            }
            tryVerify(root.shows("2 apps selected · 413.0 MB"));
            fake.hold = ["uninstall"];
            rowButton("2 apps selected · 413.0 MB", "Uninstall selected…").clicked();
            tryVerify(() => appsSection().dialog.opened);
            verify(inDialog("Uninstall 2 apps")());
            dialogButton("Uninstall 2 apps").clicked();
            tryVerify(() => fake.held.uninstall !== undefined);
            verify(inDialog("Uninstalling Firefox Web Browser (1 of 2)…")());
            verify(inDialog("Waiting")(), "Notes waits its turn");
            const first = fake.held.uninstall;
            fake.held = {};
            first(root.answer("clean-ok"));
            tryVerify(() => fake.held.uninstall !== undefined);
            tryVerify(inDialog("Uninstalling Notes (2 of 2)…"));
            verify(inDialog("Removed · freed 1.2 GB")(), "Firefox says how it went");
            fake.held.uninstall(root.answer("clean-ok"));
            tryVerify(inDialog("Uninstalled 2 of 2 apps · freed 2.4 GB"));
            compare(root.runs("uninstall"), [["run", "uninstall", ["pacman:firefox"]], ["run", "uninstall", ["flatpak:user/org.example.Notes"]]]);
            dialogButton("OK").clicked();
            tryVerify(() => root.runs("apps").length === 2);
            tryVerify(() => root.findLabel(loader.item, "2 apps selected · 413.0 MB") === null);
        }

        function test_a_queue_goes_on_when_one_is_declined_and_can_stop() {
            root.openTab("Apps");
            tryVerify(root.shows("Firefox Web Browser"));
            const section = appsSection();
            section.selected = { "pacman:firefox": true, "flatpak:user/org.example.Notes": true, "appimage:~/Downloads/GutCheck-0.19.0-x86_64.AppImage": true };
            fake.confirms = false;
            section.dialog.openFor(section.queue);
            tryVerify(() => section.dialog.opened);
            dialogButton("Uninstall 3 apps").clicked();
            tryVerify(inDialog("Uninstalled 0 of 3 apps · freed 0 B"));
            compare(root.runs("uninstall").length, 3, "a no for one app doesn't stop the others");
            verify(inDialog("Not uninstalled: you said no.")());
            dialogButton("OK").clicked();
            tryVerify(() => !section.dialog.visible);
            compare(root.runs("apps").length, 1, "nothing changed, nothing is read again");
            // Stop after this one: the app that runs finishes, the rest are skipped.
            fake.confirms = true;
            fake.hold = ["uninstall"];
            section.dialog.openFor(section.queue);
            tryVerify(() => section.dialog.opened);
            dialogButton("Uninstall 3 apps").clicked();
            tryVerify(() => fake.held.uninstall !== undefined);
            dialogButton("Stop after this one").clicked();
            fake.held.uninstall(root.answer("clean-ok"));
            tryVerify(inDialog("Uninstalled 1 of 3 apps · freed 1.2 GB"));
            compare(root.findLabels(dialogBody(), "Skipped: the queue was stopped").length, 2);
            compare(root.runs("uninstall").length, 4);
        }

        function test_overview_pie_shares_out_the_space() {
            tryVerify(root.shows("App caches"));
            const pie = root.all(loader.item, []).find(i => i.slices !== undefined && i.hovered !== undefined);
            verify(pie !== undefined && pie.visible, "the overview has a pie");
            compare(pie.slices.length, 7, "six categories and Everything else");
            compare(pie.slices[0].label, "App caches");
            compare(pie.slices[6].label, "Everything else");
            verify(root.findLabel(pie, "1.5 GB · 56%") !== null, "the legend says size and share");
            verify(root.findLabel(pie, "2.6 GB") !== null, "the middle says the total");
            pie.hovered = 1;
            tryVerify(() => root.findLabel(pie, "Browser caches") !== null && root.findLabels(pie, "730.0 MB").length === 1);
        }

        function test_system_leftovers_are_deleted_only_when_chosen() {
            root.openTab("Leftovers");
            tryVerify(() => root.runs("orphans").length === 1, 5000, "user leftovers load with the tab");
            compare(root.runs("orphans-system").length, 0, "system leftovers wait to be asked");
            root.findButton(loader.item, "Find system leftovers").clicked();
            tryVerify(root.shows("/opt/oldtool"));
            rowButton("/opt/oldtool", "Delete…").clicked();
            tryVerify(root.idle);
            compare(root.runs("orphan-delete-system"), [["run", "orphan-delete-system", ["/opt/oldtool"]]]);
        }

        function test_preferences_follow_the_file_and_change_one_key() {
            compare(fake.calls.filter(c => c[0] === "watch" && c[1] === "config").length, 0);
            root.openTab("Preferences");
            tryVerify(() => fake.calls.some(c => c[0] === "watch" && c[1] === "config"));
            tryVerify(root.shows("Saved in ~/.config/crumbs/config.toml"));
            verify(root.findLabel(loader.item, "~/Games") !== null);
            const devCache = root.all(loader.item, []).filter(i => i.text === "Developer caches" && i.checked !== undefined)[0];
            verify(devCache !== undefined);
            compare(devCache.checked, false, "switched off by default");
            devCache.toggle();
            devCache.toggled();
            tryVerify(root.idle);
            compare(root.runs("config-remove"), [["run", "config-remove", ["categories_off", "dev-cache"]]]);
            rowButton("~/Games", "Remove").clicked();
            tryVerify(root.idle);
            compare(root.runs("config-remove").length, 2);
            compare(root.runs("config-remove")[1], ["run", "config-remove", ["protect", "~/Games"]]);
        }

        function test_search_highlight_reaches_the_rows() {
            tryVerify(root.shows("App caches"));
            loader.item.highlight = "apps";
            const rows = root.all(loader.item, []).filter(i => i.settingId === "apps");
            compare(rows.length, 1);
            compare(rows[0].flashing, true);
            loader.item.highlight = "preferences";
            compare(root.all(loader.item, []).filter(i => i.settingId === "preferences" && i.flashing).length, 1);
        }
    }
}
