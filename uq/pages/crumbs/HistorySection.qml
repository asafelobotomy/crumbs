// Recent cleanups: the last 20 batches, with a way to put one back. `history` is watched, so the
// list follows crumbs' history file (page.toml); it also reads again after a restore.
import QtQuick
import QtQuick.Controls as QQC2
import QtQuick.Layouts
import org.uq.settings
import "logic.js" as Logic

ColumnLayout {
    id: section

    property var host
    property bool loaded: false
    property string problem: ""
    property var rows: []
    property var watching: null

    function show(result) {
        // A change that lands while a read runs is answered by the run after it.
        if (Logic.isCancelled(result) || (result && result.error === "already running"))
            return;
        const env = Logic.envelope(result);
        section.loaded = env.data !== null;
        section.problem = env.data !== null ? "" : (env.cli || env.error || "crumBS gave an answer this page can't read.");
        if (env.data !== null)
            section.rows = Logic.batches(env.data);
    }

    function start() {
        if (section.host && section.host.ctx && section.watching === null)
            section.watching = section.host.ctx.command.watch("history", section.show);
    }

    function refresh() {
        if (section.host && section.host.ctx)
            section.host.ctx.command.run("history", [], section.show);
    }

    spacing: 8

    SettingRow {
        settingId: "history"
        label: "Recent cleanups"
        highlight: section.host ? section.host.highlight : ""
        Layout.fillWidth: true
    }
    ErrorCard {
        title: "Couldn't read your recent cleanups"
        detail: section.problem
        visible: section.problem !== ""
        Layout.fillWidth: true
    }
    UqLabel {
        text: "Nothing cleaned up yet."
        visible: section.loaded && section.rows.length === 0
        opacity: 0.7
    }
    Repeater {
        model: section.rows

        ColumnLayout {
            id: row

            required property var modelData
            readonly property string key: "batch:" + row.modelData.batch

            spacing: 2
            Layout.fillWidth: true

            RowLayout {
                spacing: 12
                Layout.fillWidth: true

                UqLabel {
                    text: row.modelData.title
                    wrapMode: Text.Wrap
                    Layout.fillWidth: true
                }
                UqLabel {
                    text: row.modelData.time + " · " + row.modelData.itemsText + " · " + row.modelData.sizeText
                    opacity: 0.7
                }
                QQC2.Button {
                    text: "Put back"
                    visible: row.modelData.canRestore
                    enabled: !!section.host && !section.host.busy
                    onClicked: section.host.write(row.key, "restore", [row.modelData.batch], () => section.refresh())
                }
            }
            Outcome {
                host: section.host
                noteKey: row.key
                Layout.fillWidth: true
            }
        }
    }
}
