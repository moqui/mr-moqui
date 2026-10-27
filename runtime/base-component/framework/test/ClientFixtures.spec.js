// The users the framework's client specs (test/client, moqui test-client) log in as: sec.view.only (VIEW on Tools and
// System), sec.none.only (no artifact authz) and the load user test.load (VIEW on System, MOQUI_API and the
// Enumeration entity, no tarpit of its own), with Moqui Java's passwords, made as SecurityTestSupport.ensureUsers made
// them in a Spock setupSpec (plans/archive/ClientTestHarness.md D10, 7). The HTTP security proofs also get sec.all.only
// (AUTHZA_ALL on Tools and System), sec.lock.test (the view group, unlocked on every run), sec.ent.view and
// sec.ent.all (narrow and catch-all AT_ENTITY), sec.api.view and sec.api.all (MOQUI_API), sec.ip.v4 and sec.ip.loop
// (ipAllowed only; login is not tried here), sec.key.http (a known login key), sec.mfa.totp (a single-use factor;
// password login is not tried here) and sec.tap.user (a service tarpit and the setPreference transition), plus the
// SEC_SMT_TEST system-message remotes. moqui test-client framework --serve runs this spec on
// its server's scratch database; for another server run it on that server's database:
//     moqui test framework --spec ClientFixtures --db existing
// Each row is made only when it is missing, so a database kept between runs takes it again. These users are not demo
// data, so no production load can reach them. ADMIN already has REST_SCHEMA in MoquiSetupData.xml, so it is not granted
// again here.

const USERS = [
    { username: "sec.view.only", password: "SecView1!!", group: "SEC_VIEW_GROUP" },
    { username: "sec.none.only", password: "SecNone1!!", group: "SEC_NONE_GROUP" },
    { username: "test.load", password: "TestLoad1!!", group: "TEST_LOAD_GROUP" },
    { username: "sec.all.only", password: "SecAll1!!", group: "SEC_ALL_GROUP" },
    { username: "sec.lock.test", password: "SecLock1!!", group: "SEC_VIEW_GROUP" },
    { username: "sec.ent.view", password: "SecEntView1!!", group: "SEC_ENT_VIEW_GROUP" },
    { username: "sec.ent.all", password: "SecEntAll1!!", group: "SEC_ENT_ALL_GROUP" },
    { username: "sec.api.view", password: "SecApiView1!!", group: "SEC_API_VIEW_GROUP" },
    { username: "sec.api.all", password: "SecApiAll1!!", group: "SEC_API_ALL_GROUP" },
    { username: "sec.key.http", password: "SecKeyHttp1!!", group: "SEC_ENT_VIEW_GROUP" },
    { username: "sec.tap.user", password: "SecTapUser1!!", group: "SEC_TAP_GROUP" },
]

// hash_login_key("sec-test-login-key-fixed-40-chars-value"): SHA-256 hex, empty salt (login-key/@encrypt-hash-type,
// SHA-256 in MoquiDefaultConf.xml). ec.user has no hashLoginKey; hashPassword is argon2 and singleUseCodeHash salts
// with SaltySalt, so neither is the hash login_user_key looks up. The api_key / login_key header sends the plaintext.
const LOGIN_KEY_HASH = "27d4afeaef1da1588baac5f07ee8feaecf7e67e10947764177bd98c8e234357b"
const MFA_CODE = "87654321"
// AT_SERVICE push name is the service's own name. The transition frame for POST /apps/setPreference is
// "{screen.location}/{transition}" (screen/render.rs), this location, not the "location#name" artifact-hit name.
const TAP_SVC = "org.moqui.impl.UserServices.set#Preference"
const TAP_TRANS = "component://webroot/screen/webroot/apps.xml/setPreference"

