// A ring chart of what a space section found, by category (Overview.pieSlices): the biggest
// categories each get a slice and the rest join "Everything else". The legend beside it names
// every slice with its size and share, so no slice is told apart by colour alone; pointing at a
// slice (or its legend line) shows it in the middle of the ring.
import QtQuick
import QtQuick.Controls as QQC2
import QtQuick.Layouts
import org.uq.settings
import "logic.js" as Logic

import "overview.js" as Overview
RowLayout {
    id: pie

    property var host
    /// Logic.spaceView().
    property var view
    readonly property bool dark: {
        const colors = pie.host && pie.host.ctx && pie.host.ctx.theme ? pie.host.ctx.theme.colors : null;
        return Overview.isLightColor(colors ? colors.text : "");
    }
    readonly property var slices: Overview.pieSlices(pie.view ? pie.view.rows : [], pie.dark)
    /// The slice pointed at, -1 for none.
    property int hovered: -1

    visible: pie.slices.length > 0
    spacing: 24

    Item {
        id: chart

        readonly property real outer: Math.min(width, height) / 2
        readonly property real inner: outer * 0.62

        implicitWidth: 180
        implicitHeight: 180
        Layout.alignment: Qt.AlignTop
        Accessible.role: Accessible.Chart
        Accessible.name: "Space you can free, by kind: " + pie.slices.map(s => s.label + " " + s.sizeText).join(", ")

        Canvas {
            id: canvas
            anchors.fill: parent
            renderStrategy: Canvas.Cooperative

            onPaint: {
                const ctx = getContext("2d");
                ctx.reset();
                const cx = width / 2;
                const cy = height / 2;
                const gap = pie.slices.length > 1 ? 0.025 : 0;
                for (let i = 0; i < pie.slices.length; i++) {
                    const s = pie.slices[i];
                    const grow = i === pie.hovered ? 4 : 0;
                    const a0 = s.start - Math.PI / 2 + gap / 2;
                    const a1 = Math.max(a0 + 0.002, s.end - Math.PI / 2 - gap / 2);
                    ctx.beginPath();
                    ctx.arc(cx, cy, chart.outer - 4 + grow, a0, a1, false);
                    ctx.arc(cx, cy, chart.inner, a1, a0, true);
                    ctx.closePath();
                    ctx.fillStyle = s.color;
                    ctx.globalAlpha = pie.hovered < 0 || i === pie.hovered ? 1 : 0.45;
                    ctx.fill();
                }
            }
        }
        Connections {
            target: pie
            function onSlicesChanged() {
                canvas.requestPaint();
            }
            function onHoveredChanged() {
                canvas.requestPaint();
            }
        }
        MouseArea {
            anchors.fill: parent
            hoverEnabled: true
            onPositionChanged: mouse => pie.hovered = Overview.sliceAt(pie.slices, mouse.x, mouse.y, width / 2, height / 2, chart.inner, chart.outer)
            onExited: pie.hovered = -1
        }
        ColumnLayout {
            anchors.centerIn: parent
            width: chart.inner * 1.6
            spacing: 0

            UqLabel {
                text: pie.hovered >= 0 ? pie.slices[pie.hovered].sizeText : (pie.view ? pie.view.totalText : "")
                font.bold: true
                horizontalAlignment: Text.AlignHCenter
                Layout.fillWidth: true
            }
            UqLabel {
                text: pie.hovered >= 0 ? pie.slices[pie.hovered].label : "in total"
                horizontalAlignment: Text.AlignHCenter
                wrapMode: Text.Wrap
                maximumLineCount: 2
                elide: Text.ElideRight
                opacity: 0.7
                font.pixelSize: 11
                Layout.fillWidth: true
            }
        }
    }

    ColumnLayout {
        spacing: 6
        Layout.fillWidth: true
        Layout.alignment: Qt.AlignVCenter

        Repeater {
            model: pie.slices

            Item {
                id: entry

                required property var modelData
                required property int index

                implicitHeight: line.implicitHeight
                Layout.fillWidth: true

                RowLayout {
                    id: line
                    anchors.fill: parent
                    spacing: 8

                    Rectangle {
                        width: 12
                        height: 12
                        radius: 3
                        color: entry.modelData.color
                        Layout.alignment: Qt.AlignVCenter
                    }
                    UqLabel {
                        text: entry.modelData.label
                        font.bold: pie.hovered === entry.index
                        elide: Text.ElideRight
                        Layout.fillWidth: true
                    }
                    UqLabel {
                        text: entry.modelData.sizeText + " · " + entry.modelData.percentText
                        opacity: 0.7
                        font.bold: pie.hovered === entry.index
                    }
                }
                MouseArea {
                    anchors.fill: parent
                    hoverEnabled: true
                    onEntered: pie.hovered = entry.index
                    onExited: pie.hovered = -1
                }
            }
        }
    }
}
