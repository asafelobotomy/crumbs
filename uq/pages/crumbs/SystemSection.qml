// System space: looked at only when asked (`scan-system`), and cleaned with the password.
import QtQuick
import QtQuick.Controls as QQC2
import QtQuick.Layouts
import org.uq.settings
import "logic.js" as Logic

ColumnLayout {
    id: section

    property var host
    property bool loading: false
    property bool checked: false
    property var view: Logic.emptyView()

    function refresh() {
        if (!section.host || !section.host.ctx || section.loading)
            return;
        section.loading = true;
        section.host.ctx.command.run("scan-system", [], result => {
            section.loading = false;
            if (Logic.isCancelled(result))
                return;
            section.checked = true;
            section.view = Logic.spaceView(result);
        });
    }

    spacing: 8

    SettingRow {
        settingId: "system-space"
        label: "System space"
        highlight: section.host ? section.host.highlight : ""
        Layout.fillWidth: true

        UqLabel {
            text: "Checking system space…"
            visible: section.loading
        }
        QQC2.Button {
            text: "Check system space"
            enabled: !section.loading
            onClicked: section.refresh()
        }
    }
    UqLabel {
        text: "Looking doesn't need your password. Cleaning system files does."
        visible: !section.checked && !section.loading
        wrapMode: Text.Wrap
        opacity: 0.7
        Layout.fillWidth: true
    }
    ErrorCard {
        title: "Couldn't check system space"
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
            cleanText: "Clean up…"
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
}
