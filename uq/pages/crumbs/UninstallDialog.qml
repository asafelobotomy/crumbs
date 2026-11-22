// The uninstall dialog, for one app (its Uninstall… button) or for the apps ticked on the Apps tab
// (a queue). It goes through three steps:
//   review:  how much should go, and what that removes (UninstallPreview, read with --dry-run);
//   working: a progress bar while crumBS uninstalls, one app after another;
//   done:    what was removed and what stayed (RemovalResult), and OK to close.
// Each uninstall is its own confirmed command (page.toml), so uQ asks once for each app, and an
// app that is open is never forced: its refusal is shown, with a button that asks it to close.
import QtQuick
import QtQuick.Controls as QQC2
import QtQuick.Layouts
import org.uq.settings
import "logic.js" as Logic

QQC2.Dialog {
    id: dialog

    property var host
    /// One per app: {app, state ("waiting", "running", "done" or "skipped"), close, report}.
    property var entries: []
    /// "review", "working" or "done".
    property string stage: "review"
    property string mode: "all"
    /// Asked to stop: the app being uninstalled finishes, the rest are skipped.
    property bool stopping: false
    /// Something was removed: the app list reads again when the dialog closes.
    property bool changed: false
    readonly property bool single: dialog.entries.length === 1
    readonly property int finishedCount: dialog.entries.filter(e => e.state === "done" || e.state === "skipped").length
    readonly property var running: dialog.entries.find(e => e.state === "running") || null
    readonly property int openCount: dialog.entries.filter(e => e.report && e.report.running).length

    /// Something may have changed: the list reads again.
    signal finished

    function openFor(apps) {
        dialog.entries = apps.map(a => ({ app: a, state: "waiting", close: false, report: null }));
        dialog.mode = "all";
        dialog.stage = "review";
        dialog.stopping = false;
        dialog.changed = false;
        dialog.open();
    }

    function setEntry(i, change) {
        const next = dialog.entries.slice();
        next[i] = Object.assign({}, next[i], change);
        dialog.entries = next;
    }

    function start() {
        if (!dialog.host || dialog.host.busy)
            return;
        dialog.stopping = false;
        dialog.stage = "working";
        dialog.runNext();
    }

    /// Uninstalls the next waiting app, or ends the queue.
    function runNext() {
        const i = dialog.entries.findIndex(e => e.state === "waiting");
        if (i < 0 || dialog.stopping) {
            dialog.entries = dialog.entries.map(e => e.state === "waiting" ? Object.assign({}, e, { state: "skipped" }) : e);
            dialog.stage = "done";
            return;
        }
        const entry = dialog.entries[i];
        const commands = Logic.uninstallCommands(dialog.mode);
        dialog.setEntry(i, { state: "running" });
        const started = dialog.host.runOne("dialog:" + entry.app.id, entry.close ? commands.close : commands.run, [entry.app.id], result => {
            const report = Logic.removalReport(result, "");
            if (report.status === "removed" || report.status === "partial")
                dialog.changed = true;
            dialog.setEntry(i, { state: "done", report: report });
            dialog.runNext();
        });
        if (!started) {
            dialog.setEntry(i, { state: "waiting" });
            dialog.stopping = true;
            dialog.runNext();
        }
    }

    /// The apps that were open go again, this time asked to close first.
    function retryClosing() {
        dialog.entries = dialog.entries.map(e => e.report && e.report.running ? Object.assign({}, e, { state: "waiting", close: true, report: null }) : e);
        dialog.start();
    }

    function statusText(entry) {
        if (entry.state === "running")
            return "Uninstalling…";
        if (entry.state === "skipped")
            return "Skipped: the queue was stopped";
        if (entry.state === "done" && entry.report)
            return entry.report.message;
        return "Waiting";
    }

    parent: QQC2.Overlay.overlay
    anchors.centerIn: parent
    modal: true
    padding: 16
    width: Math.min(parent ? parent.width - 32 : 640, 640)
    closePolicy: dialog.stage === "working" ? QQC2.Popup.NoAutoClose : QQC2.Popup.CloseOnEscape
    onClosed: {
        if (dialog.changed) {
            dialog.changed = false;
            dialog.finished();
        }
    }

    contentItem: ColumnLayout {
        spacing: 12

        UqLabel {
            text: dialog.stage === "done" ? Logic.queueSummary(dialog.entries.map(e => e.report))
                : dialog.single ? "Uninstall " + dialog.entries[0].app.name
                : "Uninstall " + Logic.plural(dialog.entries.length, "app", "apps")
            font.bold: true
            font.pixelSize: 18
            wrapMode: Text.Wrap
            Layout.fillWidth: true
        }

        QQC2.ScrollView {
            id: scroll
            clip: true
            contentWidth: availableWidth
            Layout.fillWidth: true
            Layout.preferredHeight: Math.min(body.implicitHeight, dialog.parent ? dialog.parent.height * 0.7 : 520)

            ColumnLayout {
                id: body
                width: scroll.availableWidth
                spacing: 10

                // ---- review: the app (or the queue), how much should go, and what that removes
                RowLayout {
                    visible: dialog.single && dialog.stage === "review"
                    spacing: 12
                    Layout.fillWidth: true

                    UqAppIcon {
                        name: dialog.single ? dialog.entries[0].app.icon : ""
                        label: dialog.single ? dialog.entries[0].app.name : ""
                        size: 40
                    }
                    UqLabel {
                        text: dialog.single ? dialog.entries[0].app.sourceText + " · " + dialog.entries[0].app.sizeText : ""
                        opacity: 0.7
                        Layout.fillWidth: true
                    }
                }
                UqLabel {
                    text: "They are uninstalled one after another, and each asks once more before it goes."
                    visible: !dialog.single && dialog.stage === "review"
                    wrapMode: Text.Wrap
                    opacity: 0.7
                    Layout.fillWidth: true
                }
                UqLabel {
                    text: "How much should go?"
                    visible: dialog.stage === "review"
                    font.bold: true
                }
                QQC2.ButtonGroup {
                    id: modes
                }
                Repeater {
                    model: dialog.stage === "review" ? [
                        { mode: "all", text: "The app, its settings, data and caches" },
                        { mode: "keep-data", text: "The app and its caches; keep my settings and data" },
                        { mode: "leftovers", text: "Only what it left behind; the app stays installed" }
                    ] : []

                    QQC2.RadioButton {
                        required property var modelData
                        text: modelData.text
                        checked: dialog.mode === modelData.mode
                        QQC2.ButtonGroup.group: modes
                        onClicked: dialog.mode = modelData.mode
                    }
                }
                UninstallPreview {
                    id: onlyPreview
                    visible: dialog.single && dialog.stage === "review"
                    active: dialog.opened && visible
                    host: dialog.host
                    app: dialog.single ? dialog.entries[0].app : null
                    mode: dialog.mode
                    Layout.fillWidth: true
                }

                // ---- the queue: one line per app, with its plan (review) or how it went
                Repeater {
                    model: dialog.single ? [] : dialog.entries

                    QQC2.Frame {
                        id: line

                        required property var modelData
                        required property int index
                        property bool open: false

                        padding: 8
                        Layout.fillWidth: true

                        ColumnLayout {
                            anchors.fill: parent
                            spacing: 6

                            RowLayout {
                                spacing: 10
                                Layout.fillWidth: true

                                UqAppIcon {
                                    name: line.modelData.app.icon
                                    label: line.modelData.app.name
                                    size: 24
                                }
                                ColumnLayout {
                                    spacing: 0
                                    Layout.fillWidth: true

                                    UqLabel {
                                        text: line.modelData.app.name
                                        font.bold: true
                                        elide: Text.ElideRight
                                        Layout.fillWidth: true
                                    }
                                    UqLabel {
                                        id: lineStatus
                                        text: dialog.stage === "review" ? line.modelData.app.sourceText + " · " + line.modelData.app.sizeText : dialog.statusText(line.modelData)
                                        wrapMode: Text.Wrap
                                        opacity: line.modelData.state === "running" ? 1 : 0.7
                                        Layout.fillWidth: true
                                    }
                                    Binding {
                                        target: lineStatus
                                        property: "color"
                                        value: dialog.host ? dialog.host.errorColor() : "red"
                                        when: dialog.stage === "done" && !!line.modelData.report && line.modelData.report.failed
                                    }
                                }
                                QQC2.Button {
                                    text: line.open ? "Hide" : (dialog.stage === "review" ? "What goes" : "What went")
                                    visible: dialog.stage === "review" || (dialog.stage === "done" && !!line.modelData.report && line.modelData.report.groups.length + line.modelData.report.failures.length > 0)
                                    flat: true
                                    onClicked: line.open = !line.open
                                }
                            }
                            UninstallPreview {
                                visible: line.open && dialog.stage === "review"
                                active: dialog.opened && visible
                                host: dialog.host
                                app: line.modelData.app
                                mode: dialog.mode
                                Layout.fillWidth: true
                            }
                            RemovalResult {
                                visible: line.open && dialog.stage === "done"
                                host: dialog.host
                                report: line.modelData.report
                                unfolded: false
                                showMessage: false
                                Layout.fillWidth: true
                            }
                        }
                    }
                }

                // ---- working: what runs now, and how far the queue is
                ColumnLayout {
                    visible: dialog.stage === "working"
                    spacing: 6
                    Layout.fillWidth: true

                    UqLabel {
                        text: dialog.running === null ? "" : "Uninstalling " + dialog.running.app.name + (dialog.single ? "…" : " (" + (dialog.finishedCount + 1) + " of " + dialog.entries.length + ")…")
                        wrapMode: Text.Wrap
                        Layout.fillWidth: true
                    }
                    QQC2.ProgressBar {
                        indeterminate: true
                        Layout.fillWidth: true
                    }
                    QQC2.ProgressBar {
                        visible: !dialog.single
                        from: 0
                        to: Math.max(1, dialog.entries.length)
                        value: dialog.finishedCount
                        Layout.fillWidth: true
                    }
                    UqLabel {
                        text: dialog.host && dialog.host.busy ? Logic.workingText(dialog.host.workingCommand) : ""
                        visible: text !== ""
                        wrapMode: Text.Wrap
                        opacity: 0.7
                        Layout.fillWidth: true
                    }
                }

                // ---- done, one app: what was removed, right there
                RemovalResult {
                    visible: dialog.single && dialog.stage === "done"
                    host: dialog.host
                    report: dialog.single ? dialog.entries[0].report : null
                    Layout.fillWidth: true
                }
            }
        }

        RowLayout {
            spacing: 8
            Layout.fillWidth: true

            UqLabel {
                text: dialog.stage !== "review" ? "" : dialog.single ? "You'll be asked once more." : "You'll be asked once for each app."
                opacity: 0.7
                wrapMode: Text.Wrap
                Layout.fillWidth: true
            }
            QQC2.Button {
                text: "Stop after this one"
                visible: dialog.stage === "working" && !dialog.single && dialog.entries.some(e => e.state === "waiting")
                enabled: !dialog.stopping
                onClicked: dialog.stopping = true
            }
            QQC2.Button {
                text: dialog.openCount === 1 ? "Close it and uninstall" : "Close them and uninstall"
                visible: dialog.stage === "done" && dialog.openCount > 0
                enabled: !!dialog.host && !dialog.host.busy
                onClicked: dialog.retryClosing()
            }
            QQC2.Button {
                text: "Cancel"
                visible: dialog.stage === "review"
                onClicked: dialog.close()
            }
            QQC2.Button {
                text: dialog.mode === "leftovers" ? (dialog.single ? "Remove leftovers" : "Remove their leftovers")
                    : (dialog.single ? "Uninstall" : "Uninstall " + Logic.plural(dialog.entries.length, "app", "apps"))
                visible: dialog.stage === "review"
                enabled: !!dialog.host && !dialog.host.busy && (!dialog.single || onlyPreview.ready)
                onClicked: dialog.start()
            }
            QQC2.Button {
                text: "OK"
                visible: dialog.stage === "done"
                onClicked: dialog.close()
            }
        }
    }
}
