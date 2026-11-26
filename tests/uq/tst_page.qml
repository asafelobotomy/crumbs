// The Storage and cleanup page against a fake ctx (FakeContext.qml): what it asks crumbs for, what
// it shows, and what it never does. Offscreen:
//   QML_XHR_ALLOW_FILE_READ=1 QT_QPA_PLATFORM=offscreen qmltestrunner -import tests/uq/stub -input tests/uq
// QML_XHR_ALLOW_FILE_READ lets fixture() read the files in fixtures/.
import QtQuick
import QtTest

StorageHarness {
    id: root

    StorageCase {
        name: "StoragePage"
        harness: root

        function test_scan_on_open_only() {
            tryVerify(root.shows("App caches"));
            tryVerify(root.shows("Cleanup"));
            compare(root.runs().map(c => c[1]).sort(), ["history", "scan"]);
            verify(root.fake.calls.some(c => c[0] === "watch" && c[1] === "history"), "history is followed, not polled");
            compare(root.runs("scan"), [["run", "scan", []]]);
        }

        function test_rows_say_what_they_are() {
            tryVerify(root.shows("App caches"));
            verify(root.findLabel(root.loader.item, "1.5 GB · 2 items") !== null);
            verify(root.findLabel(root.loader.item, "Deleted for good") !== null);
            verify(root.findLabel(root.loader.item, "Not on this computer: Unused container images and volumes") !== null);
            verify(root.findLabel(root.loader.item, "Developer caches") === null, "a category that is switched off has no row");
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

            root.findButton(root.loader.item, "Check system space").clicked();
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
            root.fake.commands = c;
            root.findButton(root.loader.item, "Check system space").clicked();
            tryVerify(root.shows("System logs"));
            rowButton("System logs", "Clean up…").clicked();
            tryVerify(root.shows("Couldn't get administrator rights"));
            const manual = "sudo '/usr/bin/journalctl' '--vacuum-size=500M'";
            verify(root.findLabel(root.loader.item, manual) !== null);
            compare(root.findLabel(root.loader.item, manual).visible, false, "the commands are under Advanced details");
            // the System logs row's own button (every row has a hidden one)
            // (the pie's legend says "System logs" too; the row's label is the one with a button)
            const details = root.findLabels(root.loader.item, "System logs").map(l => root.findButton(l.parent.parent.parent.parent, "Advanced details")).find(b => b !== null);
            verify(details !== undefined);
            details.clicked();
            compare(root.findLabel(root.loader.item, manual).visible, true);
        }

        function test_filter_matches_the_name() {
            root.openTab("Apps");
            tryVerify(root.shows("Firefox Web Browser"));
            const field = root.all(root.loader.item, []).filter(i => i.placeholderText === "Filter by name")[0];
            verify(field !== undefined);
            field.text = "FIRE";
            tryVerify(() => root.findLabel(root.loader.item, "Notes") === null);
            verify(root.findLabel(root.loader.item, "Firefox Web Browser") !== null);
        }

        function test_cancel_is_silent() {
            tryVerify(root.shows("App caches"));
            root.fake.confirms = false;
            const button = rowButton("App caches", "Clean up");
            button.clicked();
            tryVerify(() => root.runs("clean").length === 1);
            tryVerify(root.idle);
            wait(50);
            const texts = root.all(root.loader.item, []).filter(i => typeof i.text === "string" && !root.isButton(i)).map(i => i.text);
            compare(texts.filter(t => /Cancel|Freed|wrong|administrator|Working/.test(t)), []);
            compare(root.runs("scan").length, 1, "nothing was read again");
            verify(rowButton("App caches", "Clean up").enabled);
        }

        function test_failed_write_shows_why() {
            const c = root.defaults();
            c["clean"] = { code: 1, error: "exit 1: refusing", output: { schema_version: 1, command: "clean", data: { plans: [], report: null }, errors: [{ source: "cli", message: "refusing: some sources failed" }] } };
            root.fake.commands = c;
            tryVerify(root.shows("App caches"));
            rowButton("App caches", "Clean up").clicked();
            tryVerify(root.shows("refusing: some sources failed"));
        }

        function test_failed_read_shows_the_error() {
            const c = root.defaults();
            c["scan"] = { error: "timed out" };
            root.fake.commands = c;
            root.loader.source = "";
            root.loader.source = root.pageUrl;
            root.loader.item.ctx = root.fake.ctx;
            tryVerify(root.shows("timed out"));
            verify(root.findLabel(root.loader.item, "Couldn't look for space you can free") !== null);
        }

        function test_untrusted_path_plain() {
            const path = "/home/user/.config/<i>x</i>‮evil";
            const c = root.defaults();
            c["orphans"] = { code: 0, output: { schema_version: 1, command: "orphans", errors: [], data: { orphans: [
                { path: path, kind: "config", confidence: "medium", reason: "<b>not</b> installed", shared: false, preselected: false, mime_lines: [], size: { bytes: 2048, files: 1, errors: 0 } }] } } };
            root.fake.commands = c;
            root.findButton(root.loader.item, "Find leftovers").clicked();
            tryVerify(root.shows(path));
            compare(root.findLabel(root.loader.item, path).textFormat, Text.PlainText);
            verify(root.findLabel(root.loader.item, "<b>not</b> installed") !== null);
            rowButton(path, "Move to trash").clicked();
            tryVerify(root.idle);
            compare(root.runs("orphan-trash"), [["run", "orphan-trash", [path]]]);
        }

        function test_one_write_at_a_time() {
            tryVerify(root.shows("App caches"));
            root.fake.hold = ["clean"];
            rowButton("App caches", "Clean up").clicked();
            verify(root.fake.held.clean !== undefined, "the cleanup is running");
            verify(root.findLabel(root.loader.item, "Working…") !== null);
            const buttons = root.findButtons(root.loader.item, "Clean up");
            verify(buttons.length > 1);
            for (const b of buttons)
                compare(b.enabled, false);
            // a second click, if it got through, must not start a second write
            rowButton("Thumbnails", "Clean up").clicked();
            compare(root.runs("clean").length, 1);
            root.fake.held.clean(root.answer("clean-ok"));
            tryVerify(root.idle);
            tryVerify(root.shows("Freed 1.2 GB"));
            verify(rowButton("Thumbnails", "Clean up").enabled);
        }

        function test_a_clean_that_ends_during_a_scan_scans_again() {
            tryVerify(root.shows("App caches"));
            tryVerify(root.idle);
            const before = root.runs("scan").length;
            root.fake.hold = ["scan", "clean"];
            rowButton("App caches", "Clean up").clicked();
            verify(root.fake.held.clean !== undefined, "the cleanup is running");
            root.findButton(root.loader.item, "Refresh").clicked();
            verify(root.fake.held.scan !== undefined, "a scan started before the cleanup ended");
            const early = root.fake.held.scan;
            root.fake.held.clean(root.answer("clean-ok"));
            // the re-read after the cleanup waits for the scan in flight, then scans once more
            wait(50);
            compare(root.runs("scan").length, before + 1);
            root.fake.hold = [];
            early(root.answer("scan"));
            tryVerify(() => root.runs("scan").length === before + 2);
        }

        function test_history_put_back() {
            root.openTab("History");
            tryVerify(root.shows("Cleanup"));
            compare(rowButton("Cleanup", "Put back").visible, true);
            compare(root.findButton(root.findLabel(root.loader.item, "Removed oldtool").parent, "Put back").visible, false);
            rowButton("Cleanup", "Put back").clicked();
            tryVerify(root.shows("Put back 1 item"));
            tryVerify(root.idle);
            compare(root.runs("restore"), [["run", "restore", ["1794141850000"]]]);
        }

        function test_a_change_to_the_history_reads_it_again() {
            tryVerify(root.shows("Cleanup"));
            const c = root.defaults();
            c["history"] = { code: 0, output: { schema_version: 1, command: "history", errors: [], data: { batches: [] } } };
            root.fake.commands = c;
            root.fake.change("history");
            tryVerify(root.shows("Nothing cleaned up yet."));
        }

        function test_shown_again_reads_again() {
            tryVerify(root.shows("App caches"));
            tryVerify(() => root.runs("scan").length === 1);
            root.loader.item.refresh();
            tryVerify(() => root.runs("scan").length === 2);
            compare(root.runs("apps").length, 0, "apps not listed yet are not read");
            root.openTab("Apps");
            tryVerify(() => root.runs("apps").length === 1);
            tryVerify(root.shows("Firefox Web Browser"));
            root.loader.item.refresh();
            tryVerify(() => root.runs("apps").length === 2);
            tryVerify(() => root.runs("scan").length === 3);
            compare(root.runs("scan-system").length, 0, "the system is still read only when asked");
        }

        function test_overview_pie_shares_out_the_space() {
            tryVerify(root.shows("App caches"));
            const pie = root.all(root.loader.item, []).find(i => i.slices !== undefined && i.hovered !== undefined);
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
            root.findButton(root.loader.item, "Find system leftovers").clicked();
            tryVerify(root.shows("/opt/oldtool"));
            rowButton("/opt/oldtool", "Delete…").clicked();
            tryVerify(root.idle);
            compare(root.runs("orphan-delete-system"), [["run", "orphan-delete-system", ["/opt/oldtool"]]]);
        }

        function test_preferences_follow_the_file_and_change_one_key() {
            compare(root.fake.calls.filter(c => c[0] === "watch" && c[1] === "config").length, 0);
            root.openTab("Preferences");
            tryVerify(() => root.fake.calls.some(c => c[0] === "watch" && c[1] === "config"));
            tryVerify(root.shows("Saved in ~/.config/crumbs/config.toml"));
            verify(root.findLabel(root.loader.item, "~/Games") !== null);
            const devCache = root.all(root.loader.item, []).filter(i => i.text === "Developer caches" && i.checked !== undefined)[0];
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
            root.loader.item.highlight = "apps";
            const rows = root.all(root.loader.item, []).filter(i => i.settingId === "apps");
            compare(rows.length, 1);
            compare(rows[0].flashing, true);
            root.loader.item.highlight = "preferences";
            compare(root.all(root.loader.item, []).filter(i => i.settingId === "preferences" && i.flashing).length, 1);
        }
    }
}
