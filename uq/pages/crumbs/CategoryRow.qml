// One category of reclaimable space: its plain name, how much, and a button that cleans it up.
// Which command runs comes from logic.js (cleanCommandFor), so a category is never cleaned under
// a gentler question than its kind needs.
import QtQuick
import QtQuick.Controls as QQC2
import QtQuick.Layouts
import org.uq.settings
import "logic.js" as Logic

ColumnLayout {
    id: row

    property var host
    /// One entry of Logic.categories().
    property var item
    property string cleanText: "Clean up"
    readonly property string key: "category:" + (row.item ? row.item.category : "")

    /// A cleanup finished (not cancelled): the section reads its space again.
    signal cleaned()

    function clean() {
        if (!row.host || !row.item || row.item.command === null)
            return;
        row.host.write(row.key, row.item.command, [row.item.category], () => row.cleaned());
    }

    spacing: 2

    RowLayout {
        spacing: 12
        Layout.fillWidth: true

        UqLabel {
            text: row.item.label
            wrapMode: Text.Wrap
            Layout.fillWidth: true
        }
        UqLabel {
            text: row.item.error !== null ? "Couldn't check this" : row.item.sizeText + " · " + row.item.itemsText
        }
        QQC2.Button {
            text: row.cleanText
            visible: row.item.command !== null && row.item.error === null
            enabled: !!row.host && !row.host.busy && row.item.items > 0
            onClicked: row.clean()
        }
    }
    UqLabel {
        id: detail
        text: row.item.error !== null ? row.item.error : row.item.how
        visible: text !== ""
        wrapMode: Text.Wrap
        opacity: row.item.error !== null ? 1 : 0.7
        Layout.fillWidth: true
    }
    Binding {
        target: detail
        property: "color"
        value: row.host ? row.host.errorColor() : "red"
        when: row.item.error !== null
    }
    Outcome {
        host: row.host
        noteKey: row.key
        Layout.fillWidth: true
    }
}
