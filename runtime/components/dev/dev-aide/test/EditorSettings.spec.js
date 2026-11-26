// The editor settings (WorkspaceServices get#, store#EditorSettings and store#EditorKey, plans/design-dev-aide.md 5.1):
// the user's DEV_AIDE_EDITOR preference, a UserPreferenceKey; the defaults with none; the font size, tab size, wrap,
// white space, keymap (default, vscode or vim) and type check stored, a size out of range and an unknown keymap
// refused; a command's keys rebound over the keymap, keys another command has or not in CodeMirror's form refused, and
// empty for the default back. Settings shows them in its one render (GroupServers.spec.js); m-code-editor applies them
// (WebrootCode.page.js, DevAide.page.js).
// Run: ./moq test dev-aide --spec EditorSettings
const PREFERENCE = "DEV_AIDE_EDITOR"
const call = (verb, parameters) => {
    try {
        const out =
            ec.service
                .sync()
                .name("org.moqui.devaide.WorkspaceServices." + verb)
                .parameters(parameters || {})
                .call() || {}
        const errors = ec.message.getErrorsString()
        ec.message.clearErrors()
        return {out, errors}
    } catch (error) {
        ec.message.clearErrors()
        return {out: {}, errors: String(error.message)}
    }
}
const keysOf = (out, command) => ((out.commands || []).find(row => row.command == command) || {}).keys

spec({
    stepwise: true,
    setupSpec() {
        login("john.doe")
        ec.user.setPreference(PREFERENCE, "")
    },
    cleanupSpec() {
        ec.user.setPreference(PREFERENCE, "")
    }
})

feature("the settings are the defaults with no preference, and what is stored after", {
    when() {
        const declared = ec.entity.find("moqui.basic.Enumeration").condition("enumId", PREFERENCE).disableAuthz().one()
        const before = call("get#EditorSettings")
        const stored = call("store#EditorSettings", {fontSize: "16", tabSize: "2", wrap: "true", whitespace: "true", keymap: "vim", typeCheck: "true"})
        const tooBig = call("store#EditorSettings", {fontSize: "99"})
        const keymap = call("store#EditorSettings", {keymap: "emacs"})
        const after = call("get#EditorSettings")
        // a size given empty is its default: CodeMirror's tab size and indent; one not given is kept
        const cleared = call("store#EditorSettings", {tabSize: ""})
        return {declared, before, stored, tooBig, keymap, after, cleared, preference: JSON.parse(ec.user.getPreference(PREFERENCE) || "{}")}
    },
    then({declared, before, stored, tooBig, keymap, after, cleared, preference}) {
        declared.enumTypeId == "UserPreferenceKey"
        before.errors == ""
        before.out.settings.fontSize == 13
        before.out.settings.tabSize == null
        before.out.settings.keymap == "default"
        before.out.settings.typeCheck == false
        before.out.keymaps.includes("default") && before.out.keymaps.includes("vscode") && before.out.keymaps.includes("vim")
        keysOf(before.out, "undo") == "Mod-z"
        // the Workspace's own commands are shell key map actions, not rows here
        keysOf(before.out, "gotoFile") == null
        stored.errors == ""
        tooBig.errors.includes("fontSize is a whole number from 8 to 32")
        keymap.errors.includes("no keymap emacs")
        after.out.settings.fontSize == 16
        after.out.settings.tabSize == 2
        after.out.settings.wrap == true
        after.out.settings.whitespace == true
        after.out.settings.keymap == "vim"
        after.out.settings.typeCheck == true
        preference.fontSize == 16
        cleared.out.settings.tabSize == null
        cleared.out.settings.fontSize == 16
    }
})

feature("a command's keys rebound over the keymap; keys another has, or not a key, refused; empty sets the default back", {
    when() {
        const rebound = call("store#EditorKey", {command: "undo", keys: "Mod-e"})
        // the same key written another way, or as it is pressed on the platform, is the same key
        const taken = call("store#EditorKey", {command: "redo", keys: "ctrl-E"})
        const notKeys = call("store#EditorKey", {command: "redo", keys: "Mod-"})
        // Save is the shell's devSave, in the text editor too; and no editor command takes a shell action's keys
        const save = call("store#EditorKey", {command: "save", keys: "Alt-s"})
        const shell = call("store#EditorKey", {command: "redo", keys: "Mod-p"})
        const unknown = call("store#EditorKey", {command: "nothing", keys: "Mod-q"})
        // the old key of the rebound command is free now
        const freed = call("store#EditorKey", {command: "find", keys: "Mod-z"})
        const settings = call("get#EditorSettings")
        // its default back is refused while another command has it, then taken once that one is back too
        const held = call("store#EditorKey", {command: "undo", keys: ""})
        call("store#EditorKey", {command: "find", keys: ""})
        const back = call("store#EditorKey", {command: "undo", keys: ""})
        // the other settings are kept by a key's store
        return {rebound, taken, notKeys, save, shell, unknown, freed, settings, held, back}
    },
    then({rebound, taken, notKeys, save, shell, unknown, freed, settings, held, back}) {
        rebound.errors == ""
        keysOf(rebound.out, "undo") == "Mod-e"
        taken.errors.includes("is the key of Undo")
        notKeys.errors.includes("is not a key")
        unknown.errors.includes("no editor command nothing")
        save.errors.includes("no editor command save")
        shell.errors.includes("Go to file, a shell action")
        freed.errors == ""
        settings.out.settings.keys.undo == "Mod-e"
        settings.out.settings.keys.find == "Mod-z"
        settings.out.settings.fontSize == 16
        held.errors.includes("is the key of Find in the file")
        keysOf(back.out, "undo") == "Mod-z"
        back.out.settings.keys.undo == null
    }
})
