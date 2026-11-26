// Uninstalling from the Storage and cleanup page against a fake ctx: the preview, the dialog, the
// queue, refusals and what went. Run like tst_page.qml (see there).
import QtQuick
import QtTest

StorageHarness {
    id: root

    StorageCase {
        name: "StorageUninstall"
        harness: root

        function test_uninstall_refused_running() {
            const c = root.defaults();
            c["uninstall"] = { code: 1, error: "exit 1: Firefox Web Browser is running", output: root.fixture("uninstall-running") };
            root.fake.commands = c;
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
            root.fake.commands = c;
            root.openTab("Apps");
            tryVerify(root.shows("Firefox Web Browser"));
            uninstallFirefox();
            tryVerify(inDialog("Some items couldn't be removed · freed 1.2 GB"));
            verify(inDialog("/home/user/.cache/foo\n/home/user/.cache/bar")(), "what was removed is listed");
            verify(inDialog("/home/user/.mozilla: busy")(), "what stayed is listed, with why");
            compare(root.runs("apps").length, 1, "the list waits for OK");
            dialogButton("OK").clicked();
            tryVerify(() => !appsSection().dialog.visible);
            tryVerify(() => root.findLabel(root.loader.item, "Firefox Web Browser") === null);
        }

        function test_part_of_uq_has_no_button() {
            root.openTab("Apps");
            const partOf = () => root.all(root.loader.item, []).find(i => typeof i.text === "string" && i.text.startsWith("Part of uQ") && !root.isButton(i));
            tryVerify(() => partOf() !== undefined);
            const label = partOf();
            verify(root.findButton(label.parent.parent, "Uninstall…") === null || root.findButton(label.parent.parent, "Uninstall…").visible === false);
            compare(root.findButtons(root.loader.item, "Uninstall…").filter(b => b.visible).length, 4, "the other four apps can be uninstalled");
        }

        function test_an_uninstall_says_where_the_password_goes() {
            root.fake.hold = ["uninstall"];
            root.openTab("Apps");
            tryVerify(root.shows("Firefox Web Browser"));
            uninstallFirefox();
            verify(root.fake.held.uninstall !== undefined, "the uninstall is running");
            tryVerify(inDialog("Uninstalling Firefox Web Browser…"));
            verify(inDialog("Working… If a password dialog opened, enter your password there.")());
            verify(root.all(dialogBody(), []).some(i => i.indeterminate === true && i.visible), "a moving progress bar");
            compare(root.findButtons(dialogBody(), "OK").filter(b => b.visible).length, 0, "no way to close it while it runs");
            root.fake.held.uninstall({ code: 5, error: "exit 5", output: { schema_version: 1, command: "uninstall", errors: [{ source: "cli", message: "administrator rights were not granted; nothing that needs them was changed" }], data: { plans: [], report: null, manual_commands: [] } } });
            tryVerify(inDialog("Couldn't get administrator rights"));
            tryVerify(root.idle);
        }

        function test_apps_load_when_their_tab_opens() {
            tryVerify(root.shows("App caches"));
            compare(root.runs("apps").length, 0);
            root.openTab("Apps");
            tryVerify(() => root.runs("apps").length === 1);
            tryVerify(root.shows("Firefox Web Browser"));
            const icons = root.all(root.loader.item, []).filter(i => i.label === "Firefox Web Browser" && i.found !== undefined);
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
            // the sidebar: three ways, each with what it means; the chosen one is highlighted
            for (const t of ["Uninstall…", "Everything", "The app, its settings, data and caches", "Keep Settings & Data", "Leftovers", "Only what it left behind; the app stays installed"])
                verify(inDialog(t)(), t);
            verify(inDialog("/home/user/.cache/mozilla")(), "the files are listed without unfolding anything");
            const keep = root.findButtons(dialogBody(), "Keep Settings & Data").find(i => i.visible);
            verify(keep !== undefined);
            compare(keep.highlighted, false);
            keep.clicked();
            tryVerify(() => keep.highlighted);
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
            const boxes = root.all(root.loader.item, []).filter(i => i.checkable === true && i.visible && /^Add .* to the apps to uninstall$/.test(i.Accessible.name || ""));
            compare(boxes.length, 4, "every app crumBS may remove has a tick box; Part of uQ has none");
            for (const name of ["Firefox Web Browser", "Notes"]) {
                const box = boxes.find(b => b.Accessible.name === "Add " + name + " to the apps to uninstall");
                box.toggle();
                box.toggled();
            }
            tryVerify(root.shows("2 apps selected · 413.0 MB"));
            root.fake.hold = ["uninstall"];
            rowButton("2 apps selected · 413.0 MB", "Uninstall selected…").clicked();
            tryVerify(() => appsSection().dialog.opened);
            verify(inDialog("Uninstall 2 apps")());
            dialogButton("Uninstall 2 apps").clicked();
            tryVerify(() => root.fake.held.uninstall !== undefined);
            verify(inDialog("Uninstalling Firefox Web Browser (1 of 2)…")());
            verify(inDialog("Waiting")(), "Notes waits its turn");
            const first = root.fake.held.uninstall;
            root.fake.held = {};
            first(root.answer("clean-ok"));
            tryVerify(() => root.fake.held.uninstall !== undefined);
            tryVerify(inDialog("Uninstalling Notes (2 of 2)…"));
            verify(inDialog("Removed · freed 1.2 GB")(), "Firefox says how it went");
            root.fake.held.uninstall(root.answer("clean-ok"));
            tryVerify(inDialog("Uninstalled 2 of 2 apps · freed 2.4 GB"));
            compare(root.runs("uninstall"), [["run", "uninstall", ["pacman:firefox"]], ["run", "uninstall", ["flatpak:user/org.example.Notes"]]]);
            dialogButton("OK").clicked();
            tryVerify(() => root.runs("apps").length === 2);
            tryVerify(() => root.findLabel(root.loader.item, "2 apps selected · 413.0 MB") === null);
        }

        function test_a_queue_goes_on_when_one_is_declined_and_can_stop() {
            root.openTab("Apps");
            tryVerify(root.shows("Firefox Web Browser"));
            const section = appsSection();
            section.selected = { "pacman:firefox": true, "flatpak:user/org.example.Notes": true, "appimage:~/Downloads/GutCheck-0.19.0-x86_64.AppImage": true };
            root.fake.confirms = false;
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
            root.fake.confirms = true;
            root.fake.hold = ["uninstall"];
            section.dialog.openFor(section.queue);
            tryVerify(() => section.dialog.opened);
            dialogButton("Uninstall 3 apps").clicked();
            tryVerify(() => root.fake.held.uninstall !== undefined);
            dialogButton("Stop after this one").clicked();
            root.fake.held.uninstall(root.answer("clean-ok"));
            tryVerify(inDialog("Uninstalled 1 of 3 apps · freed 1.2 GB"));
            compare(root.findLabels(dialogBody(), "Skipped: the queue was stopped").length, 2);
            compare(root.runs("uninstall").length, 4);
        }
    }
}