const one = (entity, fields) => {
    let find = ec.entity.find(entity)
    for (const [name, value] of Object.entries(fields)) find = find.condition(name, value)
    return find.one()
}
const ensure = (entity, key, fields) => {
    if (one(entity, key) != null) return
    const row = ec.entity.makeValue(entity)
    row.setFields(Object.assign({}, key, fields || {}), true, null, null)
    row.create()
}
const ensureGroup = (userGroupId, description) => ensure("moqui.security.UserGroup", { userGroupId }, { description })
const ensureAuthz = (artifactAuthzId, userGroupId, artifactGroupId, authzActionEnumId) =>
    ensure("moqui.security.ArtifactAuthz", { artifactAuthzId },
        { userGroupId, artifactGroupId, authzTypeEnumId: "AUTHZT_ALWAYS", authzActionEnumId })
const ensureMember = (artifactGroupId, artifactName, artifactTypeEnumId, nameIsPattern) =>
    ensure("moqui.security.ArtifactGroupMember",
        { artifactGroupId, artifactName, artifactTypeEnumId }, { nameIsPattern })
// A new transaction, as Java's resetLockAccount: the unlock has to commit even when the caller's transaction is gone.
const updateAccount = (parameters) => {
    ec.message.clearErrors()
    const before = ec.message.errors || 0
    const out = ec.service.sync().name("update#moqui.security.UserAccount").parameters(parameters)
        .disableAuthz().requireNewTransaction(true).call()
    if ((ec.message.errors || 0) > before || out == null) {
        const errors = ec.message.getErrorsString()
        throw new Error(`update#UserAccount failed${errors ? ": " + errors : ""}`)
    }
    ec.message.clearErrors()
}
const ensureUser = ({ username, password, group }) => {
    const existing = one("moqui.security.UserAccount", { username })
    const userId = existing != null ? existing.userId : call("org.moqui.impl.UserServices.create#UserAccount",
        { username, newPassword: password, newPasswordVerify: password, userFullName: username }).userId
    if (one("moqui.security.UserGroupMember", { userId, userGroupId: group }) == null) {
        call("create#moqui.security.UserGroupMember", { userId, userGroupId: group, fromDate: ec.user.nowTimestamp })
    }
    if (username === "sec.lock.test") {
        updateAccount({ userId, disabled: "N", successiveFailedLogins: 0, disabledDateTime: null })
    }
    return userId
}
const ensureLoginKey = (userId) => {
    if (one("moqui.security.UserLoginKey", { loginKey: LOGIN_KEY_HASH }) != null) return
    const fromDate = ec.user.nowTimestamp
    const thruDate = new Date(ec.user.timestampMillis(fromDate) + 365 * 24 * 60 * 60 * 1000)
    const row = ec.entity.makeValue("moqui.security.UserLoginKey")
    row.setFields({
        loginKey: LOGIN_KEY_HASH, userId, fromDate, thruDate, description: "security test known login key",
    }, true, null, null)
    row.create()
}
// Consuming the code sets thruDate; a later run puts the same hash back and clears thruDate.
const ensureMfa = (userId) => {
    const hashed = ec.user.singleUseCodeHash(MFA_CODE)
    const existing = one("moqui.security.UserAuthcFactor", { factorId: "SEC_MFA_TOTP_FACTOR" })
    if (existing == null) {
        const row = ec.entity.makeValue("moqui.security.UserAuthcFactor")
        row.setFields({
            factorId: "SEC_MFA_TOTP_FACTOR", userId, factorTypeEnumId: "UafSingleUse", factorOption: hashed,
            fromDate: ec.user.nowTimestamp, needsValidation: "N",
        }, true, null, null)
        row.create()
        return
    }
    existing.userId = userId
    existing.factorTypeEnumId = "UafSingleUse"
    existing.factorOption = hashed
    existing.thruDate = null
    existing.needsValidation = "N"
    existing.update()
}

