// Storage and cleanup: what crumBS finds, and what the person chooses to remove. Everything goes
// through the crumbs command line (page.toml); logic.js reads its answers and words the result.
// The page never forces a removal: crumBS's own refusals are shown as they are.
import QtQuick
import QtQuick.Controls as QQC2
import QtQuick.Layouts
import org.uq.settings
import "logic.js" as Logic

Item {
    id: page

    property var ctx
    property var theme
    /// The [[setting]] row Settings search jumped to; the rows flash and scroll themselves.
    property string highlight
    /// The key of the row whose write is running, "" when none. Only one write runs at a time.
    property string workingKey: ""
    readonly property bool busy: page.workingKey !== ""
    /// What each write said, by row key: {message, manual, detail, failed}.
    property var notes: ({})
    property var openedWith: null

    function noteFor(key) {
        return page.notes[key] || null;
    }

    function errorColor() {
        const colors = page.ctx && page.ctx.theme ? page.ctx.theme.colors : null;
        return (colors && colors.error) || "red";
    }

    /// Runs one write (a confirmed command) for the row `key`, then `done(result)` unless the
    /// person cancelled. A cancel changes nothing and shows nothing.
    function write(key, command, args, done) {
        if (page.busy || !page.ctx)
            return;
        const why = Logic.argsProblem(command, args);
        if (why !== "") {
            page.notes = Logic.withNote(page.notes, key, { message: why, manual: [], detail: [], failed: true });
            return;
        }
        page.notes = Logic.withNote(page.notes, key, null);
        page.workingKey = key;
        page.ctx.command.run(command, args, result => {
            page.workingKey = "";
            if (Logic.isCancelled(result))
                return;
            page.notes = Logic.withNote(page.notes, key, Logic.outcome(result));
            if (done)
                done(result);
        });
    }

    // Read the cheap things now: the space you can free, and (followed from then on) the history.
    // The system space, leftovers and apps are read only when asked.
    onCtxChanged: {
        if (page.ctx && page.ctx !== page.openedWith) {
            page.openedWith = page.ctx;
            space.refresh();
            recent.start();
        }
    }

    Flickable {
        id: flick
        anchors.fill: parent
        contentWidth: width
        contentHeight: column.implicitHeight + 32
        clip: true
        boundsBehavior: Flickable.StopAtBounds
        QQC2.ScrollBar.vertical: QQC2.ScrollBar {}

        ColumnLayout {
            id: column
            x: 16
            y: 16
            width: flick.width - 32
            spacing: 28

            SpaceSection {
                id: space
                host: page
                Layout.fillWidth: true
            }
            SystemSection {
                host: page
                Layout.fillWidth: true
            }
            LeftoversSection {
                host: page
                Layout.fillWidth: true
            }
            HistorySection {
                id: recent
                host: page
                Layout.fillWidth: true
            }
            AppsSection {
                host: page
                Layout.fillWidth: true
            }
        }
    }
}
