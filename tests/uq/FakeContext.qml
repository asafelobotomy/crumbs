// A stand-in for the `ctx` uQ Settings gives a page (docs/settings-pages.md), for a page's own
// QML tests. Answers come later (never during the call), as in Settings.
//
//   FakeContext {
//       id: fake
//       commands: ({ status: { code: 0, output: { state: "running" } } })
//   }
//   ... page.ctx = fake.ctx; ... compare(fake.calls[0], ["run", "status", []]);
import QtQuick

QtObject {
    id: fake

    /// ctx.theme.
    property var theme: ({ colors: { text: "#202020", error: "#c00000" } })
    /// Command id -> the result its callback gets, or a function(args) returning it. A command
    /// not listed gets {error: "unknown command"}.
    property var commands: ({})
    /// Method or property name -> the result (`{value}` or `{error}`), or a function(args).
    property var dbus: ({})
    /// The person's answer to every `confirm` question: false gives {error, cancelled: true}.
    property bool confirms: true
    /// Command ids that have `confirm` in page.toml.
    property var confirmed: []
    /// What the page asked for, in order: ["run", id, args], ["call", method, args, target],
    /// ["get", property, target], ["openApp", desktopId], ["watch", id], ["unwatch", handle].
    property var calls: []
    /// handle -> {id, callback} for ctx.command.watch; change(id) runs them again.
    property var watches: ({})
    property int nextWatch: 0

    /// Pretend the files or signals of watched command `id` changed: it runs again.
    function change(id) {
        for (const h in fake.watches)
            if (fake.watches[h].id === id)
                fake.run(id, [], fake.watches[h].callback);
    }

    function answer(callback, result) {
        if (typeof callback === "function")
            Qt.callLater(() => callback(result));
    }

    function pick(table, key, args, missing) {
        const r = table[key];
        if (r === undefined)
            return missing;
        return typeof r === "function" ? r(args) : r;
    }

    function run(id, args, callback) {
        fake.calls.push(["run", id, args || []]);
        if (fake.confirmed.indexOf(id) >= 0 && !fake.confirms)
            return answer(callback, { error: "Cancelled", cancelled: true });
        answer(callback, pick(fake.commands, id, args || [], { error: "unknown command" }));
    }

    readonly property var ctx: ({
            theme: fake.theme,
            command: {
                run: (id, args, callback) => fake.run(id, args, callback),
                watch: (id, callback) => {
                    fake.calls.push(["watch", id]);
                    const handle = ++fake.nextWatch;
                    fake.watches[handle] = { id: id, callback: callback };
                    fake.run(id, [], callback);
                    return handle;
                },
                unwatch: handle => {
                    fake.calls.push(["unwatch", handle]);
                    delete fake.watches[handle];
                }
            },
            dbus: {
                call: (method, args, callback, target) => {
                    fake.calls.push(["call", method, args || [], target]);
                    fake.answer(callback, fake.pick(fake.dbus, method, args || [], { error: "not listed" }));
                },
                get: (property, callback, target) => {
                    fake.calls.push(["get", property, target]);
                    fake.answer(callback, fake.pick(fake.dbus, property, [], { error: "not listed" }));
                }
            },
            openApp: (desktopId, callback) => {
                fake.calls.push(["openApp", desktopId]);
                fake.answer(callback, /^[A-Za-z0-9_][A-Za-z0-9._-]*\.desktop$/.test(desktopId) ? { ok: true } : { error: "not an app id" });
            }
        })
}
