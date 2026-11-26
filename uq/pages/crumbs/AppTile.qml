// One app on the Apps tab: a framed tile with its icon, name, where it came from and its size,
// and its Uninstall… button inside the same frame, so no button can be taken for a neighbour's.
// The desktop's own packages say "Part of uQ" and have no button (crumBS would refuse anyway).
// Uninstall… opens the uninstall dialog; the tick box puts the app in the queue the Apps tab
// uninstalls one after another.
import QtQuick
import QtQuick.Controls as QQC2
import QtQuick.Layouts
import org.uq.settings
import "apps.js" as Apps
QQC2.Frame {
    id: tile

    property var host
    /// One entry of Apps.apps().
    property var app
    /// Selected for the queue.
    property bool selected: false
    readonly property string key: "app:" + (tile.app ? tile.app.id : "")

    /// Uninstall… was pressed.
    signal uninstallAsked
    /// The tick box was changed by hand.
    signal selectToggled(bool on)

    padding: 12

    ColumnLayout {
        anchors.fill: parent
        spacing: 10

        RowLayout {
            spacing: 12
            Layout.fillWidth: true

            QQC2.CheckBox {
                checked: tile.selected
                visible: tile.app.partOf === null
                enabled: !!tile.host && !tile.host.busy && tile.app.canUninstall
                Accessible.name: "Add " + tile.app.name + " to the apps to uninstall"
                onToggled: tile.selectToggled(checked)
            }
            UqAppIcon {
                name: tile.app.icon
                label: tile.app.name
                size: 40
            }
            ColumnLayout {
                spacing: 2
                Layout.fillWidth: true

                UqLabel {
                    text: tile.app.name
                    font.bold: true
                    elide: Text.ElideRight
                    Layout.fillWidth: true
                }
                UqLabel {
                    text: tile.app.partOf !== null ? "Part of " + tile.app.partOf + " · " + tile.app.sizeText : tile.app.sourceText + " · " + tile.app.sizeText
                    elide: Text.ElideRight
                    opacity: 0.7
                    Layout.fillWidth: true
                }
            }
            QQC2.Button {
                text: "Uninstall…"
                visible: tile.app.partOf === null
                enabled: !!tile.host && !tile.host.busy && tile.app.canUninstall
                onClicked: tile.uninstallAsked()
            }
        }
        Outcome {
            host: tile.host
            noteKey: tile.key
            Layout.fillWidth: true
        }
    }
}
