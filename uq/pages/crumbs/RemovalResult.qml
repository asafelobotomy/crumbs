// What one uninstall in the dialog did, once it is over (Logic.removalReport): crumBS's answer in
// plain words, then what was removed, grouped like the preview, and what stayed and why.
// Commands to run by hand sit behind Advanced details.
import QtQuick
import QtQuick.Controls as QQC2
import QtQuick.Layouts
import org.uq.settings
import "logic.js" as Logic

ColumnLayout {
    id: result

    property var host
    /// Logic.removalReport().
    property var report
    /// The lists start unfolded (one app) or folded (a queue).
    property bool unfolded: true
    /// The queue says the message on the app's own line already.
    property bool showMessage: true
    property bool expanded: false

    spacing: 4

    UqLabel {
        id: message
        text: result.report ? result.report.message : ""
        visible: result.showMessage
        wrapMode: Text.Wrap
        Layout.fillWidth: true
    }
    Binding {
        target: message
        property: "color"
        value: result.host ? result.host.errorColor() : "red"
        when: !!result.report && result.report.failed
    }
    Repeater {
        model: result.report ? result.report.groups : []

        ColumnLayout {
            id: group

            required property var modelData
            property bool open: result.unfolded

            spacing: 2
            Layout.fillWidth: true

            RowLayout {
                spacing: 8
                Layout.fillWidth: true

                QQC2.ToolButton {
                    text: group.open ? "▾" : "▸"
                    Accessible.name: group.open ? "Hide the list" : "Show the list"
                    onClicked: group.open = !group.open
                }
                UqLabel {
                    text: group.modelData.title
                    Layout.fillWidth: true
                }
                UqLabel {
                    text: group.modelData.countText
                    opacity: 0.7
                }
            }
            UqLabel {
                text: group.modelData.items.join("\n") + (group.modelData.more > 0 ? "\n… and " + group.modelData.more + " more" : "")
                visible: group.open
                wrapMode: Text.WrapAnywhere
                opacity: 0.8
                Layout.leftMargin: 40
                Layout.fillWidth: true
            }
        }
    }
    UqLabel {
        text: "Not removed:"
        visible: !!result.report && result.report.failures.length > 0
        font.bold: true
    }
    UqLabel {
        text: result.report ? result.report.failures.join("\n") : ""
        visible: text !== ""
        wrapMode: Text.WrapAnywhere
        opacity: 0.8
        Layout.leftMargin: 12
        Layout.fillWidth: true
    }
    QQC2.Button {
        text: "Advanced details"
        flat: true
        visible: !!result.report && result.report.manual.length > 0
        onClicked: result.expanded = !result.expanded
    }
    UqLabel {
        text: result.report ? result.report.manual.join("\n") : ""
        visible: result.expanded && text !== ""
        wrapMode: Text.WrapAnywhere
        opacity: 0.8
        Layout.fillWidth: true
    }
}