spec({
    setupSpec() {
        ec.artifactExecution.disableAuthz()
        try {
            ensureGroup("SEC_VIEW_GROUP", "Security test VIEW-only")
            ensureAuthz("SEC_VIEW_TOOLS", "SEC_VIEW_GROUP", "TOOLS_APP", "AUTHZA_VIEW")
            ensureAuthz("SEC_VIEW_SYS", "SEC_VIEW_GROUP", "SYSTEM_APP", "AUTHZA_VIEW")
            ensureGroup("SEC_NONE_GROUP", "Security test no artifact authz")
            ensureGroup("SEC_ALL_GROUP", "Security test AUTHZA_ALL")
            ensureAuthz("SEC_ALL_TOOLS", "SEC_ALL_GROUP", "TOOLS_APP", "AUTHZA_ALL")
            ensureAuthz("SEC_ALL_SYS", "SEC_ALL_GROUP", "SYSTEM_APP", "AUTHZA_ALL")

            ensureGroup("SEC_ENT_VIEW_GROUP", "Security test AT_ENTITY narrow VIEW")
            ensureGroup("SEC_ENT_ALL_GROUP", "Security test AT_ENTITY catch-all ALL")
            ensure("moqui.security.ArtifactGroup", { artifactGroupId: "SEC_ENT_NARROW" }, { description: "Security test one entity" })
            ensureMember("SEC_ENT_NARROW", "moqui.basic.Enumeration", "AT_ENTITY", "N")
            ensure("moqui.security.ArtifactGroup", { artifactGroupId: "SEC_ENT_CATCHALL" }, { description: "Security test all entities" })
            ensureMember("SEC_ENT_CATCHALL", ".*", "AT_ENTITY", "Y")
            ensureAuthz("SEC_ENT_VIEW_AUTHZ", "SEC_ENT_VIEW_GROUP", "SEC_ENT_NARROW", "AUTHZA_VIEW")
            ensureAuthz("SEC_ENT_ALL_AUTHZ", "SEC_ENT_ALL_GROUP", "SEC_ENT_CATCHALL", "AUTHZA_ALL")

            ensureGroup("SEC_API_VIEW_GROUP", "Security test MOQUI_API VIEW")
            ensureGroup("SEC_API_ALL_GROUP", "Security test MOQUI_API ALL")
            ensureAuthz("SEC_API_VIEW_AUTHZ", "SEC_API_VIEW_GROUP", "MOQUI_API", "AUTHZA_VIEW")
            ensureAuthz("SEC_API_ALL_AUTHZ", "SEC_API_ALL_GROUP", "MOQUI_API", "AUTHZA_ALL")

            ensureGroup("SEC_TAP_GROUP", "Security test tarpit")
            ensure("moqui.security.ArtifactGroup", { artifactGroupId: "SEC_TAP_SVC" }, { description: "Security test service tarpit" })
            ensureMember("SEC_TAP_SVC", TAP_SVC, "AT_SERVICE", "N")
            ensure("moqui.security.ArtifactGroup", { artifactGroupId: "SEC_TAP_TRANS" }, { description: "Security test transition tarpit" })
            ensureMember("SEC_TAP_TRANS", TAP_TRANS, "AT_XML_SCREEN_TRANS", "N")
            ensureAuthz("SEC_TAP_SVC_AUTHZ", "SEC_TAP_GROUP", "SEC_TAP_SVC", "AUTHZA_ALL")
            ensureAuthz("SEC_TAP_TRANS_AUTHZ", "SEC_TAP_GROUP", "SEC_TAP_TRANS", "AUTHZA_ALL")
            ensure("moqui.security.ArtifactTarpit", { userGroupId: "SEC_TAP_GROUP", artifactGroupId: "SEC_TAP_SVC" },
                { maxHitsCount: 3, maxHitsDuration: 60, tarpitDuration: 30 })
            ensure("moqui.security.ArtifactTarpit", { userGroupId: "SEC_TAP_GROUP", artifactGroupId: "SEC_TAP_TRANS" },
                { maxHitsCount: 3, maxHitsDuration: 60, tarpitDuration: 30 })

            ensureGroup("TEST_LOAD_GROUP", "Load test user")
            ensureAuthz("TEST_LOAD_SYS", "TEST_LOAD_GROUP", "SYSTEM_APP", "AUTHZA_VIEW")
            // /rest/s1/moqui/... (runtime/base-component/tools/data/ToolsSecurityData.xml)
            ensureAuthz("TEST_LOAD_API", "TEST_LOAD_GROUP", "MOQUI_API", "AUTHZA_VIEW")
            // /rest/e1/moqui.basic.Enumeration
            ensure("moqui.security.ArtifactGroup", { artifactGroupId: "TEST_LOAD_ENUM" }, { description: "Load test Enumeration entity" })
            ensure("moqui.security.ArtifactGroupMember",
                { artifactGroupId: "TEST_LOAD_ENUM", artifactName: "moqui.basic.Enumeration", artifactTypeEnumId: "AT_ENTITY" },
                { nameIsPattern: "N" })
            ensureAuthz("TEST_LOAD_ENUM_AUTHZ", "TEST_LOAD_GROUP", "TEST_LOAD_ENUM", "AUTHZA_VIEW")

            for (const user of USERS) ensureUser(user)
            const ipV4 = ensureUser({ username: "sec.ip.v4", password: "SecIpV41!!", group: "SEC_NONE_GROUP" })
            const ipLoop = ensureUser({ username: "sec.ip.loop", password: "SecIpLoop1!!", group: "SEC_NONE_GROUP" })
            updateAccount({ userId: ipV4, ipAllowed: "10.99.99.99" })
            updateAccount({ userId: ipLoop, ipAllowed: "127.0.0.1,::1" })
            ensureLoginKey(one("moqui.security.UserAccount", { username: "sec.key.http" }).userId)
            const mfaId = ensureUser({ username: "sec.mfa.totp", password: "SecMfaTotp1!!", group: "SEC_NONE_GROUP" })
            ensureMfa(mfaId)

            ensure("moqui.service.message.SystemMessageType", { systemMessageTypeId: "SEC_SMT_TEST" },
                { description: "Security test message type" })
            ensure("moqui.service.message.SystemMessageRemote", { systemMessageRemoteId: "SEC_SMR_HMAC" }, {
                description: "Security test HMAC", systemMessageTypeId: "SEC_SMT_TEST", messageAuthEnumId: "SmatHmacSha256",
                authHeaderName: "X-Moqui-Signature", sharedSecret: "sec-hmac-test-secret",
            })
            ensure("moqui.service.message.SystemMessageRemote", { systemMessageRemoteId: "SEC_SMR_HMAC_TS" }, {
                description: "Security test HMAC timestamp", systemMessageTypeId: "SEC_SMT_TEST",
                messageAuthEnumId: "SmatHmacSha256Timestamp", authHeaderName: "X-Moqui-Signature",
                sharedSecret: "sec-hmac-test-secret",
            })
            ensure("moqui.service.message.SystemMessageRemote", { systemMessageRemoteId: "SEC_SMR_NONE" }, {
                description: "Security test no auth", systemMessageTypeId: "SEC_SMT_TEST", messageAuthEnumId: "SmatNone",
            })
        } finally {
            ec.artifactExecution.enableAuthz()
        }
    },
})

