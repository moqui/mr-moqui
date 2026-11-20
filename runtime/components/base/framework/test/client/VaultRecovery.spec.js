/* This software is in the public domain under CC0 1.0 Universal plus a Grant of Patent License. */
// Organization recovery, split k of n, end to end over REST (/rest/s1/vault, plans/Vault-Instances.md V4,
// docs/security/Vaults.md) with the test client's `vault` member: each user's client is the vault crate's own session,
// as the browser's worker and `moqui vault recovery` are. Two holders make a recovery key split 2 of 2; a member turns
// recovery on for their vault, loses their key and makes a new one; one holder starts the recovery, the other seals
// their share to it, and the first combines the shares in their client and gives the member's new key the vault key;
// the member opens the vault with it. The server's rules (k - 1 cannot, VAULT_RECOVERY and a recent sign-in, a
// single-secret vault, an operational vault's members) are crates/moqui-vault-server's tests (src/tests/recovery.rs).
// Run: ./moq test-client framework --serve --spec VaultRecovery

const RUN = String(Date.now())
// test/ClientFixtures.spec.js makes them, each with a second factor and one single-use code: a strong, recent sign-in
const USERS = [
    ["r1", "sec.vault.r1", "SecVaultR1!!", "70318011"],
    ["r2", "sec.vault.r2", "SecVaultR2!!", "70318012"],
    ["m", "sec.vault.m", "SecVaultM1!!", "70318013"],
]
const BODY = { v: 1, type: "note", name: "Safe combination", fields: { text: "12-34-56" } }
let users = {}

const api = (user, method, path, body) => {
    const opts = method === "GET" ? { params: body || {}, accept: "json" } : { json: body || {}, accept: "json" }
    const res = user.s.request(method, "/rest/s1/vault" + path, opts)
    let value = {}
    try { value = JSON.parse(res.text) || {} } catch (e) { value = { text: res.text } }
    return Object.assign({ status: res.status }, value)
}
const refused = (answer, text) => (answer.status >= 400 || !!answer.errors) && JSON.stringify(answer).includes(text)
const ok = (answer, what) => {
    if (answer.status >= 400 || answer.errors) throw new Error(`${what}: ${JSON.stringify(answer).slice(0, 400)}`)
    return answer
}
const checks = (user) => api(user, "GET", "/keyChecks").keyCheckList
// `from` compares `to`'s active key's fingerprint, then stores the check its client signed
const check = (from, to) => {
    const key = api(from, "GET", "/publicKey", { userId: to.userId })
    const made = from.c.call("key_check_make", { checked_key_id: key.personalKeyId, checked_user_id: key.userId, checked_public: key.public,
        fingerprint: key.fingerprint, checked_at: Date.now() })
    ok(api(from, "POST", "/keyChecks", { check: made }), "store#KeyCheck")
}

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
            const keyId = `vrc-${name}-${RUN}`
            const made = c.call("personal_make", { personal_key_id: keyId, user_id: key.userId, passphrase: "a long pass phrase" })
            ok(api(user, "POST", "/personalKey", { personalKeyId: keyId, public: made.public, passphraseWrap: made.passphrase_wrap,
                recoveryWrap: made.recovery_wrap }), `setup#PersonalKey for ${username}`)
            users[name] = Object.assign(user, { c, userId: key.userId, keyId, public: made.public })
        }
    }
})

