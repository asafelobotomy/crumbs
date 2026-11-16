// System leftovers: folders directly in /opt that no package owns (`orphans --system`),
// read only when asked. Each can be deleted for good, as root, after its own question.
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
        section.host.ctx.command.run("orphans-system", [], result => {
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
        label: "Folders no package owns"
        Layout.fillWidth: true

        QQC2.Button {
            text: section.loaded ? "Look again" : "Find system leftovers"
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
            text: "Looking through /opt…"
            opacity: 0.7
        }
    }
    Outcome {
        host: section.host
        noteKey: "section:system-leftovers"
        Layout.fillWidth: true
    }
    ErrorCard {
        title: "Couldn't look for system leftovers"
        detail: section.problem
        visible: section.problem !== ""
        Layout.fillWidth: true
    }
    UqLabel {
        text: "No system leftovers found."
        visible: section.loaded && section.rows.length === 0
        opacity: 0.7
    }
    Repeater {
        model: section.rows

        ColumnLayout {
            id: row

            required property var modelData
            readonly property string key: "system-orphan:" + row.modelData.path

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
                    text: "Delete…"
                    enabled: !!section.host && !section.host.busy && Logic.argsProblem("orphan-delete-system", [row.modelData.path]) === ""
                    onClicked: section.host.write(row.key, "orphan-delete-system", [row.modelData.path], () => section.refresh(), "section:system-leftovers")
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
