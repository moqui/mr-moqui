// org.moqui.security.VaultServices' access as a server spec (plans/Vault-Instances.md V1, docs/security/Vaults.md;
// crates/moqui-vault-server, whose cargo tests hold the rules of the signed records): a vault, its items and an item's
// history are not found to a non-member, and a write by one is refused; a delivery is listed and filed only by its
// recipient; a wrapped personal key is sent only after a strong, recent sign-in (a password and a second factor), and an
// account with a password and no second factor is told to set one up. The client's side, with real keys, is
// test/client/VaultShared.spec.js. Run: ./moq test framework --spec VaultServices
const A = "VAULTSPEC_A"
const B = "VAULTSPEC_B"
const C = "VAULTSPEC_C"
const PASSWORD = "Vault-spec-9!"
const SERVICES = "org.moqui.security.VaultServices."
// a personal key's two public halves (crates/moqui-vault testdata/vectors.json); the server stores them as given
const PUBLIC = "8f63c2c1d2a100e62733fb8a2ec5eaee49f3f3bababb0b516420f603fa17a53d0bd139cd65f6d1c5088384c7efb564909f50ecc29f02250676690b4a69c7bc9d"
// the shape of a passphrase wrap at the Argon2id floor; the server keeps it and never opens it
const WRAP = {m_kib: 65536, t: 3, p: 1, salt: "00".repeat(16), sealed: "ab".repeat(92)}
const RUN = String(Date.now())

function sys(run) {
    const wasDisabled = ec.artifactExecution.disableAuthz()
    try {
        return run()
    } finally {
        if (!wasDisabled) ec.artifactExecution.enableAuthz()
    }
}
function username(userId) {
    return userId.toLowerCase().replace(/_/g, ".")
}
function as(userId, name, parameters) {
    if (userId) ec.user.internalLoginUser(username(userId))
    let out = {}
    let thrown = ""
    try {
        out = sys(() => ec.service.sync().name(name.includes(".") ? name : SERVICES + name).parameters(parameters || {}).call()) || {}
    } catch (err) {
        thrown = String(err.message || err)
    }
    const errors = thrown + (ec.message.hasError() ? ec.message.errorsString : "")
    ec.message.clearErrors()
    return Object.assign({errors}, out)
}
function makeUser(userId) {
    sys(() => {
        if (ec.entity.find("moqui.security.UserAccount").condition("userId", userId).count() === 0) {
            const account = ec.entity.makeValue("moqui.security.UserAccount")
            account.setFields({userId, username: username(userId), userFullName: "Vault " + userId.slice(-1), disabled: "N"}, true, null, null)
            account.create()
        }
        const account = ec.entity.find("moqui.security.UserAccount").condition("userId", userId).one()
        account.setFields({currentPassword: ec.user.hashPassword(PASSWORD), passwordHashType: "argon2", passwordSalt: "", passwordBase64: "N",
            successiveFailedLogins: 0, disabled: "N", disabledDateTime: null, requirePasswordChange: "N"}, true, null, null)
        account.update()
        ec.entity.find("moqui.security.UserAuthcFactor").condition("userId", userId).deleteAll()
        ec.entity.find("moqui.security.vault.VaultPersonalKey").condition("userId", userId).deleteAll()
        ec.entity.find("moqui.security.vault.VaultDelivery").condition("recipientUserId", userId).deleteAll()
    })
}
const eventsOf = (deliveryId) =>
    sys(() => ec.entity.find("moqui.security.vault.VaultEvent").condition("deliveryId", deliveryId).list()).map(event => event.eventTypeEnumId)

spec({
    setupSpec() {
        for (const u of [A, B, C]) makeUser(u)
    }
})

feature("a vault, its items and an item's history are not found to a non-member, and a write by one is refused", {
    when() {
        const vaultId = "vaultspec-ss-" + RUN
        const vaultItemId = "vaultspec-it-" + RUN
        const made = as(A, "create#Vault", {vaultId, vaultKindEnumId: "VkPersonal", lockEnumId: "VlSingleSecret",
            secretKindEnumId: "VsPassword", secretWrap: {sealed: "cd".repeat(60)}})
        const item = as(null, "create#VaultItem", {vaultId, vaultItemId, keyVersion: 1, wrappedItemKey: "aa".repeat(60), bodyCipher: "bb".repeat(300)})
        const own = as(null, "get#Vault", {vaultId})
        const ownItems = as(null, "list#VaultItems", {vaultId})
        const vault = as(B, "get#Vault", {vaultId})
        const items = as(null, "list#VaultItems", {vaultId})
        const history = as(null, "list#VaultItemHistory", {vaultItemId})
        const write = as(null, "update#VaultItem", {vaultItemId, expectedRevision: 1, keyVersion: 1, wrappedItemKey: "aa".repeat(60), bodyCipher: "ee".repeat(300)})
        const gone = as(null, "delete#Vault", {vaultId})
        const listed = as(null, "list#Vaults", {})
        return {made, item, own, ownItems, vault, items, history, write, gone, listed, vaultId}
    },
    then({made, item, own, ownItems, vault, items, history, write, gone, listed, vaultId}) {
        made.errors == ""
        item.errors == ""
        own.vault.vaultId == vaultId
        own.secretWrap.sealed == "cd".repeat(60)
        ownItems.itemList.length == 1
        ownItems.itemList[0].bodyCipher == "bb".repeat(300)
        vault.errors.includes("No vault")
        vault.secretWrap == null
        items.errors.includes("No vault")
        items.itemList == null
        history.errors.includes("No item")
        write.errors.includes("No item")
        gone.errors.includes("No vault")
        !JSON.stringify(listed.vaultList || []).includes(vaultId)
    }
})

