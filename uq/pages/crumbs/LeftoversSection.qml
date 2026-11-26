// Leftovers of removed apps: `orphans` runs when the tab is first opened, and when asked; each
// one can be moved to the trash.
import QtQuick
import QtQuick.Controls as QQC2
import QtQuick.Layouts
import org.uq.settings
import "logic.js" as Logic

import "apps.js" as Apps
ColumnLayout {
    id: section

    property var host
    property bool loading: false
    property bool loaded: false
    property string problem: ""
    property var rows: []

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
        section.host.ctx.command.run("orphans", [], result => {
            section.loading = false;
            if (Logic.isCancelled(result))
                return;
            const env = Logic.envelope(result);
            section.loaded = env.data !== null;
            section.problem = env.data !== null ? "" : (env.cli || env.error || "crumBS gave an answer this page can't read.");
            section.rows = Apps.orphansList(env.data);
        });
    }

    spacing: 12

    SettingRow {
        settingId: "leftovers"
        label: "Leftovers of removed apps"
        highlight: section.host ? section.host.highlight : ""
        Layout.fillWidth: true

        QQC2.Button {
            text: section.loaded ? "Look again" : "Find leftovers"
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
            text: "Looking through your settings, data and cache folders…"
            opacity: 0.7
        }
    }
    Outcome {
        host: section.host
        noteKey: "section:leftovers"
        Layout.fillWidth: true
    }
    ErrorCard {
        title: "Couldn't look for leftovers"
        detail: section.problem
        visible: section.problem !== ""
        Layout.fillWidth: true
    }
    UqLabel {
        text: "No leftovers found."
        visible: section.loaded && section.rows.length === 0
        opacity: 0.7
    }
    Repeater {
        model: section.rows

        QQC2.Frame {
            id: row

            required property var modelData
            readonly property string key: "orphan:" + row.modelData.path

            padding: 12
            Layout.fillWidth: true

          ColumnLayout {
            anchors.fill: parent
            spacing: 6

            RowLayout {
                spacing: 12
                Layout.fillWidth: true

                UqLabel {
                    text: row.modelData.shown
                    font.bold: true
                    wrapMode: Text.WrapAnywhere
                    Layout.fillWidth: true
                }
                UqLabel {
                    text: row.modelData.sizeText
                }
                QQC2.Button {
                    text: "Move to trash"
                    enabled: !!section.host && !section.host.busy && row.modelData.valid
                    onClicked: section.host.write(row.key, "orphan-trash", [row.modelData.path], () => section.refresh(), "section:leftovers")
                }
            }
            UqLabel {
                text: row.modelData.reason
                visible: text !== ""
                wrapMode: Text.Wrap
                opacity: 0.7
                Layout.fillWidth: true
            }
            Outcome {
                host: section.host
                noteKey: row.key
                Layout.fillWidth: true
            }
          }
        }
    }
}
