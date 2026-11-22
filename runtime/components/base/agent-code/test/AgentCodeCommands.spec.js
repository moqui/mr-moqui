// The Agent Code command pipeline and settings (plans/agent-code/02-Orchestration.md section 1, 10-Web-UI.md section 9):
// a command writes its thread, a change and a receipt in one transaction, and the same commandId answers the stored
// receipt again and writes nothing; another user's thread is not found to them, and a user with no AgentCodeUse is
// refused. A client setting is checked against the schema before it is stored, and a server setting needs
// AgentCodeAdmin.
// Run: ./moq test agent-code --spec AgentCodeCommands
const CMD = "org.moqui.code.CommandServices.dispatch#Command"
const SET = "org.moqui.code.SettingsServices."
const OWNER = "AGENT_CODE_SPEC_OWNER"
const OTHER = "AGENT_CODE_SPEC_OTHER"
const STRANGER = "AGENT_CODE_SPEC_STRANGER"
const THREAD = "agcspec-thread-1"

function sys(run) {
    const wasDisabled = ec.artifactExecution.disableAuthz()
    try {
        return run()
    } finally {
        if (!wasDisabled) ec.artifactExecution.enableAuthz()
    }
}
function makeUser(userId, group) {
    sys(() => {
        if (ec.entity.find("moqui.security.UserAccount").condition("userId", userId).count() == 0) {
            const account = ec.entity.makeValue("moqui.security.UserAccount")
            account.setFields({userId, username: userId.toLowerCase(), userFullName: userId, disabled: "N"}, true, null, null)
            account.create()
        }
        if (group && ec.entity.find("moqui.security.UserGroupMember").condition("userGroupId", group).condition("userId", userId).count() == 0) {
            const member = ec.entity.makeValue("moqui.security.UserGroupMember")
            member.setFields({userGroupId: group, userId, fromDate: ec.user.nowTimestamp}, true, null, null)
            member.create()
        }
    })
}
// A service call as `userId`: its result and its errors.
function as(userId, name, parameters) {
    ec.user.internalLoginUser(userId.toLowerCase())
    try {
        const result = ec.service.sync().name(name).parameters(parameters).call() || {}
        return {result, errors: ec.message.errorsString}
    } catch (err) {
        return {result: {}, errors: String(err.message || err)}
    } finally {
        ec.message.clearAll()
        ec.user.logoutUser()
    }
}
function clean() {
    sys(() => {
        for (const entity of ["moqui.agent.code.AgentCodeChange", "moqui.agent.code.AgentConversationUser"])
            ec.entity.find(entity).condition("conversationId", THREAD).deleteAll()
        ec.entity.find("moqui.agent.code.AgentCodeCommand").condition("commandId", "like", "agcspec-%").deleteAll()
        ec.entity.find("moqui.agent.AgentConversation").condition("conversationId", THREAD).deleteAll()
        ec.entity.find("moqui.security.UserPreference").condition("userId", OWNER).condition("preferenceKey", "like", "code.%").deleteAll()
    })
}

spec({
    setupSpec() {
        makeUser(OWNER, "AGENT_CODE_USERS")
        makeUser(OTHER, "AGENT_CODE_USERS")
        makeUser(STRANGER, null)
        clean()
    },
    cleanupSpec() {
        clean()
    }
})

feature("a command writes once and replays its receipt; another user's thread is not found, and no AgentCodeUse is refused", {
    when() {
        const create = {type: "thread.create", commandId: "agcspec-create-1", threadId: THREAD, title: "Spec thread"}
        const first = as(OWNER, CMD, {command: create})
        const again = as(OWNER, CMD, {command: create})
        const changes = sys(() => ec.entity.find("moqui.agent.code.AgentCodeChange").condition("conversationId", THREAD).count())
        const pinByOther = as(OTHER, CMD, {command: {type: "thread.pin", commandId: "agcspec-pin-1", threadId: THREAD}})
        const sameIdByOther = as(OTHER, CMD, {command: create})
        const stranger = as(STRANGER, CMD, {command: {type: "thread.create", commandId: "agcspec-create-2"}})
        const snapshot = as(OWNER, "org.moqui.code.ShellServices.get#ShellSnapshot", {})
        return {first, again, changes, pinByOther, sameIdByOther, stranger, snapshot}
    },
    then({first, again, changes, pinByOther, sameIdByOther, stranger, snapshot}) {
        first.errors == ""
        first.result.result.threadId == THREAD
        first.result.replayed == false
        again.result.replayed == true
        again.result.sequence == first.result.sequence
        changes == 1
        pinByOther.result.errorCode == "not_found"
        sameIdByOther.result.errorCode == "conflict"
        stranger.errors != ""
        snapshot.result.snapshot.threads.some(thread => thread.id == THREAD && thread.title == "Spec thread")
    }
})

feature("a client setting is checked against the schema, and a server setting needs AgentCodeAdmin", {
    when() {
        const stored = as(OWNER, SET + "update#Settings", {kind: "client", patch: {boringSidebarEnabled: true}})
        const wrongType = as(OWNER, SET + "update#Settings", {kind: "client", patch: {appearanceContrast: "loud"}})
        const unknown = as(OWNER, SET + "update#Settings", {kind: "client", patch: {noSuchSetting: 1}})
        const server = as(OWNER, SET + "update#Settings", {kind: "server", patch: {automaticGitFetchInterval: 60}})
        const read = as(OWNER, SET + "get#Settings", {kind: "client"})
        return {stored, wrongType, unknown, server, read}
    },
    then({stored, wrongType, unknown, server, read}) {
        stored.errors == ""
        wrongType.errors.includes("appearanceContrast")
        unknown.errors.includes("noSuchSetting")
        server.errors != ""
        read.result.settings.boringSidebarEnabled == true
        read.result.settings.appearanceContrast == 100
    }
})
