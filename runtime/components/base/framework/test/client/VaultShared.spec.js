/* This software is in the public domain under CC0 1.0 Universal plus a Grant of Patent License. */
// Shared-key vaults over REST (/rest/s1/vault, plans/Vault-Instances.md V1, docs/security/Vaults.md) with the test
// client's `vault` member: each user's client is the vault crate's own session, as the browser's worker and `moqui
// vault` are. What the server serves of a personal vault holds no item's name, type or value in the clear; a client
// refuses a key version or a wrap no manager signed, a delivery whose signature is wrong, and a wrap to a changed key
// it has not checked again; a removed member gets no wrap and a write at the old key version is refused; each change
// writes its event. The server's own rules are crates/moqui-vault-server's tests and test/VaultServices.spec.js.
// Run: ./moq test-client framework --serve --spec VaultShared

const RUN = String(Date.now())
const USERS = [["a", "sec.all.only", "SecAll1!!"], ["b", "sec.view.only", "SecView1!!"], ["c", "sec.none.only", "SecNone1!!"]]
const BODY = { v: 1, type: "login", name: "Bank of Example", fields: { username: "ada-at-bank", password: "hunter2-very-secret" } }
let users = {}
let vaultId = ""
let itemId = ""

// A vault service over REST: its answer, with the status.
const api = (user, method, path, body) => {
    const opts = method === "GET" ? { params: body || {}, accept: "json" } : { json: body || {}, accept: "json" }
    const res = user.s.request(method, "/rest/s1/vault" + path, opts)
    let value = {}
    try { value = JSON.parse(res.text) || {} } catch (e) { value = { text: res.text } }
    return Object.assign({ status: res.status }, value)
}
const refused = (answer, text) => (answer.status >= 400 || !!answer.errors) && JSON.stringify(answer).includes(text)
const checks = (user) => api(user, "GET", "/keyChecks").keyCheckList
// `from` compares `to`'s fingerprint, then stores the check its client signed
const check = (from, to) => {
    const key = api(from, "GET", "/publicKey", { userId: to.userId })
    const made = from.c.call("key_check_make", { checked_key_id: key.personalKeyId, checked_public: key.public,
        fingerprint: key.fingerprint, checked_at: Date.now() })
    api(from, "POST", "/keyChecks", { check: made })
}
const member = (user, role) => ({ user_id: user.userId, role, key_id: user.keyId, public: user.public })
const flip = (hex) => (hex[0] === "0" ? "1" : "0") + hex.slice(1)

spec({
    timeout: 60,
    setupSpec() {
        needsFixtures("the client fixture users")
        for (const [name, username, password] of USERS) {
            const s = session({ user: username, password })
            const user = { s }
            const key = api(user, "GET", "/personalKey")
            if (key.hasKey) skip("the fixture users have personal vault keys from an earlier run: --serve gives a fresh database")
            const c = vault.client()
            const keyId = `vsh-${name}-${RUN}`
            const made = c.call("personal_make", { personal_key_id: keyId, user_id: key.userId, passphrase: "a long pass phrase" })
            const setup = api(user, "POST", "/personalKey", { personalKeyId: keyId, public: made.public,
                passphraseWrap: made.passphrase_wrap, recoveryWrap: made.recovery_wrap })
            if (setup.status >= 400 || setup.errors) throw new Error(`setup#PersonalKey for ${username}: ${JSON.stringify(setup)}`)
            users[name] = Object.assign(user, { c, userId: key.userId, keyId, public: made.public })
        }
    }
})

