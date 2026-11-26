// The uninstall dialog's left column: how much goes, as three choices under "Uninstall…". The
// one picked is highlighted, and the dialog's right column shows what it removes.
import QtQuick
import QtQuick.Controls as QQC2
import QtQuick.Layouts
import org.uq.settings

ColumnLayout {
    id: sidebar

    /// "all", "keep-data" or "leftovers" (Uninstall.uninstallCommands).
    property string mode: "all"
    property var host

    signal picked(string mode)

    spacing: 4

    UqLabel {
        text: "Uninstall…"
        font.bold: true
        Layout.bottomMargin: 4
    }
    Repeater {
        model: [
            { mode: "all", title: "Everything", text: "The app, its settings, data and caches" },
            { mode: "keep-data", title: "Keep Settings & Data", text: "The app and its caches; keep my settings and data" },
            { mode: "leftovers", title: "Leftovers", text: "Only what it left behind; the app stays installed" }
        ]

        QQC2.ItemDelegate {
            id: choice

            required property var modelData
            readonly property bool current: sidebar.mode === choice.modelData.mode

            text: choice.modelData.title
            highlighted: choice.current
            checkable: true
            checked: choice.current
            padding: 10
            Accessible.name: choice.modelData.title + ": " + choice.modelData.text
            Layout.fillWidth: true
            onClicked: sidebar.picked(choice.modelData.mode)

            background: Rectangle {
                radius: 6
                color: sidebar.host ? sidebar.host.accentColor() : "steelblue"
                opacity: choice.current ? 0.18 : (choice.hovered ? 0.08 : 0)
                border.width: choice.current ? 1 : 0
                border.color: sidebar.host ? sidebar.host.accentColor() : "steelblue"
            }
            contentItem: ColumnLayout {
                spacing: 2

                UqLabel {
                    text: choice.modelData.title
                    font.bold: true
                    Layout.fillWidth: true
                }
                UqLabel {
                    text: choice.modelData.text
                    wrapMode: Text.Wrap
                    opacity: 0.7
                    Layout.fillWidth: true
                }
            }
        }
    }
}
