// Space you can free (your own files): `scan` runs when the page opens and on Refresh.
import QtQuick
import QtQuick.Controls as QQC2
import QtQuick.Layouts
import org.uq.settings
import "logic.js" as Logic

ColumnLayout {
    id: section

    property var host
    property bool loading: false
    property var view: Logic.emptyView()

    function refresh() {
        if (!section.host || !section.host.ctx || section.loading)
            return;
        section.loading = true;
        section.host.ctx.command.run("scan", [], result => {
            section.loading = false;
            if (!Logic.isCancelled(result))
                section.view = Logic.spaceView(result);
        });
    }

    spacing: 8

    SettingRow {
        settingId: "space"
        label: "Space you can free"
        highlight: section.host ? section.host.highlight : ""
        Layout.fillWidth: true

        UqLabel {
            text: "Looking for space you can free…"
            visible: section.loading
        }
        QQC2.Button {
            text: "Refresh"
            enabled: !section.loading
            onClicked: section.refresh()
        }
    }
    ErrorCard {
        title: "Couldn't look for space you can free"
        detail: section.view.problem
        visible: section.view.problem !== ""
        Layout.fillWidth: true
    }
    Repeater {
        model: section.view.rows

        CategoryRow {
            required property var modelData
            host: section.host
            item: modelData
            onCleaned: section.refresh()
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
