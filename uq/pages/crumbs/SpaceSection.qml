// Space you can free (your own files): `scan` runs when the page opens and on Refresh. The total
// heads the card, a ring chart shares it out by category (SpacePie), and each category has a bar
// for its size next to the biggest one.
import QtQuick
import QtQuick.Controls as QQC2
import QtQuick.Layouts
import org.uq.settings
import "logic.js" as Logic

ColumnLayout {
    id: section

    property var host
    property bool loading: false
    // A refresh asked for while a scan runs (say, after a clean) scans once more when it ends,
    // so the sizes shown never come from a scan that started before the change.
    property bool again: false
    property var view: Logic.emptyView()

    /// The page's refresh() when uQ shows it again: the same read as Refresh.
    function reread() {
        section.refresh();
    }

    function refresh() {
        if (!section.host || !section.host.ctx)
            return;
        if (section.loading) {
            section.again = true;
            return;
        }
        section.loading = true;
        section.host.ctx.command.run("scan", [], result => {
            section.loading = false;
            if (!Logic.isCancelled(result))
                section.view = Logic.spaceView(result);
            if (section.again) {
                section.again = false;
                section.refresh();
            }
        });
    }

    spacing: 12

    SettingRow {
        settingId: "space"
        label: section.loading && section.view.rows.length === 0 ? "Looking…" : (section.view.totalText || "Space you can free")
        highlight: section.host ? section.host.highlight : ""
        Layout.fillWidth: true

        QQC2.Button {
            text: "Refresh"
            enabled: !section.loading
            onClicked: section.refresh()
        }
    }
    // While a scan runs: a moving bar and what it is doing, so the total doesn't appear from
    // nowhere. The last answer stays on screen under it until the new one is in.
    ColumnLayout {
        visible: section.loading
        spacing: 4
        Layout.fillWidth: true

        QQC2.ProgressBar {
            indeterminate: true
            Layout.fillWidth: true
        }
        UqLabel {
            text: "Looking through caches, logs, the trash and old downloads. This can take a minute on a big disk."
            wrapMode: Text.Wrap
            opacity: 0.7
            Layout.fillWidth: true
        }
    }
    ErrorCard {
        title: "Couldn't look for space you can free"
        detail: section.view.problem
        visible: section.view.problem !== ""
        Layout.fillWidth: true
    }
    SpacePie {
        host: section.host
        view: section.view
        Layout.fillWidth: true
        Layout.bottomMargin: 4
    }
    Repeater {
        model: section.view.rows

        CategoryRow {
            required property var modelData
            host: section.host
            item: modelData
            after: () => section.refresh()
            Layout.fillWidth: true
        }
    }
    UqLabel {
        text: section.view.unavailable
        visible: text !== ""
        wrapMode: Text.Wrap
        opacity: 0.7
        Layout.fillWidth: true
    }
    UqLabel {
        text: section.view.switchedOff
        visible: text !== ""
        wrapMode: Text.Wrap
        opacity: 0.7
        Layout.fillWidth: true
    }
}
