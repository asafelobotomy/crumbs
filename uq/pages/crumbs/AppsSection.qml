// Apps and their size: `apps` runs when asked. Uninstall removes an app with its leftovers; the
// desktop's own packages say "Part of uQ" and have no button (crumBS would refuse anyway).
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

        UqLabel {
            text: "Listing apps…"
            visible: section.loading
        }
        QQC2.Button {
            text: "List apps"
            enabled: !section.loading
            onClicked: section.refresh()
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
                    text: "Uninstall…"
                    visible: row.modelData.partOf === null
                    enabled: !!section.host && !section.host.busy && row.modelData.canUninstall
                    onClicked: section.host.write(row.key, "uninstall", [row.modelData.id], () => section.refresh(), "section:apps")
                }
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