feature("two of two holders recover a member's vault to the member's new key, which opens it", {
    when() {
        const { r1, r2, m } = users
        // r1 makes the recovery key, split 2 of 2, after checking r2's key
        check(r1, r2)
        const recoveryKeyId = `vrc-rk-${RUN}`
        const made = r1.c.call("recovery_make", { recovery_key_id: recoveryKeyId, k: 2, key_checks: checks(r1),
            holders: [r1, r2].map(h => ({ user_id: h.userId, key_id: h.keyId, public: h.public })) })
        ok(api(r1, "POST", "/recoveryKeys", { record: made.record, shares: made.shares }), "create#VaultRecoveryKey")

        // m makes a vault with an item, and turns recovery on after checking the recovery key's maker
        const vaultId = `vrc-vault-${RUN}`
        const itemId = `vrc-item-${RUN}`
        const me = { user_id: m.userId, role: "manage", key_id: m.keyId, public: m.public }
        const entry = m.c.call("key_version_first", { vault_id: vaultId, members: [me] })
        m.c.call("vault_key_new", { vault_id: vaultId, key_version: 1 })
        const wrap = m.c.call("vault_key_wrap_member", { vault_id: vaultId, key_version: 1, member_user_id: m.userId,
            member_key_id: m.keyId, member_public: m.public })
        ok(api(m, "POST", "/vaults", { vaultId, vaultKindEnumId: "VkPersonal", lockEnumId: "VlSharedKey", entry, memberWraps: [wrap] }), "create#Vault")
        const sealed = m.c.call("item_seal", { vault_id: vaultId, item_id: itemId, key_version: 1, body: JSON.stringify(BODY) })
        ok(api(m, "POST", `/vaults/${vaultId}/items`, { vaultItemId: itemId, keyVersion: 1, wrappedItemKey: sealed.wrapped_item_key,
            bodyCipher: sealed.sealed_body }), "create#VaultItem")
        check(m, r1)
        const key = api(m, "GET", "/recoveryKeys").recoveryKeyList.find(k => k.recoveryKeyId === recoveryKeyId)
        const recoveryKeys = [{ record: key.record, maker_public: key.makerPublic }]
        const member = m.c.call("recovery_member", { record: key.record, maker_public: key.makerPublic, key_checks: checks(m) })
        const vault0 = api(m, "GET", `/vaults/${vaultId}`)
        const entry2 = m.c.call("key_version_next", { entries: vault0.entries, members: [me, member], version: 1, revision: 2,
            key_checks: checks(m), recovery_keys: recoveryKeys })
        const recoveryWrap = m.c.call("vault_key_wrap_member", { vault_id: vaultId, key_version: 1, member_user_id: member.user_id,
            member_key_id: member.key_id, member_public: member.public, key_checks: checks(m), recovery_keys: recoveryKeys })
        ok(api(m, "POST", `/vaults/${vaultId}/members`, { expectedRevision: vault0.vault.revision, entry: entry2, wrap: recoveryWrap }),
            "add#VaultMember (recovery on)")
        const recoveryOn = api(m, "GET", `/vaults/${vaultId}`).vault.recoveryOn

        // r1 knows m's key (a signer of the vault's chain); m loses it and makes a new one, which r1 checks again
        check(r1, m)
        const m2 = vault.client()
        const newKeyId = `vrc-m2-${RUN}`
        const remade = m2.call("personal_make", { personal_key_id: newKeyId, user_id: m.userId, passphrase: "another pass phrase" })
        ok(api(m, "POST", "/personalKey/replace", { personalKeyId: newKeyId, public: remade.public,
            passphraseWrap: remade.passphrase_wrap, recoveryWrap: remade.recovery_wrap }), "replace#PersonalKey")
        check(r1, m)

        // r1 starts the recovery; completing it before the other holder's share is refused
        const held = (user) => api(user, "GET", "/recoveryKeys").recoveryKeyList.find(k => k.recoveryKeyId === recoveryKeyId)
            .holders.find(h => h.userId === user.userId).held
        const sessionId = `vrc-rs-${RUN}`
        const session = r1.c.call("recovery_session_start", { session_id: sessionId, record: key.record, maker_public: key.makerPublic,
            held: held(r1), vault_id: vaultId, subject_user_id: m.userId, subject_key_id: newKeyId, subject_public: remade.public,
            key_checks: checks(r1) })
        ok(api(r1, "POST", "/recoveries", { session }), "start#VaultRecovery")
        const early = api(r1, "POST", `/recoveries/${sessionId}/complete`, { expectedRevision: 1, entry: { x: 1 }, wrap: { x: 1 } })

        // r2 sees what the session recovers, checks r1's key and m's new key, and seals its share to the session's
        // ephemeral key (the session's starter checked as a holder by their holder row)
        check(r2, r1)
        check(r2, m)
        const row = api(r2, "GET", "/recoveries").sessionList.find(s => s.recoverySessionId === sessionId)
        const share = r2.c.call("recovery_share_deliver", { record: key.record, maker_public: key.makerPublic, held: held(r2),
            session: row.session, session_holder_public: row.holderPublic, session_holder: row.holderShare, key_checks: checks(r2) })
        ok(api(r2, "POST", `/recoveries/${sessionId}/shares`, { share }), "deliver#VaultRecoveryShare")

        // r1 combines the shares in its client, opens the vault key through the recovery key's wrap, and rekeys m
        const open = api(r1, "GET", `/recoveries/${sessionId}`)
        r1.c.call("recovery_combine", { session_id: sessionId, record: open.record, maker_public: open.makerPublic, held: open.held,
            deliveries: open.deliveries, key_checks: checks(r1) })
        const r1Checks = checks(r1)
        r1.c.call("recovery_open_vault", { entries: open.entries, key_checks: r1Checks, session_id: sessionId, wrap: open.recoveryWrap })
        const rekeyed = r1.c.call("recovery_rekey", { session_id: sessionId, entries: open.entries, key_checks: r1Checks })
        const completed = api(r1, "POST", `/recoveries/${sessionId}/complete`, { expectedRevision: open.revision,
            entry: rekeyed.entry, wrap: rekeyed.wrap, rewraps: rekeyed.rewraps })

        // m's new client checks the recovery key's maker, opens the vault through the chain and reads the item
        const mVault = api(m, "GET", `/vaults/${vaultId}`)
        const mWrap = mVault.wraps.find(w => w.member_key_id === newKeyId)
        const m2Check = (() => {
            const r1Key = api(m, "GET", "/publicKey", { userId: r1.userId })
            const made = m2.call("key_check_make", { checked_key_id: r1Key.personalKeyId, checked_user_id: r1.userId,
                checked_public: r1Key.public, fingerprint: r1Key.fingerprint, checked_at: Date.now() })
            ok(api(m, "POST", "/keyChecks", { check: made }), "store#KeyCheck by the new key")
            return checks(m)
        })()
        const opened = m2.try("vault_key_open", { entries: mVault.entries, key_checks: m2Check, recovery_keys: recoveryKeys, wrap: mWrap })
        const item = api(m, "GET", `/vaults/${vaultId}/items`).itemList.find(i => i.vaultItemId === itemId)
        const body = m2.call("item_open", { vault_id: vaultId, item_id: itemId, key_version: 1, wrapped_item_key: item.wrappedItemKey,
            sealed_body: item.bodyCipher }).body
        const events = api(m, "GET", `/vaults/${vaultId}/events`).eventList.map(e => e.eventTypeEnumId)
        const used = api(m, "GET", "/recoveryKeys").recoveryKeyList.find(k => k.recoveryKeyId === recoveryKeyId).used
        return { recoveryOn, early, completed, opened, body: JSON.parse(body), events, used }
    },
    then({ recoveryOn, early, completed, opened, body, events, used }) {
        recoveryOn == "Y"
        refused(early, "other holders' shares")
        completed.revision > 0
        opened.ok == true
        body.fields.text == "12-34-56"
        events.includes("VeRecoveryStarted")
        events.includes("VeRecoveryShared")
        events.includes("VeRecovery")
        used == true
    }
})
