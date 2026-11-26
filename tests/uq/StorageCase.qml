// The Storage and cleanup page's TestCase: finders that verify, the uninstall dialog, and a fresh
// page for every test (init). `harness` is the StorageHarness the tests run in.
import QtQuick
import QtTest

TestCase {
    id: tc

    /// The StorageHarness: the fake ctx, the page's Loader and the finders.
    property var harness

    when: windowShown

    // The button that sits in the same row as the label `labelText`.
    function rowButton(labelText, buttonText) {
        const labels = harness.findLabels(harness.loader.item, labelText);
        verify(labels.length > 0, "a row labelled " + labelText);
        // An app's button is in its tile's row, one level above the label's own column. The
        // pie's legend repeats a category's name, without a button.
        let button = null;
        for (const label of labels)
            button = button || harness.findButton(label.parent, buttonText) || harness.findButton(label.parent.parent, buttonText);
        verify(button !== null, labelText + " has a " + buttonText + " button");
        return button;
    }

    function appsSection() {
        return harness.all(harness.loader.item, []).find(i => i.dialog !== undefined && i.queue !== undefined);
    }

    /// The uninstall dialog's content (a popup, so not under the page's own items).
    function dialogBody() {
        return appsSection().dialog.contentItem;
    }

    function inDialog(text) {
        return () => harness.findLabels(dialogBody(), text).some(l => l.visible);
    }

    function dialogButton(text) {
        const b = harness.findButtons(dialogBody(), text).find(i => i.visible);
        verify(b !== undefined, "the dialog has a visible " + text + " button");
        return b;
    }

    /// Opens Firefox's uninstall dialog, waits for the plan, and presses its Uninstall.
    function uninstallFirefox() {
        rowButton("Firefox Web Browser", "Uninstall…").clicked();
        tryVerify(() => appsSection().dialog.opened);
        tryVerify(inDialog("Frees about 362.0 MB"));
        const go = dialogButton("Uninstall");
        tryVerify(() => go.enabled);
        go.clicked();
    }

    function init() {
        harness.fake.calls = [];
        harness.fake.confirms = true;
        harness.fake.hold = [];
        harness.fake.held = {};
        harness.fake.commands = harness.defaults();
        harness.loader.source = "";
        harness.loader.source = harness.pageUrl;
        verify(harness.loader.status === Loader.Ready, "the page loads");
        harness.loader.item.ctx = harness.fake.ctx;
    }
}
