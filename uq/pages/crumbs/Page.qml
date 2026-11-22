// Storage and cleanup: what crumBS finds, and what the person chooses to remove. Everything goes
// through the crumbs command line (page.toml); logic.js reads its answers and words the result.
// The page never forces a removal: crumBS's own refusals are shown as they are.
// Laid out like Settings' own pages (uq_api = 2): five tabs of cards.
import QtQuick
import QtQuick.Controls as QQC2
import QtQuick.Layouts
import org.uq.settings
import "logic.js" as Logic

SettingsPage {
    id: page

    property var ctx
    property var theme
    /// The [[setting]] row Settings search jumped to; the rows flash, and their tab opens.
    property string highlight
    /// The key of the row whose write is running, "" when none. Only one write runs at a time.
    property string workingKey: ""
    /// The command id of the write that is running, "" when none.
    property string workingCommand: ""
    readonly property bool busy: page.workingKey !== ""
    /// What each write said, by row key: {message, manual, detail, failed}.
    property var notes: ({})
    property var openedWith: null

    pageId: "crumbs"

    function noteFor(key) {
        return page.notes[key] || null;
    }

    function errorColor() {
        const colors = page.ctx && page.ctx.theme ? page.ctx.theme.colors : null;
        return (colors && colors.error) || "red";
    }

    function accentColor() {
        const colors = page.ctx && page.ctx.theme ? page.ctx.theme.colors : null;
        return (colors && colors.accent) || "steelblue";
    }

    /// Runs one write (a confirmed command) for the row `key`, then `done(result)` unless the
    /// person cancelled. A cancel changes nothing and shows nothing.
    function write(key, command, args, done, sectionKey) {
        if (page.busy || !page.ctx)
            return;
        const why = Logic.argsProblem(command, args);
        if (why !== "") {
            page.notes = Logic.withNote(page.notes, key, { message: why, manual: [], detail: [], failed: true });
            return;
        }
        page.notes = Logic.withNote(page.notes, key, null);
        if (sectionKey)
            page.notes = Logic.withNote(page.notes, sectionKey, null);
        page.workingKey = key;
        page.workingCommand = command;
        page.ctx.command.run(command, args, result => {
            page.workingKey = "";
            page.workingCommand = "";
            if (Logic.isCancelled(result))
                return;
            const note = Logic.outcome(result);
            page.notes = Logic.withNote(page.notes, key, note);
            // A re-read can drop the row (an app that is gone), so the section shows it too.
            if (sectionKey)
                page.notes = Logic.withNote(page.notes, sectionKey, note);
            if (done)
                done(result);
        });
    }

    /// Runs one write for the uninstall dialog, which shows the answer itself: `done(result)`
    /// always follows, a cancel included. Like write(), it holds every other change button.
    function runOne(key, command, args, done) {
        if (page.busy || !page.ctx)
            return false;
        const why = Logic.argsProblem(command, args);
        if (why !== "") {
            done({ error: why });
            return true;
        }
        page.workingKey = key;
        page.workingCommand = command;
        page.ctx.command.run(command, args, result => {
            page.workingKey = "";
            page.workingCommand = "";
            done(result);
        });
        return true;
    }

    /// Runs settings changes one after another (`steps` from Logic.categorySteps, or one
    /// [command, args] pair), stopping at the first that crumBS refuses; then `done()`.
    function change(key, steps, done) {
        if (page.busy || !page.ctx || steps.length === 0)
            return;
        const [command, args] = steps[0];
        const why = Logic.argsProblem(command, args);
        if (why !== "") {
            page.notes = Logic.withNote(page.notes, key, { message: why, manual: [], detail: [], failed: true });
            return;
        }
        page.notes = Logic.withNote(page.notes, key, null);
        page.workingKey = key;
        page.workingCommand = command;
        page.ctx.command.run(command, args, result => {
            page.workingKey = "";
            page.workingCommand = "";
            const env = Logic.envelope(result);
            if (!env.ok) {
                page.notes = Logic.withNote(page.notes, key, Logic.outcome(result));
                if (done)
                    done();
                return;
            }
            if (steps.length > 1)
                page.change(key, steps.slice(1), done);
            else if (done)
                done();
        });
    }

    // Read the cheap things now: the space you can free, and (followed from then on) the history.
    // The rest is read when its tab is opened or when asked.
    onCtxChanged: {
        if (page.ctx && page.ctx !== page.openedWith) {
            page.openedWith = page.ctx;
            space.refresh();
            recent.start();
        }
    }

    SettingsTab {
        title: "Overview"
        iconName: "drive-harddisk"

        SettingsCard {
            title: "Space you can free"
            iconName: "edit-clear-all"
            help: "Caches and old files crumBS found in your home folder."
            wide: true
            Layout.fillWidth: true

            SpaceSection {
                id: space
                host: page
                Layout.fillWidth: true
            }
        }
        SettingsCard {
            title: "System space"
            iconName: "computer"
            help: "Looking is free. Cleaning system files asks for your password."
            wide: true
            Layout.fillWidth: true

            SystemSection {
                host: page
                Layout.fillWidth: true
            }
        }
    }

    SettingsTab {
        title: "Apps"
        iconName: "applications-all"

        SettingsCard {
            title: "Your apps"
            iconName: "applications-all"
            help: "Uninstall shows everything it would remove before it removes anything."
            wide: true
            Layout.fillWidth: true

            AppsSection {
                host: page
                Layout.fillWidth: true
            }
        }
    }

    SettingsTab {
        title: "Leftovers"
        iconName: "edit-delete"

        SettingsCard {
            title: "Leftovers of removed apps"
            iconName: "folder"
            help: "Files in your home folder that belong to no installed app. They go to the trash."
            wide: true
            Layout.fillWidth: true

            LeftoversSection {
                host: page
                Layout.fillWidth: true
            }
        }
        SettingsCard {
            title: "System leftovers"
            iconName: "folder-root"
            help: "Folders in /opt that no package owns. Deleting them needs your password."
            wide: true
            Layout.fillWidth: true

            SystemLeftovers {
                host: page
                Layout.fillWidth: true
            }
        }
    }

    SettingsTab {
        title: "History"
        iconName: "document-open-recent"

        SettingsCard {
            title: "Recent cleanups"
            iconName: "edit-undo"
            help: "What went to the trash can be put back."
            wide: true
            Layout.fillWidth: true

            HistorySection {
                id: recent
                host: page
                Layout.fillWidth: true
            }
        }
    }

    SettingsTab {
        title: "Preferences"
        iconName: "configure"

        PreferencesSection {
            host: page
            Layout.fillWidth: true
        }
    }
}
