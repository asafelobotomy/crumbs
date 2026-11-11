// Leftovers of removed apps: `orphans` runs when asked; each one can be moved to the trash.
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
    property var rows: []

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
            section.rows = Logic.orphansList(env.data);
        });
    }

    spacing: 8

    SettingRow {
        settingId: "leftovers"
        label: "Leftovers of removed apps"
        highlight: section.host ? section.host.highlight : ""
        Layout.fillWidth: true

        UqLabel {
            text: "Looking for leftovers…"
            visible: section.loading
        }
        QQC2.Button {
            text: "Find leftovers"
            enabled: !section.loading
            onClicked: section.refresh()
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

        ColumnLayout {
            id: row

            required property var modelData
            readonly property string key: "orphan:" + row.modelData.path

            spacing: 2
            Layout.fillWidth: true

            RowLayout {
                spacing: 12
                Layout.fillWidth: true

                UqLabel {
                    text: row.modelData.shown
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
