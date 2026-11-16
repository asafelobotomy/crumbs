// Apps and their size: `apps` runs when the tab is first opened, and on Refresh. Each app shows
// its icon; Uninstall… opens a preview of everything that would go (UninstallPreview), and
// only its own button removes anything. The desktop's own packages say "Part of uQ" and have
// no button (crumBS would refuse anyway).
import QtQuick
import QtQuick.Controls as QQC2
import QtQuick.Layouts
import org.uq.settings
import "logic.js" as Logic

ColumnLayout {
    id: section

    property var host
    property bool loading: false
    property bool loaded: false
    property string problem: ""
    property var all: []
    readonly property var shown: Logic.filterApps(section.all, filter.text, 200)
    /// The id of the app whose preview is open, "" when none.
    property string previewing: ""

    /// Reads when the tab is first shown, also when Settings opens the page on this tab.
    function autoload() {
        if (section.visible && !section.loaded && !section.loading)
            section.refresh();
    }

    onVisibleChanged: section.autoload()

    Connections {
        target: section.host
        function onCtxChanged() {
            section.autoload();
        }
    }

    function refresh() {
        if (!section.host || !section.host.ctx || section.loading)
            return;
        section.loading = true;
        section.host.ctx.command.run("apps", [], result => {
            section.loading = false;
            if (Logic.isCancelled(result))
                return;
            const env = Logic.envelope(result);
            section.loaded = env.data !== null;
            section.problem = env.data !== null ? "" : (env.cli || env.error || "crumBS gave an answer this page can't read.");
            section.all = Logic.apps(env.data);
        });
    }

    spacing: 8

    SettingRow {
        settingId: "apps"
        label: "Apps and their size"
        highlight: section.host ? section.host.highlight : ""
        Layout.fillWidth: true

        QQC2.Button {
            text: section.loaded ? "Refresh" : "List apps"
            enabled: !section.loading
            onClicked: section.refresh()
        }
    }
    ColumnLayout {
        visible: section.loading
        spacing: 4
        Layout.fillWidth: true

        QQC2.ProgressBar {
            indeterminate: true
            Layout.fillWidth: true
        }
        UqLabel {
            text: "Listing your apps and measuring them…"
            opacity: 0.7
        }
    }
    Outcome {
        host: section.host
        noteKey: "section:apps"
        Layout.fillWidth: true
    }
    UqLabel {
        text: "AppImages kept by appOFholding are managed there."
        wrapMode: Text.Wrap
        opacity: 0.7
        Layout.fillWidth: true
    }
    ErrorCard {
        title: "Couldn't list your apps"
        detail: section.problem
        visible: section.problem !== ""
        Layout.fillWidth: true
    }
    QQC2.TextField {
        id: filter
        placeholderText: "Filter by name"
        visible: section.loaded
        Layout.fillWidth: true
    }
    UqLabel {
        text: "No apps match."
        visible: section.loaded && section.shown.rows.length === 0
        opacity: 0.7
    }
    Repeater {
        model: section.shown.rows

        ColumnLayout {
            id: row

            required property var modelData
            readonly property string key: "app:" + row.modelData.id

            spacing: 2
            Layout.fillWidth: true

            RowLayout {
                spacing: 12
                Layout.fillWidth: true

                UqAppIcon {
                    name: row.modelData.icon
                    label: row.modelData.name
                    size: 32
                }
                UqLabel {
                    text: row.modelData.name
                    wrapMode: Text.Wrap
                    Layout.fillWidth: true
                }
                UqLabel {
                    text: row.modelData.sourceText + " · " + row.modelData.sizeText
                    opacity: 0.7
                }
                UqLabel {
                    text: row.modelData.partOf !== null ? "Part of " + row.modelData.partOf : ""
                    visible: row.modelData.partOf !== null
                }
                QQC2.Button {
                    text: section.previewing === row.modelData.id ? "Close" : "Uninstall…"
                    visible: row.modelData.partOf === null
                    enabled: !!section.host && !section.host.busy && row.modelData.canUninstall
                    onClicked: section.previewing = section.previewing === row.modelData.id ? "" : row.modelData.id
                }
            }
            UninstallPreview {
                visible: section.previewing === row.modelData.id
                active: visible
                host: section.host
                app: row.modelData
                noteKey: row.key
                after: () => {
                    section.previewing = "";
                    section.refresh();
                }
                Layout.fillWidth: true
            }
            Outcome {
                host: section.host
                noteKey: row.key
                Layout.fillWidth: true
            }
        }
    }
    UqLabel {
        text: "Showing the first 200. Type to narrow the list."
        visible: section.shown.more
        opacity: 0.7
    }
}