feature("#username is in #group and logs in with its password", {
    where: USERS,
    when({ username, password, group }) {
        ec.artifactExecution.disableAuthz()
        let account, member
        try {
            account = one("moqui.security.UserAccount", { username })
            member = account == null ? null : one("moqui.security.UserGroupMember", { userId: account.userId, userGroupId: group })
        } finally {
            ec.artifactExecution.enableAuthz()
        }
        const loggedIn = ec.user.loginUser(username, password)
        const userId = ec.user.userId
        if (loggedIn) logout()
        return { account, member, loggedIn, userId }
    },
    then({ account, member, loggedIn, userId }) {
        account != null
        account.disabled != "Y"
        member != null
        loggedIn
        userId == account.userId
    },
})

// loginUser outside a request has an empty client address, and an empty address skips ipAllowed, so a passing
// login would not prove the allow list. sec.ip.v4 is not this process either way. sec.mfa.totp is separate:
// loginUser returns false while a second factor is still required.
feature("#username allows only #ipAllowed and is not logged in here", {
    where: [
        { username: "sec.ip.v4", ipAllowed: "10.99.99.99" },
        { username: "sec.ip.loop", ipAllowed: "127.0.0.1,::1" },
    ],
    when({ username }) {
        ec.artifactExecution.disableAuthz()
        try {
            const account = one("moqui.security.UserAccount", { username })
            const member = account == null ? null : one("moqui.security.UserGroupMember",
                { userId: account.userId, userGroupId: "SEC_NONE_GROUP" })
            return { account, member }
        } finally {
            ec.artifactExecution.enableAuthz()
        }
    },
    then({ account, member, ipAllowed }) {
        account != null
        account.disabled != "Y"
        account.ipAllowed == ipAllowed
        member != null
    },
})