feature("a delivery is listed and filed only by its recipient", {
    when() {
        const deliveryId = "vaultspec-dl-" + RUN
        sys(() => {
            const row = ec.entity.makeValue("moqui.security.vault.VaultDelivery")
            row.setFields({deliveryId, recipientUserId: B, recipientKeyId: "vaultspec-pk-b", purpose: "item", senderKind: "user",
                senderId: A, sealedValue: "ab".repeat(80), signature: "cd".repeat(64), sentDate: ec.user.nowTimestamp}, true, null, null)
            row.create()
        })
        const other = as(C, "list#IncomingDeliveries", {})
        const otherFile = as(null, "file#VaultDelivery", {deliveryId})
        const mine = as(B, "list#IncomingDeliveries", {})
        const filed = as(null, "file#VaultDelivery", {deliveryId})
        const after = as(null, "list#IncomingDeliveries", {})
        const all = as(null, "list#IncomingDeliveries", {includeFiled: true})
        return {other, otherFile, mine, filed, after, all, events: eventsOf(deliveryId), deliveryId}
    },
    then({other, otherFile, mine, filed, after, all, events, deliveryId}) {
        other.deliveryList.length == 0
        otherFile.errors.includes("No delivery")
        mine.deliveryList.length == 1
        mine.deliveryList[0].delivery.delivery_id == deliveryId
        filed.errors == ""
        after.deliveryList.length == 0
        all.deliveryList.length == 1
        events.length == 1
        events[0] == "VeDeliveryFiled"
    }
})

feature("a wrapped personal key is sent only after a strong, recent sign-in: a password and a second factor", {
    when() {
        const personalKeyId = "vaultspec-pk-" + RUN
        const setup = as(A, "setup#PersonalKey", {personalKeyId, public: PUBLIC, passphraseWrap: WRAP, recoveryWrap: "ef".repeat(100)})
        const shown = as(null, "get#PersonalKey", {})
        const internal = as(null, "get#WrappedPersonalKey", {})
        const password = as(null, "org.moqui.sso.RecentAuthServices.reauthenticate#User", {password: PASSWORD})
        const passwordOnly = as(null, "get#WrappedPersonalKey", {})
        const codes = sys(() => ec.service.sync().name("org.moqui.impl.UserServices.create#SingleUseAuthcCodes")
            .parameters({userId: A, numberOfCodes: 2}).call()).singleUseCodes
        ec.user.logoutUser()
        as(A, "get#PersonalKey", {})
        const first = as(null, "org.moqui.sso.RecentAuthServices.reauthenticate#User", {password: PASSWORD})
        const code = as(null, "org.moqui.sso.RecentAuthServices.reauthenticate#User", {code: String(codes[0])})
        const strong = as(null, "get#WrappedPersonalKey", {})
        const allowed = as(null, "get#PersonalKey", {})
        sys(() => ec.entity.find("moqui.security.UserAuthcFactor").condition("userId", A).deleteAll())
        return {setup, shown, internal, password, passwordOnly, first, code, strong, allowed, personalKeyId}
    },
    then({setup, shown, internal, password, passwordOnly, first, code, strong, allowed, personalKeyId}) {
        setup.errors == ""
        setup.personalKeyId == personalKeyId
        shown.public == PUBLIC
        shown.passphraseWrap == null
        shown.fetchAllowed == false
        internal.errors.includes("set up a second factor")
        internal.passphraseWrap == null
        password.reauthenticated == true
        passwordOnly.errors.includes("set up a second factor")
        first.codeNeeded == true
        code.reauthenticated == true
        strong.errors == ""
        strong.passphraseWrap.m_kib == 65536
        strong.recoveryWrap == "ef".repeat(100)
        allowed.fetchAllowed == true
    }
})
