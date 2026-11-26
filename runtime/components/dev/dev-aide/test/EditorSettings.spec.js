// The editor settings (WorkspaceServices get#, store#EditorSettings and store#EditorKey, plans/design-dev-aide.md 5.1):
// the user's DEV_AIDE_EDITOR preference, a UserPreferenceKey; the defaults with none; the font size, tab size, wrap,
// white space and keymap stored, a size out of range and an unknown keymap refused; a command's keys rebound over the
// keymap, keys another command has or not in CodeMirror's form refused, and empty for the default back. Settings shows
// them in its one render (GroupServers.spec.js); m-code-editor applies them (WebrootCode.page.js, DevAide.page.js).
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
        const stored = call("store#EditorSettings", {fontSize: "16", tabSize: "2", wrap: "true", whitespace: "true", keymap: "default"})
        const tooBig = call("store#EditorSettings", {fontSize: "99"})
        const keymap = call("store#EditorSettings", {keymap: "emacs"})
        const after = call("get#EditorSettings")
        return {declared, before, stored, tooBig, keymap, after, preference: JSON.parse(ec.user.getPreference(PREFERENCE) || "{}")}
    },
    then({declared, before, stored, tooBig, keymap, after, preference}) {
        declared.enumTypeId == "UserPreferenceKey"
        before.errors == ""
        before.out.settings.fontSize == 13
        before.out.settings.tabSize == 4
        before.out.settings.keymap == "default"
        before.out.keymaps.includes("default")
        keysOf(before.out, "gotoFile") == "Mod-p"
        stored.errors == ""
        tooBig.errors.includes("fontSize is a whole number from 8 to 32")
        keymap.errors.includes("no keymap emacs")
        after.out.settings.fontSize == 16
        after.out.settings.tabSize == 2
        after.out.settings.wrap == true
        after.out.settings.whitespace == true
        preference.fontSize == 16
    }
})

feature("a command's keys rebound over the keymap; keys another has, or not a key, refused; empty sets the default back", {
    when() {
        const rebound = call("store#EditorKey", {command: "gotoFile", keys: "Mod-e"})
        const taken = call("store#EditorKey", {command: "save", keys: "Mod-e"})
        const notKeys = call("store#EditorKey", {command: "save", keys: "Mod-"})
        const unknown = call("store#EditorKey", {command: "nothing", keys: "Mod-q"})
        // the old key of the rebound command is free now
        const freed = call("store#EditorKey", {command: "find", keys: "Mod-p"})
        const settings = call("get#EditorSettings")
        const back = call("store#EditorKey", {command: "gotoFile", keys: ""})
        // the other settings are kept by a key's store
        return {rebound, taken, notKeys, unknown, freed, settings, back}
    },
    then({rebound, taken, notKeys, unknown, freed, settings, back}) {
        rebound.errors == ""
        keysOf(rebound.out, "gotoFile") == "Mod-e"
        taken.errors.includes("is the key of Go to file")
        notKeys.errors.includes("is not a key")
        unknown.errors.includes("no editor command nothing")
        freed.errors == ""
        settings.out.settings.keys.gotoFile == "Mod-e"
        settings.out.settings.keys.find == "Mod-p"
        settings.out.settings.fontSize == 16
        keysOf(back.out, "gotoFile") == "Mod-p"
        back.out.settings.keys.gotoFile == null
    }
})