feature("the single-use factor is restored and a password alone is not logged in", {
    when() {
        ec.artifactExecution.disableAuthz()
        try {
            const account = one("moqui.security.UserAccount", { username: "sec.mfa.totp" })
            const member = account == null ? null : one("moqui.security.UserGroupMember",
                { userId: account.userId, userGroupId: "SEC_NONE_GROUP" })
            const factor = one("moqui.security.UserAuthcFactor", { factorId: "SEC_MFA_TOTP_FACTOR" })
            return { account, member, factor, hashed: ec.user.singleUseCodeHash(MFA_CODE) }
        } finally {
            ec.artifactExecution.enableAuthz()
        }
    },
    then({ account, member, factor, hashed }) {
        account != null
        member != null
        factor != null
        factor.userId == account.userId
        factor.factorTypeEnumId == "UafSingleUse"
        factor.factorOption == hashed
        factor.needsValidation == "N"
        factor.thruDate == null
    },
})

feature("the access each user is given", {
    when() {
        ec.artifactExecution.disableAuthz()
        try {
            const authz = (artifactAuthzId) => one("moqui.security.ArtifactAuthz", { artifactAuthzId })
            const tarpits = (userGroupId) => ec.entity.find("moqui.security.ArtifactTarpit").condition("userGroupId", userGroupId).count()
            const authzOf = (userGroupId) => ec.entity.find("moqui.security.ArtifactAuthz").condition("userGroupId", userGroupId).count()
            return {
                viewTools: authz("SEC_VIEW_TOOLS"), viewSys: authz("SEC_VIEW_SYS"), noneAuthz: authzOf("SEC_NONE_GROUP"),
                loadSys: authz("TEST_LOAD_SYS"), loadApi: authz("TEST_LOAD_API"), loadEnum: authz("TEST_LOAD_ENUM_AUTHZ"),
                loadTarpits: tarpits("TEST_LOAD_GROUP"),
            }
        } finally {
            ec.artifactExecution.enableAuthz()
        }
    },
    then({ viewTools, viewSys, noneAuthz, loadSys, loadApi, loadEnum, loadTarpits }) {
        viewTools.artifactGroupId == "TOOLS_APP"
        viewTools.authzActionEnumId == "AUTHZA_VIEW"
        viewSys.artifactGroupId == "SYSTEM_APP"
        viewSys.authzActionEnumId == "AUTHZA_VIEW"
        noneAuthz == 0
        loadSys.artifactGroupId == "SYSTEM_APP"
        loadApi.artifactGroupId == "MOQUI_API"
        loadApi.authzActionEnumId == "AUTHZA_VIEW"
        loadEnum.artifactGroupId == "TEST_LOAD_ENUM"
        loadTarpits == 0
    },
})

