// What uninstalling one app would remove, before anything is removed: crumbs plans the removal
// with --dry-run, and the plan is shown grouped (the app itself, your files, settings, caches,
// menu entries), each group saying whether it goes to the trash or for good. The person picks
// how much goes; the Uninstall button runs that same choice, which crumBS confirms again.
import QtQuick
import QtQuick.Controls as QQC2
import QtQuick.Layouts
import org.uq.settings
import "logic.js" as Logic

ColumnLayout {
    id: preview

    property var host
    /// One entry of Logic.apps().
    property var app
    /// The row's key in the page's notes.
    property string noteKey
    /// Only an open preview reads; closing it keeps what it read.
    property bool active: false
    /// Called after an uninstall that wasn't cancelled.
    property var after
    property string mode: "all"
    property bool loading: false
    property var view: null
    property string readFor: ""

    function read() {
        if (!preview.host || !preview.host.ctx || !preview.app || preview.loading)
            return;
        const args = [preview.app.id];
        // From the mode itself: a binding on it may not have caught up inside onModeChanged.
        const command = Logic.uninstallCommands(preview.mode).preview;
        if (Logic.argsProblem(command, args) !== "")
            return;
        const want = preview.mode + "\n" + preview.app.id;
        preview.loading = true;
        preview.host.ctx.command.run(command, args, result => {
            preview.loading = false;
            preview.view = Logic.removalPreview(result, "");
            preview.readFor = want;
            // The choice changed while that ran: read the plan for the new one.
            if (preview.active && preview.readFor !== preview.mode + "\n" + preview.app.id)
                preview.read();
        });
    }

    onActiveChanged: {
        if (preview.active && preview.readFor !== preview.mode + "\n" + (preview.app ? preview.app.id : ""))
            preview.read();
    }
    onModeChanged: {
        if (preview.active)
            preview.read();
    }

    spacing: 8

    UqLabel {
        text: "How much should go?"
        font.bold: true
    }
    QQC2.ButtonGroup {
        id: modes
    }
    Repeater {
        model: [
            { mode: "all", text: "The app, its settings, data and caches" },
            { mode: "keep-data", text: "The app and its caches; keep my settings and data" },
            { mode: "leftovers", text: "Only what it left behind; the app stays installed" }
        ]

        QQC2.RadioButton {
            required property var modelData
            text: modelData.text
            checked: preview.mode === modelData.mode
            enabled: !preview.loading
            QQC2.ButtonGroup.group: modes
            onClicked: preview.mode = modelData.mode
        }
    }

    ColumnLayout {
        visible: preview.loading
        spacing: 4
        Layout.fillWidth: true

        QQC2.ProgressBar {
            indeterminate: true
            Layout.fillWidth: true
        }
        UqLabel {
            text: "Working out what would be removed…"
            opacity: 0.7
        }
    }
    ErrorCard {
        title: "Couldn't work out what would be removed"
        detail: preview.view ? preview.view.problem : ""
        visible: !preview.loading && preview.view !== null && preview.view.problem !== ""
        Layout.fillWidth: true
    }

    ColumnLayout {
        visible: !preview.loading && preview.view !== null && preview.view.problem === ""
        spacing: 6
        Layout.fillWidth: true

        UqLabel {
            text: preview.view ? preview.view.totalText : ""
            font.bold: true
        }
        Repeater {
            model: preview.view ? preview.view.groups : []

            ColumnLayout {
                id: group

                required property var modelData
                property bool open: false

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
                        text: [group.modelData.countText, group.modelData.sizeText].filter(t => t !== "").join(" · ")
                        opacity: 0.7
                    }
                }
                UqLabel {
                    id: how
                    text: group.modelData.how
                    opacity: 0.8
                    Layout.leftMargin: 40
                }
                Binding {
                    target: how
                    property: "color"
                    value: preview.host ? preview.host.errorColor() : "red"
                    when: group.modelData.forGood
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
        Repeater {
            model: preview.view ? preview.view.notes : []

            UqLabel {
                required property string modelData
                text: modelData
                wrapMode: Text.Wrap
                opacity: 0.7
                Layout.fillWidth: true
            }
        }
        RowLayout {
            spacing: 8

            QQC2.Button {
                text: preview.mode === "leftovers" ? "Remove leftovers" : "Uninstall"
                enabled: !!preview.host && !preview.host.busy && preview.view !== null && !preview.view.empty
                onClicked: preview.host.write(preview.noteKey, Logic.uninstallCommands(preview.mode).run, [preview.app.id], preview.after, "section:apps")
            }
            UqLabel {
                text: "You'll be asked once more."
                opacity: 0.7
            }
        }
    }
}
