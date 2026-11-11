// What a write said, under its row: "Working…" while it runs, then crumBS's own answer in plain
// words. Commands to run by hand, and what failed, sit behind Advanced details.
import QtQuick
import QtQuick.Controls as QQC2
import QtQuick.Layouts
import org.uq.settings
import "logic.js" as Logic

ColumnLayout {
    id: out

    /// The page (Page.qml): it holds the notes and which write is running.
    property var host
    /// The row's key in the page's notes.
    property string noteKey
    readonly property var note: out.host ? out.host.noteFor(out.noteKey) : null
    readonly property bool working: !!out.host && out.host.workingKey === out.noteKey
    readonly property var details: out.note ? out.note.manual.concat(out.note.detail) : []
    property bool expanded: false

    visible: out.working || out.note !== null
    spacing: 2
    onNoteChanged: out.expanded = false

    UqLabel {
        id: message
        text: out.working ? Logic.WORKING : (out.note ? out.note.message : "")
        visible: text !== ""
        wrapMode: Text.Wrap
        Layout.fillWidth: true
    }
    Binding {
        target: message
        property: "color"
        value: out.host ? out.host.errorColor() : "red"
        when: !out.working && out.note !== null && out.note.failed
    }
    QQC2.Button {
        text: "Advanced details"
        flat: true
        visible: !out.working && out.details.length > 0
        onClicked: out.expanded = !out.expanded
    }
    UqLabel {
        text: out.details.join("\n")
        visible: out.expanded && !out.working && out.details.length > 0
        wrapMode: Text.WrapAnywhere
        opacity: 0.8
        Layout.fillWidth: true
    }
}
