// Apps and their size: `apps` runs when the tab is first opened, and on Refresh. The apps are
// sorted into menu categories (Internet, Games, ...), each a section that folds away; every app
// is a tile (AppTile) with its icon, name, size and its own Uninstall… button right there, so a
// button can't be mistaken for its neighbour's. Uninstall… opens a preview of everything that
// would go (UninstallPreview), and only its own button removes anything. The desktop's own packages say "Part of uQ" and have
// no button (crumBS would refuse anyway).
import QtQuick
import QtQuick.Controls as QQC2
import QtQuick.Layouts
import org.uq.settings
import "logic.js" as Logic

ColumnLayout {
    id: section

    property var host
    property bool loading: false
    property bool loaded: false
    property string problem: ""
    property var all: []
    readonly property var shown: Logic.filterApps(section.all, filter.text, 200)
    readonly property var groups: Logic.groupApps(section.shown.rows)
    /// The categories that are unfolded, by key. A search unfolds all.
    property var folded: ({})
    readonly property bool searching: filter.text.trim() !== ""
    readonly property string totalText: {
        var total = 0;
        for (var i = 0; i < section.all.length; i++)
            total += section.all[i].installSize || 0;
        return Logic.size(total);
    }

    /// Whether category `key` shows its apps: unfolded by hand, or by a search.
    function isOpen(key) {
        return section.searching || section.folded[key] === true;
    }

    /// The first time the apps arrive, the biggest category is unfolded, so the tab opens on
    /// something; after that only the person's own choices count.
    function unfoldBiggest() {
        if (Object.keys(section.folded).length > 0)
            return;
        let biggest = null;
        for (const g of Logic.groupApps(section.all))
            if (biggest === null || g.total > biggest.total)
                biggest = g;
        if (biggest !== null)
            section.folded = { [biggest.key]: true };
    }

    function toggle(key) {
        const next = Object.assign({}, section.folded);
        next[key] = !section.isOpen(key);
        section.folded = next;
    }
    /// The id of the app whose preview is open, "" when none.
    property string previewing: ""

    /// Reads when the tab is first shown, also when Settings opens the page on this tab.
    function autoload() {
        if (section.visible && !section.loaded && !section.loading)
            section.refresh();
    }

    onVisibleChanged: section.autoload()

    Connections {
        target: section.host
        function onCtxChanged() {
            section.autoload();
        }
    }

    function refresh() {
        if (!section.host || !section.host.ctx || section.loading)
            return;
        section.loading = true;
        section.host.ctx.command.run("apps", [], result => {
            section.loading = false;
            if (Logic.isCancelled(result))
                return;
            const env = Logic.envelope(result);
            section.loaded = env.data !== null;
            section.problem = env.data !== null ? "" : (env.cli || env.error || "crumBS gave an answer this page can't read.");
            section.all = Logic.apps(env.data);
            section.unfoldBiggest();
        });
    }

    spacing: 8

    SettingRow {
        settingId: "apps"
        label: section.loaded ? Logic.plural(section.all.length, "app", "apps") + " · " + section.totalText : "Apps and their size"
        highlight: section.host ? section.host.highlight : ""
        Layout.fillWidth: true

        QQC2.Button {
            text: section.loaded ? "Refresh" : "List apps"
            enabled: !section.loading
            onClicked: section.refresh()
        }
    }
    ColumnLayout {
        visible: section.loading
        spacing: 4
        Layout.fillWidth: true

        QQC2.ProgressBar {
            indeterminate: true
            Layout.fillWidth: true
        }
        UqLabel {
            text: "Listing your apps and measuring them…"
            opacity: 0.7
        }
    }
    Outcome {
        host: section.host
        noteKey: "section:apps"
        Layout.fillWidth: true
    }
    UqLabel {
        text: "AppImages kept by appOFholding are managed there."
        wrapMode: Text.Wrap
        opacity: 0.7
        Layout.fillWidth: true
    }
    ErrorCard {
        title: "Couldn't list your apps"
        detail: section.problem
        visible: section.problem !== ""
        Layout.fillWidth: true
    }
    QQC2.TextField {
        id: filter
        placeholderText: "Filter by name"
        visible: section.loaded
        Layout.fillWidth: true
    }
    UqLabel {
        text: "No apps match."
        visible: section.loaded && section.shown.rows.length === 0
        opacity: 0.7
    }
    Repeater {
        model: section.groups

        ColumnLayout {
            id: group

            required property var modelData
            readonly property bool open: section.isOpen(group.modelData.key)

            spacing: 8
            Layout.fillWidth: true
            Layout.topMargin: 6

            // The category's header: click anywhere on it to fold or unfold its apps.
            Item {
                implicitHeight: header.implicitHeight + 12
                Layout.fillWidth: true

                MouseArea {
                    anchors.fill: parent
                    enabled: !section.searching
                    cursorShape: Qt.PointingHandCursor
                    onClicked: section.toggle(group.modelData.key)
                }
                RowLayout {
                    id: header
                    anchors.fill: parent
                    anchors.topMargin: 6
                    anchors.bottomMargin: 6
                    spacing: 10

                    UqLabel {
                        text: group.open ? "▾" : "▸"
                        opacity: section.searching ? 0.3 : 0.7
                        Layout.preferredWidth: 14
                    }
                    UqAppIcon {
                        name: group.modelData.icon
                        label: group.modelData.title
                        size: 20
                    }
                    UqLabel {
                        text: group.modelData.title
                        font.bold: true
                    }
                    UqLabel {
                        text: group.modelData.countText + " · " + group.modelData.totalText
                        opacity: 0.7
                        Layout.fillWidth: true
                    }
                }
            }
            GridLayout {
                visible: group.open
                // Two tiles to a line when each can be wide enough to read, one otherwise. An
                // open preview takes the whole line (see AppTile).
                columns: width >= 560 ? 2 : 1
                columnSpacing: 12
                rowSpacing: 12
                Layout.fillWidth: true

                Repeater {
                    model: group.open ? group.modelData.rows : []

                    AppTile {
                        required property var modelData
                        host: section.host
                        app: modelData
                        previewing: section.previewing === modelData.id
                        onToggled: section.previewing = section.previewing === modelData.id ? "" : modelData.id
                        onDone: {
                            section.previewing = "";
                            section.refresh();
                        }
                        Layout.fillWidth: true
                        Layout.columnSpan: previewing ? 2 : 1
                        Layout.alignment: Qt.AlignTop
                    }
                }
            }
        }
    }
    UqLabel {
        text: "Showing the first 200. Type to narrow the list."
        visible: section.shown.more
        opacity: 0.7
    }
}