feature("what the server serves of a personal vault holds no item's name, type or value, and a member opens it through the chain", {
    when() {
        const { a, b, c } = users
        check(a, b)
        check(a, c)
        check(b, a)
        check(c, a)
        vaultId = `vsh-vault-${RUN}`
        itemId = `vsh-item-${RUN}`
        const members = [member(a, "manage"), member(b, "write"), member(c, "read")]
        const entry = a.c.call("key_version_first", { vault_id: vaultId, members, key_checks: checks(a) })
        a.c.call("vault_key_new", { vault_id: vaultId, key_version: 1 })
        const memberWraps = members.map(m => a.c.call("vault_key_wrap_member", { vault_id: vaultId, key_version: 1,
            member_user_id: m.user_id, member_key_id: m.key_id, member_public: m.public, key_checks: checks(a) }))
        const created = api(a, "POST", "/vaults", { vaultId, vaultKindEnumId: "VkPersonal", lockEnumId: "VlSharedKey", entry, memberWraps })
        const sealed = a.c.call("item_seal", { vault_id: vaultId, item_id: itemId, key_version: 1, body: JSON.stringify(BODY) })
        const item = api(a, "POST", `/vaults/${vaultId}/items`, { vaultItemId: itemId, keyVersion: 1,
            wrappedItemKey: sealed.wrapped_item_key, bodyCipher: sealed.sealed_body })
        const served = [api(a, "GET", `/vaults/${vaultId}`), api(a, "GET", `/vaults/${vaultId}/items`), api(b, "GET", "/vaults")]
            .map(answer => JSON.stringify(answer)).join("\n")
        const vault = api(b, "GET", `/vaults/${vaultId}`)
        const wrap = vault.wraps.find(w => w.key_version === 1)
        const opened = b.c.try("vault_key_open", { entries: vault.entries, key_checks: checks(b), wrap })
        const items = api(b, "GET", `/vaults/${vaultId}/items`)
        const row = items.itemList[0]
        const body = b.c.call("item_open", { vault_id: vaultId, item_id: itemId, key_version: 1,
            wrapped_item_key: row.wrappedItemKey, sealed_body: row.bodyCipher }).body
        return { created, item, served, opened, body: JSON.parse(body) }
    },
    then({ created, item, served, opened, body }) {
        created.vaultId == vaultId
        item.revision == 1
        !served.includes("Bank of Example")
        !served.includes("hunter2")
        !served.includes("ada-at-bank")
        !served.includes("\"login\"")
        opened.ok == true
        body.fields.password == "hunter2-very-secret"
    }
})

feature("a client refuses a key version or a wrap no manager signed, and a delivery whose signature is wrong", {
    when() {
        const { a, b, c } = users
        const vault = api(c, "GET", `/vaults/${vaultId}`)
        const members = [member(a, "manage"), member(b, "manage"), member(c, "read")]
        const writerSigns = b.c.try("key_version_next", { entries: vault.entries, key_checks: checks(b), members, version: 1, revision: 2 })
        const wrap = vault.wraps.find(w => w.key_version === 1)
        const forgedWrap = Object.assign({}, wrap, { signer_user_id: b.userId, signer_key_id: b.keyId })
        const byWriter = c.c.try("vault_key_open", { entries: vault.entries, key_checks: checks(c), wrap: forgedWrap })
        const entries = vault.entries.map(entry => Object.assign({}, entry))
        entries[0].signature = flip(entries[0].signature)
        const badChain = c.c.try("vault_key_open", { entries, key_checks: checks(c), wrap })
        const goodChain = c.c.try("vault_key_open", { entries: vault.entries, key_checks: checks(c), wrap })

        const deliveryId = `vsh-dlv-${RUN}`
        const sent = a.c.call("delivery_send", { delivery_id: deliveryId, to_user_id: b.userId, to_key_id: b.keyId,
            to_public: b.public, purpose: "item", value: "wifi: correct horse", key_checks: checks(a) })
        const posted = api(a, "POST", "/deliveries", { delivery: sent })
        const others = api(c, "GET", "/deliveries")
        const incoming = api(b, "GET", "/deliveries").deliveryList.find(d => d.deliveryId === deliveryId)
        const tampered = Object.assign({}, incoming.delivery, { signature: flip(incoming.delivery.signature) })
        const badDelivery = b.c.try("delivery_open", { delivery: tampered, sender_public: incoming.senderPublic, key_checks: checks(b) })
        const goodDelivery = b.c.try("delivery_open", { delivery: incoming.delivery, sender_public: incoming.senderPublic, key_checks: checks(b) })
        const filed = api(b, "POST", `/deliveries/${deliveryId}/file`, {})
        const bEvents = api(b, "GET", "/events").eventList.map(e => e.eventTypeEnumId)
        return { writerSigns, byWriter, badChain, goodChain, posted, others, badDelivery, goodDelivery, filed, bEvents, deliveryId }
    },
    then({ writerSigns, byWriter, badChain, goodChain, posted, others, badDelivery, goodDelivery, filed, bEvents, deliveryId }) {
        writerSigns.ok == false
        writerSigns.error.includes("manager")
        byWriter.ok == false
        byWriter.error.includes("manage role")
        badChain.ok == false
        goodChain.ok == true
        posted.deliveryId == deliveryId
        others.deliveryList.length == 0
        badDelivery.ok == false
        badDelivery.error.includes("signature")
        goodDelivery.result.value == "wifi: correct horse"
        filed.deliveryId == deliveryId
        bEvents.includes("VeDeliveryFiled")
    }
})

