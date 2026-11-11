// The stub org.uq.settings module loads and each component has the documented API. A page's
// repo can drop this file; it tests the stub, not the page.
import QtQuick
import QtQuick.Layouts
import QtTest
import org.uq.settings

Item {
    width: 600
    height: 400

    ColumnLayout {
        anchors.fill: parent

        UqLabel {
            id: label
            text: "<b>x</b>"
        }
        UqComboBox {
            id: combo
            model: ["One", "Two"]
        }
        SettingRow {
            id: row
            settingId: "a"
            label: "A"
            UqLabel {
                text: "in the row"
            }
        }
        SwitchRow {
            id: switchRow
            label: "On"
            value: true
        }
        ValueSlider {
            id: slider
            label: "Level"
            value: 0.5
        }
        ErrorCard {
            id: card
            title: "Failed"
            detail: "why"
        }
    }

    TestCase {
        name: "StubComponents"
        when: windowShown

        function test_api() {
            compare(label.textFormat, Text.PlainText);
            compare(combo.currentText, "One");
            verify(!row.flashing);
            row.highlight = "a";
            verify(row.flashing);
            compare(switchRow.toggle.checked, true);
            compare(slider.slider.value, 0.5);
            compare(card.title, "Failed");
        }

        function test_switch_snaps_back_until_the_page_agrees() {
            let chosen = null;
            switchRow.chosen.connect(v => chosen = v);
            switchRow.toggle.toggle();
            switchRow.toggle.toggled();
            compare(chosen, false);
            compare(switchRow.toggle.checked, true, "shows value again");
        }
    }
}
