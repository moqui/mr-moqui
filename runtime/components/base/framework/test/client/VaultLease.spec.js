/* This software is in the public domain under CC0 1.0 Universal plus a Grant of Patent License. */
// A lease of an operational vault approved end to end over REST (/rest/s1/vault, plans/archive/Vault-Instances.md V2,
// docs/security/Vaults.md "Leases") with the test client's `vault` member, the vault crate's own session as the lease
// dialog's worker and `moqui vault approve` run it: a manager asks for a lease of one item, the approver checks the
// server's lease key once, opens the vault through its chain, approves in their client (each item key unwrapped under
// the lease's own names, sealed to the lease and signed), and the server stores it; the member that asked holds the
// lease's key. A manager without the approve role is refused, a lease changed after it was signed is refused by the
// client, and a revoke ends it. The server's own rules (one item only, fail closed, two members, machine access, the
// stored-call guard, a restart) are crates/moqui-vault-server's tests.
// Run: ./moq test-client framework --serve --spec VaultLease

const RUN = String(Date.now())
// test/ClientFixtures.spec.js makes them, each with a second factor and one single-use code, so their sign-ins are
// strong ones; sec.vault.d holds VAULT_ADMIN
const USERS = [
    ["d", "sec.vault.d", "SecVaultD1!!", "70318004"],
    ["e", "sec.vault.e", "SecVaultE1!!", "70318005"],
]
const BINDING = `AppInstance:lease-${RUN}:dbPassword`
const BODY = { v: 1, type: "login", name: "Demo database", fields: { username: "demo", password: "lease-spec-db-password" } }
let users = {}
let vaultId = ""
let leaseId = ""

const api = (user, method, path, body) => {
    const opts = method === "GET" ? { params: body || {}, accept: "json" } : { json: body || {}, accept: "json" }
    const res = user.s.request(method, "/rest/s1/vault" + path, opts)
    let value = {}
    try { value = JSON.parse(res.text) || {} } catch (e) { value = { text: res.text } }
    return Object.assign({ status: res.status }, value)
}
const refused = (answer, text) => (answer.status >= 400 || !!answer.errors) && JSON.stringify(answer).includes(text)
const checks = (user) => api(user, "GET", "/keyChecks").keyCheckList
const check = (from, to) => {
    const key = api(from, "GET", "/publicKey", { userId: to.userId })
    const made = from.c.call("key_check_make", { checked_key_id: key.personalKeyId, checked_user_id: key.userId, checked_public: key.public,
        fingerprint: key.fingerprint, checked_at: Date.now() })
    api(from, "POST", "/keyChecks", { check: made })
}
const member = (user, role) => ({ user_id: user.userId, role, key_id: user.keyId, public: user.public })
const flip = (hex) => (hex[0] === "0" ? "1" : "0") + hex.slice(1)

spec({
    timeout: 60,
    setupSpec() {
        needsFixtures("the client fixture users")
        for (const [name, username, password, code] of USERS) {
            const s = session()
            const res = s.post("/rest/login", { json: { username, password, code }, accept: "json", csrf: false })
            if (res.status >= 400 || !JSON.parse(res.text).loggedIn) throw new Error(`login of ${username}: ${res.status} ${res.text.slice(0, 200)}`)
            const user = { s }
            const key = api(user, "GET", "/personalKey")
            if (key.hasKey) skip("the fixture users have personal vault keys from an earlier run: --serve gives a fresh database")
            const c = vault.client()
            const keyId = `vls-${name}-${RUN}`
            const made = c.call("personal_make", { personal_key_id: keyId, user_id: key.userId, passphrase: "a long pass phrase" })
            const setup = api(user, "POST", "/personalKey", { personalKeyId: keyId, public: made.public,
                passphraseWrap: made.passphrase_wrap, recoveryWrap: made.recovery_wrap })
            if (setup.status >= 400 || setup.errors) throw new Error(`setup#PersonalKey for ${username}: ${JSON.stringify(setup)}`)
            users[name] = Object.assign(user, { c, userId: key.userId, keyId, public: made.public })
        }
        const { d, e } = users
        check(d, e)
        check(e, d)
        vaultId = `vls-vault-${RUN}`
        const members = [member(d, "manage"), member(e, "approve")]
        const entry = d.c.call("key_version_first", { vault_id: vaultId, members, key_checks: checks(d) })
        d.c.call("vault_key_new", { vault_id: vaultId, key_version: 1 })
        const memberWraps = members.map(m => d.c.call("vault_key_wrap_member", { vault_id: vaultId, key_version: 1,
            member_user_id: m.user_id, member_key_id: m.key_id, member_public: m.public, key_checks: checks(d) }))
        const created = api(d, "POST", "/vaults", { vaultId, vaultKindEnumId: "VkOperational", lockEnumId: "VlSharedKey",
            vaultName: "Lease spec", entry, memberWraps })
        if (created.status >= 400 || created.errors) throw new Error(`create#Vault: ${JSON.stringify(created)}`)
        const itemId = `vls-item-${RUN}`
        const label = { item_type: "login", binding: BINDING, name: "Demo database" }
        const sealed = d.c.call("item_seal", { vault_id: vaultId, item_id: itemId, key_version: 1, label, body: JSON.stringify(BODY) })
        const item = api(d, "POST", `/vaults/${vaultId}/items`, { vaultItemId: itemId, keyVersion: 1, wrappedItemKey: sealed.wrapped_item_key,
            bodyCipher: sealed.sealed_body, vaultItemTypeId: "login", bindingKey: BINDING, itemName: "Demo database" })
        if (item.status >= 400 || item.errors) throw new Error(`create#VaultItem: ${JSON.stringify(item)}`)
    }
})

