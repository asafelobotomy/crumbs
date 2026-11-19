// One app on the Apps tab: a framed tile with its icon, name, where it came from and its size,
// and its Uninstall… button inside the same frame, so no button can be taken for a neighbour's.
// The desktop's own packages say "Part of uQ" and have no button (crumBS would refuse anyway).
// Uninstall… opens the preview in the tile (UninstallPreview); the tile then spans its whole line.
import QtQuick
import QtQuick.Controls as QQC2
import QtQuick.Layouts
import org.uq.settings
import "logic.js" as Logic

QQC2.Frame {
    id: tile

    property var host
    /// One entry of Logic.apps().
    property var app
    property bool previewing: false
    readonly property string key: "app:" + (tile.app ? tile.app.id : "")

    signal toggled
    /// The app was removed (or its leftovers): the list reads again.
    signal done

    padding: 12

    ColumnLayout {
        anchors.fill: parent
        spacing: 10

        RowLayout {
            spacing: 12
            Layout.fillWidth: true

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
                text: tile.previewing ? "Close" : "Uninstall…"
                visible: tile.app.partOf === null
                enabled: !!tile.host && !tile.host.busy && tile.app.canUninstall
                onClicked: tile.toggled()
            }
        }
        UninstallPreview {
            visible: tile.previewing
            active: visible
            host: tile.host
            app: tile.app
            noteKey: tile.key
            after: () => tile.done()
            Layout.fillWidth: true
        }
        Outcome {
            host: tile.host
            noteKey: tile.key
            Layout.fillWidth: true
        }
    }
}