feature("the security proof fixtures", {
    when() {
        ec.artifactExecution.disableAuthz()
        try {
            const authz = (artifactAuthzId) => one("moqui.security.ArtifactAuthz", { artifactAuthzId })
            const member = (artifactGroupId, artifactName) => one("moqui.security.ArtifactGroupMember", { artifactGroupId, artifactName })
            const tarpit = (artifactGroupId) => one("moqui.security.ArtifactTarpit",
                { userGroupId: "SEC_TAP_GROUP", artifactGroupId })
            const remote = (systemMessageRemoteId) => one("moqui.service.message.SystemMessageRemote", { systemMessageRemoteId })
            const lock = one("moqui.security.UserAccount", { username: "sec.lock.test" })
            const keyAccount = one("moqui.security.UserAccount", { username: "sec.key.http" })
            return {
                allTools: authz("SEC_ALL_TOOLS"), allSys: authz("SEC_ALL_SYS"),
                entView: authz("SEC_ENT_VIEW_AUTHZ"), entAll: authz("SEC_ENT_ALL_AUTHZ"),
                entNarrow: member("SEC_ENT_NARROW", "moqui.basic.Enumeration"), entCatch: member("SEC_ENT_CATCHALL", ".*"),
                apiView: authz("SEC_API_VIEW_AUTHZ"), apiAll: authz("SEC_API_ALL_AUTHZ"),
                tapSvcAuthz: authz("SEC_TAP_SVC_AUTHZ"), tapTransAuthz: authz("SEC_TAP_TRANS_AUTHZ"),
                tapSvc: member("SEC_TAP_SVC", TAP_SVC), tapTrans: member("SEC_TAP_TRANS", TAP_TRANS),
                tapSvcLimit: tarpit("SEC_TAP_SVC"), tapTransLimit: tarpit("SEC_TAP_TRANS"),
                lock, loginKey: one("moqui.security.UserLoginKey", { loginKey: LOGIN_KEY_HASH }), keyAccount,
                messageType: one("moqui.service.message.SystemMessageType", { systemMessageTypeId: "SEC_SMT_TEST" }),
                hmac: remote("SEC_SMR_HMAC"), hmacTs: remote("SEC_SMR_HMAC_TS"), noneRemote: remote("SEC_SMR_NONE"),
                schema: one("moqui.security.UserGroupPermission", { userGroupId: "ADMIN", userPermissionId: "REST_SCHEMA" }),
            }
        } finally {
            ec.artifactExecution.enableAuthz()
        }
    },
    then(row) {
        row.allTools.artifactGroupId == "TOOLS_APP"
        row.allTools.authzActionEnumId == "AUTHZA_ALL"
        row.allSys.artifactGroupId == "SYSTEM_APP"
        row.allSys.authzActionEnumId == "AUTHZA_ALL"
        row.entView.authzActionEnumId == "AUTHZA_VIEW"
        row.entNarrow.artifactTypeEnumId == "AT_ENTITY"
        row.entNarrow.nameIsPattern == "N"
        row.entAll.authzActionEnumId == "AUTHZA_ALL"
        row.entCatch.artifactTypeEnumId == "AT_ENTITY"
        row.entCatch.nameIsPattern == "Y"
        row.apiView.artifactGroupId == "MOQUI_API"
        row.apiView.authzActionEnumId == "AUTHZA_VIEW"
        row.apiAll.artifactGroupId == "MOQUI_API"
        row.apiAll.authzActionEnumId == "AUTHZA_ALL"
        row.tapSvcAuthz.authzActionEnumId == "AUTHZA_ALL"
        row.tapTransAuthz.authzActionEnumId == "AUTHZA_ALL"
        row.tapSvc.artifactTypeEnumId == "AT_SERVICE"
        row.tapSvc.nameIsPattern == "N"
        row.tapTrans.artifactTypeEnumId == "AT_XML_SCREEN_TRANS"
        row.tapTrans.nameIsPattern == "N"
        row.tapSvcLimit.maxHitsCount == 3
        row.tapSvcLimit.maxHitsDuration == 60
        row.tapSvcLimit.tarpitDuration == 30
        row.tapTransLimit.maxHitsCount == 3
        row.tapTransLimit.maxHitsDuration == 60
        row.tapTransLimit.tarpitDuration == 30
        row.lock.disabled != "Y"
        row.lock.successiveFailedLogins == 0
        row.lock.disabledDateTime == null
        row.loginKey != null
        row.loginKey.userId == row.keyAccount.userId
        row.loginKey.description == "security test known login key"
        row.messageType != null
        row.hmac.messageAuthEnumId == "SmatHmacSha256"
        row.hmac.authHeaderName == "X-Moqui-Signature"
        row.hmac.sharedSecret == "sec-hmac-test-secret"
        row.hmacTs.messageAuthEnumId == "SmatHmacSha256Timestamp"
        row.hmacTs.sharedSecret == "sec-hmac-test-secret"
        row.noneRemote.messageAuthEnumId == "SmatNone"
        row.schema != null
    },
})