feature("an approver checks the lease key, approves a lease in their client, and the member that asked holds it", {
    when() {
        const { d, e } = users
        const leaseKey = api(e, "GET", "/leaseKey")
        const keyCheck = e.c.call("key_check_make", { lease_key: leaseKey.leaseKey, fingerprint: leaseKey.fingerprint, checked_at: Date.now() })
        api(e, "POST", "/keyChecks", { check: keyCheck })
        const asked = api(d, "POST", "/leases", { vaultId, bindings: [BINDING], operation: "VaultLease spec run", limitMinutes: 10 })
        leaseId = asked.leaseId
        const vault = api(e, "GET", `/vaults/${vaultId}`)
        const wrap = vault.wraps.find(w => w.key_version === 1)
        e.c.call("vault_key_open", { entries: vault.entries, key_checks: checks(e), wrap })
        const got = api(e, "GET", `/leases/${leaseId}`)
        // the dialog shows what the signed lease names; a lease changed after it was signed is refused by the client
        const changed = Object.assign({}, got.lease, { public: flip(got.lease.public) })
        const tampered = e.c.try("lease_approve", { lease: changed, lease_key: got.leaseKey, key_checks: checks(e),
            wrapped_item_keys: got.wrappedItemKeys, now: Date.now() })
        const approval = e.c.call("lease_approve", { lease: got.lease, lease_key: got.leaseKey, key_checks: checks(e),
            wrapped_item_keys: got.wrappedItemKeys, now: Date.now() })
        // the manager has no approve role, even with the approval in hand
        const byManager = api(d, "POST", `/leases/${leaseId}/approve`, { approval })
        const approved = api(e, "POST", `/leases/${leaseId}/approve`, { approval })
        const after = api(d, "GET", `/leases/${leaseId}`)
        const waiting = api(e, "GET", "/leases").leaseList.filter(l => l.statusId === "VlsRequested").map(l => l.leaseId)
        const events = api(d, "GET", `/vaults/${vaultId}/events`).eventList.map(ev => ev.eventTypeEnumId)
        return { asked, got, tampered, byManager, approved, after, waiting, events }
    },
    then({ asked, got, tampered, byManager, approved, after, waiting, events }) {
        asked.lease.items.length == 1
        asked.lease.items[0].binding == BINDING
        got.statusId == "VlsRequested"
        got.lease.operation == "VaultLease spec run"
        !JSON.stringify(got).includes("lease-spec-db-password")
        tampered.ok == false
        tampered.error.includes("signature")
        refused(byManager, "approve role")
        approved.statusId == "VlsOpen"
        after.statusId == "VlsOpen"
        after.approvedByUserId == users.e.userId
        after.held == true
        !waiting.includes(leaseId)
        events.includes("VeLeaseRequested")
        events.includes("VeLeaseApproved")
    }
})

feature("a revoked lease ends, and a second approval of it is refused", {
    when() {
        const { d, e } = users
        const revoked = api(e, "POST", `/leases/${leaseId}/revoke`, {})
        const after = api(d, "GET", `/leases/${leaseId}`)
        const again = api(e, "POST", `/leases/${leaseId}/revoke`, {})
        return { revoked, after, again }
    },
    then({ revoked, after, again }) {
        revoked.statusId == "VlsRevoked"
        after.statusId == "VlsRevoked"
        after.held == false
        refused(again, "has ended")
    }
})