feature("nothing is wrapped to a changed personal key before it is checked again", {
    when() {
        const { a, b, c } = users
        // another key under c's name, as a server that swapped it would hand out: a's check of c's key does not cover it
        const changed = vault.client().call("personal_make", { personal_key_id: c.keyId, user_id: c.userId, passphrase: "another pass phrase" })
        const vault0 = api(a, "GET", `/vaults/${vaultId}`)
        const members = [member(a, "manage"), member(b, "write"), { user_id: c.userId, role: "read", key_id: c.keyId, public: changed.public }]
        const sign = a.c.try("key_version_next", { entries: vault0.entries, key_checks: checks(a), members, version: 1, revision: 2 })
        const wrap = a.c.try("vault_key_wrap_member", { vault_id: vaultId, key_version: 1, member_user_id: c.userId,
            member_key_id: c.keyId, member_public: changed.public, key_checks: checks(a) })
        const deliver = a.c.try("delivery_send", { delivery_id: `vsh-dlv2-${RUN}`, to_user_id: c.userId, to_key_id: c.keyId,
            to_public: changed.public, purpose: "item", value: "x", key_checks: checks(a) })
        return { sign, wrap, deliver }
    },
    then({ sign, wrap, deliver }) {
        sign.ok == false
        sign.error.includes("has not been checked")
        wrap.ok == false
        wrap.error.includes("has not been checked")
        deliver.ok == false
    }
})

feature("a removed member gets no wrap, a write at the old key version is refused, and each change writes its event", {
    when() {
        const { a, b, c } = users
        const vault0 = api(a, "GET", `/vaults/${vaultId}`)
        const items = api(a, "GET", `/vaults/${vaultId}/items`).itemList
        const rotated = a.c.call("vault_rotate", { entries: vault0.entries, key_checks: checks(a),
            members: [member(a, "manage"), member(c, "read")],
            items: items.map(item => ({ item_id: item.vaultItemId, wrapped_item_key: item.wrappedItemKey })) })
        const stale = api(a, "POST", `/vaults/${vaultId}/removeMember`, { userId: b.userId, expectedRevision: vault0.vault.revision - 1,
            entry: rotated.entry, itemKeys: rotated.item_keys, memberWraps: rotated.member_wraps })
        const removed = api(a, "POST", `/vaults/${vaultId}/removeMember`, { userId: b.userId, expectedRevision: vault0.vault.revision,
            entry: rotated.entry, itemKeys: rotated.item_keys, memberWraps: rotated.member_wraps })
        const bVault = api(b, "GET", `/vaults/${vaultId}`)
        const bItems = api(b, "GET", `/vaults/${vaultId}/items`)
        const old = a.c.call("item_seal", { vault_id: vaultId, item_id: itemId, key_version: 1, body: "{}" })
        const oldWrite = api(a, "PUT", `/items/${itemId}`, { expectedRevision: 1, keyVersion: 1,
            wrappedItemKey: old.wrapped_item_key, bodyCipher: old.sealed_body })
        const current = api(a, "GET", `/vaults/${vaultId}/items`).itemList[0]
        const sealed = a.c.call("item_seal", { vault_id: vaultId, item_id: itemId, key_version: 2, body: JSON.stringify(BODY),
            wrapped_item_key: current.wrappedItemKey, wrapped_key_version: current.keyVersion })
        const newWrite = api(a, "PUT", `/items/${itemId}`, { expectedRevision: 1, keyVersion: 2,
            wrappedItemKey: sealed.wrapped_item_key, bodyCipher: sealed.sealed_body })
        const cVault = api(c, "GET", `/vaults/${vaultId}`)
        const events = api(a, "GET", `/vaults/${vaultId}/events`).eventList.map(e => e.eventTypeEnumId)
        return { stale, removed, bVault, bItems, oldWrite, newWrite, cWraps: cVault.wraps.map(w => w.key_version), events }
    },
    then({ stale, removed, bVault, bItems, oldWrite, newWrite, cWraps, events }) {
        refused(stale, "changed since")
        removed.keyVersion == 2
        refused(bVault, "No vault")
        bVault.wraps == null
        refused(bItems, "No vault")
        refused(oldWrite, "key changed")
        newWrite.revision == 2
        cWraps.length == 2
        events.includes("VeVaultCreated")
        events.includes("VeItemCreated")
        events.includes("VeMemberRemoved")
        events.includes("VeKeyRotated")
        events.includes("VeItemUpdated")
    }
})
