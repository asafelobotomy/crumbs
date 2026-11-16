// Preferences: crumBS's own settings file, read with `crumbs config show` (followed from the
// first time the tab opens) and changed one key at a time with `crumbs config set|add|remove|
// reset`. crumBS checks every value before it writes, so a refusal is shown under its row and
// the file is left as it was.
import QtQuick
import QtQuick.Controls as QQC2
import QtQuick.Layouts
import org.uq.settings
import "logic.js" as Logic

ColumnLayout {
    id: section

    property var host
    property var view: null
    property var watching: null

    function show(result) {
        if (Logic.isCancelled(result) || (result && result.error === "already running"))
            return;
        section.view = Logic.configView(result);
    }

    function start() {
        if (section.host && section.host.ctx && section.watching === null)
            section.watching = section.host.ctx.command.watch("config", section.show);
    }

    /// Follows the file from the first time the tab is shown, also when Settings opens the page
    /// on this tab.
    function autoload() {
        if (section.visible)
            section.start();
    }

    onVisibleChanged: section.autoload()

    Connections {
        target: section.host
        function onCtxChanged() {
            section.autoload();
        }
    }

    spacing: 16

    ErrorCard {
        title: "Couldn't read crumBS's settings"
        detail: section.view ? section.view.problem : ""
        visible: section.view !== null && section.view.problem !== ""
        Layout.fillWidth: true
    }

    SettingsCard {
        title: "What a cleanup includes"
        iconName: "view-filter"
        help: "Switched-off kinds are left out of the space you can free and of Clean up."
        wide: true
        visible: section.view !== null && section.view.problem === ""
        Layout.fillWidth: true

        SettingRow {
            settingId: "preferences"
            label: "Kinds of files crumBS looks for"
            highlight: section.host ? section.host.highlight : ""
            Layout.fillWidth: true
        }
        Repeater {
            model: section.view !== null && section.view.problem === "" ? Logic.userCategories() : []

            ColumnLayout {
                id: cat

                required property var modelData
                readonly property string key: "pref:category:" + cat.modelData.category

                spacing: 2
                Layout.fillWidth: true

                QQC2.Switch {
                    id: toggle
                    text: cat.modelData.label
                    checked: Logic.categoryOn(cat.modelData.category, section.view)
                    enabled: !!section.host && !section.host.busy
                    onToggled: {
                        const want = toggle.checked;
                        // Show crumBS's answer, not the click: the file is followed, so the
                        // switch moves once the change is written.
                        toggle.checked = Qt.binding(() => Logic.categoryOn(cat.modelData.category, section.view));
                        section.host.change(cat.key, Logic.categorySteps(cat.modelData.category, want, section.view));
                    }
                }
                Outcome {
                    host: section.host
                    noteKey: cat.key
                    Layout.fillWidth: true
                }
            }
        }
    }

    SettingsCard {
        title: "Ages and limits"
        iconName: "chronometer"
        help: "When files count as old, and how much is kept."
        wide: true
        visible: section.view !== null && section.view.problem === ""
        Layout.fillWidth: true

        Repeater {
            model: section.view ? section.view.numbers : []

            ColumnLayout {
                id: num

                required property var modelData
                readonly property string key: "pref:" + num.modelData.key

                spacing: 2
                Layout.fillWidth: true

                SettingRow {
                    label: num.modelData.label
                    Layout.fillWidth: true

                    QQC2.SpinBox {
                        id: spin
                        visible: !num.modelData.isSize
                        from: num.modelData.min
                        to: 3650
                        value: num.modelData.isSize ? 0 : num.modelData.value
                        editable: true
                        enabled: !!section.host && !section.host.busy
                        onValueModified: section.host.change(num.key, [["config-set", [num.modelData.key, String(spin.value)]]])
                    }
                    UqLabel {
                        text: num.modelData.unit
                        visible: !num.modelData.isSize && text !== ""
                    }
                    QQC2.TextField {
                        id: sizeField
                        visible: num.modelData.isSize
                        text: num.modelData.isSize ? num.modelData.value : ""
                        placeholderText: "500M"
                        inputMethodHints: Qt.ImhNoPredictiveText
                        Layout.preferredWidth: 100
                        onAccepted: section.host.change(num.key, [["config-set", [num.modelData.key, sizeField.text.trim().toUpperCase()]]])
                    }
                    QQC2.Button {
                        text: "Default"
                        flat: true
                        enabled: !!section.host && !section.host.busy
                        onClicked: section.host.change(num.key, [["config-reset", [num.modelData.key]]])
                    }
                }
                Outcome {
                    host: section.host
                    noteKey: num.key
                    Layout.fillWidth: true
                }
            }
        }
    }

    SettingsCard {
        title: "Folders"
        iconName: "folder"
        help: "Each entry starts with / or ~/."
        wide: true
        visible: section.view !== null && section.view.problem === ""
        Layout.fillWidth: true

        Repeater {
            model: section.view ? section.view.lists : []

            ColumnLayout {
                id: list

                required property var modelData
                readonly property string key: "pref:" + list.modelData.key

                spacing: 4
                Layout.fillWidth: true

                SettingRow {
                    label: list.modelData.label
                    Layout.fillWidth: true

                    HelpTip {
                        tip: list.modelData.help
                    }
                }
                UqLabel {
                    text: "None yet."
                    visible: list.modelData.entries.length === 0
                    opacity: 0.7
                }
                Repeater {
                    model: list.modelData.entries

                    RowLayout {
                        required property string modelData
                        spacing: 8
                        Layout.fillWidth: true

                        UqLabel {
                            text: modelData
                            wrapMode: Text.WrapAnywhere
                            Layout.fillWidth: true
                        }
                        QQC2.Button {
                            text: "Remove"
                            flat: true
                            enabled: !!section.host && !section.host.busy
                            onClicked: section.host.change(list.key, [["config-remove", [list.modelData.key, modelData]]])
                        }
                    }
                }
                RowLayout {
                    spacing: 8
                    Layout.fillWidth: true

                    QQC2.TextField {
                        id: entry
                        placeholderText: "~/folder"
                        inputMethodHints: Qt.ImhNoPredictiveText
                        Layout.fillWidth: true
                        onAccepted: add.clicked()
                    }
                    QQC2.Button {
                        id: add
                        text: "Add"
                        enabled: !!section.host && !section.host.busy && entry.text.trim() !== ""
                        onClicked: {
                            section.host.change(list.key, [["config-add", [list.modelData.key, entry.text.trim()]]]);
                            entry.text = "";
                        }
                    }
                }
                Outcome {
                    host: section.host
                    noteKey: list.key
                    Layout.fillWidth: true
                }
            }
        }
    }

    UqLabel {
        text: section.view && section.view.path !== "" ? "Saved in " + section.view.path : ""
        visible: text !== ""
        opacity: 0.6
    }
}
