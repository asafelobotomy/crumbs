// What uninstalling one app would remove, before anything is removed: crumbs plans the removal
// with --dry-run, and the plan is shown grouped (the app itself, your files, settings, caches,
// menu entries), each group saying whether it goes to the trash or for good. It sits in the
// uninstall dialog (UninstallDialog), which holds the buttons and runs the chosen `mode`.
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
    /// Only an open preview reads; closing it keeps what it read.
    property bool active: false
    /// How much goes: "all", "keep-data" or "leftovers" (Logic.uninstallCommands); the dialog
    /// holds the choice.
    property string mode: "all"
    /// The lists of files start unfolded (one app) or folded (each app of a queue).
    property bool unfolded: false
    property bool loading: false
    property var view: null
    /// The plan is in, and it removes something.
    readonly property bool ready: !preview.loading && preview.view !== null && preview.view.problem === "" && !preview.view.empty
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
                property bool open: preview.unfolded

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
    }
}
