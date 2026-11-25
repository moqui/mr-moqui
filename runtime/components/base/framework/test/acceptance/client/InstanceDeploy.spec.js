// Deploys to managed instances on a real Podman host (plans/archive/Vault-Instances.md §4.6, §6 M3;
// docs/tools/Component-Deploy.md, "A deploy over the link"): an acceptance test, since it needs a manager whose MR
// Moqui instances, containers on a Podman host, dial its manager link, an image to deploy, and a public component
// repository with a passed run. The regression suite covers the rules with the fake engine and a fake link
// (crates/moqui-instance/src/tests/deploy.rs): the gate, the lease's items, one member at a time drained, and rollback
// refused across a schema change.
//
// On one Linux machine with rootless Podman:
//
//   systemctl --user enable --now podman.socket
//   podman/build.sh                                  # localhost/moqui:latest, the image MoquiLocal provisions
//   printf 'FROM localhost/moqui:latest\nRUN rm -rf /opt/moqui/components/addon/mr-home\n' | podman build -t localhost/moqui:deploytest -
//   printf 'FROM localhost/moqui:deploytest\nLABEL org.moqui.test=instance-deploy\n' | podman build -t localhost/moqui:next -
//   ./moq run test framework --acceptance --spec InstanceDeployFixtures --db existing     # its users and rows
//   moqui_conf__manage__link_url=https://host.containers.internal:8443 ./moq run           # the manager
//   ./moq run test-client framework --acceptance --spec InstanceDeploy --server https://localhost:8443 --insecure \
//     --param podmanSocket=/run/user/$UID/podman/podman.sock
//
// The fixture (test/acceptance/InstanceDeployFixtures.spec.js) makes the two members, deploy.maker and deploy.approver,
// with their second factors, the environment deploytest, the CI environment of mr-home that deploys to it with a passed
// run of its head, and the image MoquiNext. The manager listens on every address (a container reaches the host at
// host.containers.internal, not at its loopback), and its link URL is the one its instances dial. This spec then does
// what the members would: each makes or opens their personal key in their own client (the test client's vault), they
// check each other's keys and the approver the server's lease key, the maker makes the operational vault deploytest-vault
// with its deposit key, and names the Podman host. A provision deposits the credentials it makes; before a deploy that
// opens them the maker takes the deposits in (System › Vaults › Take in), and the approver approves the deploy's lease
// (the lease dialog), as an environment with no machine access needs. Without podmanSocket every feature skips, so the
// framework's acceptance run on a machine with no Podman passes over it.

// the screens' transitions run under /apps (a POST to /qapps answers with the app shell and runs nothing)
const instances = params.instancesPath ?? "/apps/system/Instances"
const vaults = params.vaultsPath ?? "/apps/system/Vaults"
const tag = params.tag ?? `dep${Date.now() % 100000}`
const ENVIRONMENT = "deploytest"
const VAULT = "deploytest-vault"
const PASSPHRASE = "the InstanceDeploy members' pass phrase"
// the fixture's users: username, password, and the secret of their authenticator (TOTP) factor
const USERS = {
    maker: ["deploy.maker", "DeployMaker1!!", "GEZDGNBVGY3TQOJQGEZDGNBVGY3TQOJQ"],
    approver: ["deploy.approver", "DeployApprover1!!", "MFRGGZDFMZTWQ2LKNNWG23TPOBYXE43U"],
}

// An authenticator app's code (RFC 6238: HMAC-SHA1, 6 digits, 30 seconds) for a base32 secret and a step. The test
// client has HMAC-SHA256 only, so SHA-1 is here.
function sha1(bytes) {
    const words = []
    const length = bytes.length
    const padded = bytes.concat([0x80])
    while (padded.length % 64 != 56) padded.push(0)
    for (let i = 7; i >= 0; i--) padded.push(i >= 4 ? 0 : ((length * 8) >>> (i * 8)) & 0xff)
    for (let i = 0; i < padded.length; i += 4) words.push((padded[i] << 24) | (padded[i + 1] << 16) | (padded[i + 2] << 8) | padded[i + 3])
    let [a0, b0, c0, d0, e0] = [0x67452301, 0xefcdab89, 0x98badcfe, 0x10325476, 0xc3d2e1f0]
    const rotl = (x, n) => (x << n) | (x >>> (32 - n))
    for (let block = 0; block < words.length; block += 16) {
        const w = words.slice(block, block + 16)
        for (let t = 16; t < 80; t++) w[t] = rotl(w[t - 3] ^ w[t - 8] ^ w[t - 14] ^ w[t - 16], 1)
        let [a, b, c, d, e] = [a0, b0, c0, d0, e0]
        for (let t = 0; t < 80; t++) {
            const [f, k] = t < 20 ? [(b & c) | (~b & d), 0x5a827999] : t < 40 ? [b ^ c ^ d, 0x6ed9eba1]
                : t < 60 ? [(b & c) | (b & d) | (c & d), 0x8f1bbcdc] : [b ^ c ^ d, 0xca62c1d6]
            const next = (rotl(a, 5) + f + e + k + w[t]) | 0
            e = d; d = c; c = rotl(b, 30); b = a; a = next
        }
        a0 = (a0 + a) | 0; b0 = (b0 + b) | 0; c0 = (c0 + c) | 0; d0 = (d0 + d) | 0; e0 = (e0 + e) | 0
    }
    return [a0, b0, c0, d0, e0].flatMap((word) => [24, 16, 8, 0].map((shift) => (word >>> shift) & 0xff))
}
function totp(secret, step) {
    const alphabet = "ABCDEFGHIJKLMNOPQRSTUVWXYZ234567"
    let bits = ""
    for (const ch of secret.replace(/=+$/, "")) bits += alphabet.indexOf(ch).toString(2).padStart(5, "0")
    const key = []
    for (let i = 0; i + 8 <= bits.length; i += 8) key.push(parseInt(bits.slice(i, i + 8), 2))
    const counter = [0, 0, 0, 0, 0, 0, 0, 0]
    for (let i = 7, n = step; i >= 0; i--, n = Math.floor(n / 256)) counter[i] = n % 256
    const block = key.concat(new Array(64 - key.length).fill(0))
    const inner = sha1(block.map((byte) => byte ^ 0x36).concat(counter))
    const mac = sha1(block.map((byte) => byte ^ 0x5c).concat(inner))
    const offset = mac[19] & 0x0f
    const value = ((mac[offset] & 0x7f) << 24) | (mac[offset + 1] << 16) | (mac[offset + 2] << 8) | mac[offset + 3]
    return String(value % 1000000).padStart(6, "0")
}
const users = {}
const state = {}

// a REST call of the vault API as a member; its answer with its status
const api = (user, method, path, body) => {
    const opts = method === "GET" ? { params: body || {}, accept: "json" } : { json: body || {}, accept: "json" }
    const res = user.s.request(method, "/rest/s1/vault" + path, opts)
    let value = {}
    try { value = JSON.parse(res.text) || {} } catch (e) { value = { text: res.text } }
    return Object.assign({ status: res.status }, value)
}
const ok = (answer, what) => {
    if (answer.status >= 400 || answer.errors) throw new Error(`${what}: ${JSON.stringify(answer).slice(0, 400)}`)
    return answer
}
// a screen transition as a member: its JSON answer (the transition's messages and errors), and an error when it has any
const post = (user, path, form) => {
    const res = user.s.post(path, { form, accept: "json" })
    let body = {}
    try { body = JSON.parse(res.text) || {} } catch (e) { body = {} }
    const errors = body.errors || body.errorMessages || (Array.isArray(body.messageErrors) ? body.messageErrors.join("; ") : "")
    if (res.status >= 400 || (errors && String(errors).length)) throw new Error(`${path}: ${res.status} ${JSON.stringify(errors || res.text.slice(0, 300))}`)
    return res
}
const checks = (user) => api(user, "GET", "/keyChecks").keyCheckList || []
const member = (user, role) => ({ user_id: user.userId, role, key_id: user.keyId, public: user.public })

// a member signed in with their password and code, their personal key made or opened in their own client
function signIn(name) {
    const [username, password, secret] = USERS[name]
    // the code of the next 30-second step (the server takes one step either side of now); a step already taken (a run
    // moments ago) is refused, so then the next step's code once its time comes
    let s = null
    let res = null
    for (let attempt = 0; attempt < 3; attempt++) {
        if (attempt > 0) sleep(30000 - (Date.now() % 30000) + 500)
        s = session()
        const code = totp(secret, Math.floor(Date.now() / 30000) + 1)
        res = s.post("/rest/login", { json: { username, password, code }, accept: "json", csrf: false })
        if (res.status < 400 && JSON.parse(res.text).loggedIn) break
        s = null
    }
    if (!s) throw new Error(`login of ${username}: ${res.status} ${res.text.slice(0, 200)}`)
    const user = { s, c: vault.client() }
    const key = api(user, "GET", "/personalKey")
    if (key.hasKey) {
        const wrapped = ok(api(user, "GET", "/personalKey/wrapped"), `the wrapped personal key of ${username}`)
        user.c.call("personal_open", { personal_key_id: wrapped.personalKeyId, user_id: wrapped.userId, public: wrapped.public,
            passphrase_wrap: wrapped.passphraseWrap, passphrase: PASSPHRASE })
        Object.assign(user, { userId: wrapped.userId, keyId: wrapped.personalKeyId, public: wrapped.public })
    } else {
        const keyId = `idp-${name}-${tag}`
        const made = user.c.call("personal_make", { personal_key_id: keyId, user_id: key.userId, passphrase: PASSPHRASE })
        ok(api(user, "POST", "/personalKey", { personalKeyId: keyId, public: made.public, passphraseWrap: made.passphrase_wrap,
            recoveryWrap: made.recovery_wrap }), `setup#PersonalKey for ${username}`)
        Object.assign(user, { userId: key.userId, keyId, public: made.public })
    }
    return user
}

// `from` checks `to`'s personal key, once
function check(from, to) {
    if (checks(from).some((made) => JSON.stringify(made).includes(to.public))) return
    const key = api(from, "GET", "/publicKey", { userId: to.userId })
    const made = from.c.call("key_check_make", { checked_key_id: key.personalKeyId, checked_user_id: key.userId, checked_public: key.public,
        fingerprint: key.fingerprint, checked_at: Date.now() })
    ok(api(from, "POST", "/keyChecks", { check: made }), "store#KeyCheck")
}

// the member's client opens the vault's current key (once per client)
function openVault(user) {
    const got = ok(api(user, "GET", `/vaults/${VAULT}`), `get#Vault ${VAULT}`)
    const version = Number(got.vault.keyVersion)
    if (user.opened === version) return got
    const wrap = (got.wraps || []).find((one) => Number(one.key_version) === version)
    user.c.call("vault_key_open", { entries: got.entries, key_checks: checks(user), wrap })
    user.opened = version
    return got
}

// the maker takes in what the server deposited (System › Vaults › Take in), so a lease can name it
function takeIn() {
    const maker = users.maker
    const got = openVault(maker)
    const version = Number(got.vault.keyVersion)
    const items = (api(maker, "GET", `/vaults/${VAULT}/items`).itemList || []).filter((item) => item.itemKeyWrapEnumId === "VwDeposit")
    for (const item of items) {
        const adopted = maker.c.call("deposit_adopt", { vault_id: VAULT, item_id: item.vaultItemId, key_version: version,
            label: { item_type: item.vaultItemTypeId, binding: item.bindingKey, name: item.itemName },
            deposit_wrapped_private: got.depositWrappedPrivate, wrapped_item_key: item.wrappedItemKey })
        const res = post(maker, `${vaults}/TakeIn/take`, { vaultItemId: item.vaultItemId, expectedRevision: item.revision, keyVersion: version,
            wrappedItemKey: adopted.wrapped_item_key })
        if (res.status >= 400 || (res.text || "").includes("\"errors\"")) throw new Error(`take#VaultDeposit ${item.bindingKey}: ${res.text}`)
    }
    return items.length
}

// the approver approves a lease in their client, as the lease dialog does; an approval needs a recent sign-in, so when
// theirs is not (reauthRequired) they sign in again, as the dialog asks, and approve once more
function approve(leaseId) {
    for (let attempt = 0; ; attempt++) {
        const approver = users.approver
        openVault(approver)
        const got = ok(api(approver, "GET", `/leases/${leaseId}`), `get#VaultLease ${leaseId}`)
        const approval = approver.c.call("lease_approve", { lease: got.lease, lease_key: got.leaseKey, key_checks: checks(approver),
            wrapped_item_keys: got.wrappedItemKeys, now: Date.now() })
        const answer = api(approver, "POST", `/leases/${leaseId}/approve`, { approval })
        if (attempt == 0 && JSON.stringify(answer).includes("reauthRequired")) {
            users.approver = signIn("approver")
            continue
        }
        ok(answer, `approve#VaultLease ${leaseId}`)
        return
    }
}

// rows as the entity REST answers them to the maker, an INSTANCE_ADMIN in ADMIN (the fixture): ADMIN alone reaches the
// entity REST but not these rows
const rows = (path, query) => users.maker.s.get(`/rest/e1/${path}`, { accept: "json", params: query || {} }).json()
// an instance's row
const instance = (id) => rows(`moqui.server.instance.AppInstance/${id}`)
// its actions, newest first
const actions = (id) =>
    users.maker.s
        .get("/rest/e1/moqui.server.instance.InstanceAction", {
            accept: "json",
            params: { appInstanceId: id, orderByField: "-startDate", pageSize: 20 },
        })
        .json()

// wait for an action of `id` that `pick` takes to end, approving its lease while it waits for one
function finished(id, pick, timeout) {
    const approved = new Set()
    return waitUntil(
        () => {
            const row = actions(id).find(pick)
            if (!row) return null
            if (row.statusEnumId == "IasWaiting" && row.leaseId && !approved.has(row.leaseId)) {
                approved.add(row.leaseId)
                approve(row.leaseId)
                return null
            }
            return row.statusEnumId == "IasRunning" || row.statusEnumId == "IasWaiting" ? null : row
        },
        { timeout, interval: 5000 },
    )
}

spec({
    timeout: 3600,
    setupSpec() {
        if (!params.podmanSocket) skip("no Podman host: --param podmanSocket=/run/user/$UID/podman/podman.sock (see the spec's header)")
        users.maker = signIn("maker")
        users.approver = signIn("approver")
        const { maker, approver } = users
        check(maker, approver)
        check(approver, maker)
        const leaseKey = api(approver, "GET", "/leaseKey")
        const keyCheck = approver.c.call("key_check_make", { lease_key: leaseKey.leaseKey, fingerprint: leaseKey.fingerprint, checked_at: Date.now() })
        api(approver, "POST", "/keyChecks", { check: keyCheck })
        // the operational vault the fixture's environment names, made once by the maker
        if (api(maker, "GET", `/vaults/${VAULT}`).status >= 400) {
            const members = [member(maker, "manage"), member(approver, "approve")]
            const entry = maker.c.call("key_version_first", { vault_id: VAULT, members, key_checks: checks(maker) })
            maker.c.call("vault_key_new", { vault_id: VAULT, key_version: 1 })
            const memberWraps = members.map((m) => maker.c.call("vault_key_wrap_member", { vault_id: VAULT, key_version: 1,
                member_user_id: m.user_id, member_key_id: m.key_id, member_public: m.public, key_checks: checks(maker) }))
            ok(api(maker, "POST", "/vaults", { vaultId: VAULT, vaultKindEnumId: "VkOperational", lockEnumId: "VlSharedKey",
                vaultName: "InstanceDeploy", entry, memberWraps }), "create#Vault")
            maker.opened = 1
            const deposit = maker.c.call("deposit_key_new", { vault_id: VAULT, key_version: 1 })
            const revision = api(maker, "GET", `/vaults/${VAULT}`).vault.revision
            ok(api(maker, "POST", `/vaults/${VAULT}/depositKey`, { expectedRevision: revision, deposit: deposit.deposit,
                depositWrappedPrivate: deposit.wrapped_private }), "set#VaultDepositKey")
        }
        const host = post(maker, `${instances}/Hosts/storeHost`, { instanceHostId: "PodmanLocal", hostTypeId: "docker",
            hostProtocol: "unix", hostAddress: params.podmanSocket })
        if (host.status >= 400) throw new Error(`store#InstanceHost: ${host.status} ${host.text.slice(0, 300)}`)
        // the commit the fixture's passed run is for
        state.commit = rows("moqui.resource.GitPipelineRun/deploytest-release").commitId
        if (!state.commit) throw new Error("no passed run deploytest-release: run the fixture InstanceDeployFixtures first")
    },
})

feature("Postgres and MR Moqui are provisioned on the Podman host, and the instance pairs and reports unsealed", {
    when() {
        const created = (form) => {
            const res = post(users.maker, `${instances}/InstanceList/createAppInstance`, form)
            try {
                return waitUntil(() => rows("moqui.server.instance.AppInstance", { instanceName: form.instanceName })[0]?.appInstanceId,
                    { timeout: 10 })
            } catch (err) {
                const said = (res.text.match(/[^<>]*(rror|efused|not allowed|required)[^<>]*/g) || []).slice(0, 5).join(" | ")
                throw new Error(`create#AppInstance ${form.instanceName}: ${res.status} ${res.url} ${said || res.text.slice(0, 200)}`)
            }
        }
        state.pg = created({ instanceName: `pg-${tag}`, instanceImageId: "Postgres18", environmentId: ENVIRONMENT, instanceHostId: "PodmanLocal" })
        state.app = created({
            instanceName: `app-${tag}`,
            // the image without mr-home, the component deployed below
            instanceImageId: params.firstImage ?? "MoquiDeployBase",
            environmentId: ENVIRONMENT,
            instanceHostId: "PodmanLocal",
            databaseInstanceId: state.pg,
        })
        post(users.maker, `${instances}/InstanceDetail/requestInstanceAction`, { appInstanceId: state.app, actionEnumId: "IatProvision" })
        const provisioned = finished(state.app, (row) => row.actionEnumId == "IatProvision", 900)
        let last = null
        let up = null
        try {
            up = waitUntil(
                () => {
                    last = instance(state.app)
                    return last.sealState == "unsealed" && last.linkState == "linked" && last.observedState == "running" ? last : null
                },
                { timeout: 600, interval: 5000 },
            )
        } catch (err) {
            throw new Error(`not linked, unsealed and running after 600s: sealState ${last?.sealState}, linkState ${last?.linkState}, ` +
                `observedState ${last?.observedState}`)
        }
        state.firstDigest = up.imageDigest
        return { provisioned, up, pg: instance(state.pg) }
    },
    then({ provisioned, up, pg }) {
        provisioned.statusEnumId == "IasDone"
        pg.observedState == "running"
        up.observedState == "running"
        up.sealState == "unsealed"
        !!up.imageDigest
    },
})

feature("a component's commit is deployed over the link, gated by its passed run, as an action with its versions", {
    when() {
        if (!state.app) skip("the instances were not provisioned")
        const taken = takeIn()
        post(users.maker, `${instances}/InstanceDetail/requestDeploy`, {
            appInstanceId: state.app,
            componentName: "mr-home",
            repositoryId: "mr-home",
            refName: "refs/heads/master",
            commitId: state.commit,
        })
        const done = finished(state.app, (row) => row.actionEnumId == "IatDeploy", 900)
        state.componentAction = done.instanceActionId
        return { taken, done, row: instance(state.app), backups: actions(state.pg) }
    },
    then({ done, row, backups }) {
        done.statusEnumId == "IasDone"
        done.componentCommitsAfter.includes(state.commit)
        row.componentCommits.includes(state.commit)
        // its database was backed up before the change
        backups.some((action) => action.actionEnumId == "IatBackup" && action.statusEnumId == "IasDone")
    },
})

feature("a new image replaces the container, keeping its volume and its seal, and comes back unsealed", {
    when() {
        if (!state.componentAction) skip("the component was not deployed")
        takeIn()
        post(users.maker, `${instances}/InstanceDetail/requestDeploy`, { appInstanceId: state.app, instanceImageId: params.nextImage ?? "MoquiNext" })
        const done = finished(state.app, (row) => row.actionEnumId == "IatDeploy" && row.instanceActionId != state.componentAction, 900)
        state.imageAction = done.instanceActionId
        return { done, row: instance(state.app) }
    },
    then({ done, row }) {
        done.statusEnumId == "IasDone"
        done.versionBefore == state.firstDigest
        row.imageDigest != state.firstDigest
        row.sealState == "unsealed"
        // the data volume, with its seal and database settings, stayed: the component deployed before is still there
        row.componentCommits.includes(state.commit)
    },
})

feature("rolling the image deploy back returns to the recorded digest, unless its schema changed", {
    when() {
        if (!state.imageAction) skip("the image was not deployed")
        const before = actions(state.app).find((row) => row.instanceActionId == state.imageAction)
        const res = post(users.maker, `${instances}/InstanceDetail/rollbackDeploy`, { appInstanceId: state.app, rollbackOfActionId: state.imageAction })
        const schemaChanged = before.schemaHashBefore && before.schemaHashBefore != before.schemaHashAfter
        const done = schemaChanged ? null : finished(state.app, (row) => row.actionEnumId == "IatRollback", 900)
        return { res, done, schemaChanged, row: instance(state.app) }
    },
    then({ done, schemaChanged, row }) {
        // a schema change is not rolled back; else the digest recorded before the deploy is the instance's again
        schemaChanged || done.statusEnumId == "IasDone"
        schemaChanged || row.imageDigest == state.firstDigest
        row.sealState == "unsealed"
    },
})

feature("the instances are removed with their volumes", {
    when() {
        if (!state.app) skip("nothing was provisioned")
        for (const id of [state.app, state.pg]) {
            post(users.maker, `${instances}/InstanceDetail/requestInstanceAction`, { appInstanceId: id, actionEnumId: "IatRemove", removeVolumes: "true" })
        }
        waitUntil(() => [instance(state.app), instance(state.pg)].every((row) => !row.instanceUuid), {
            timeout: 300,
            interval: 5000,
        })
        return { app: instance(state.app), pg: instance(state.pg) }
    },
    then({ app, pg }) {
        !app.instanceUuid && app.observedState == "missing"
        !pg.instanceUuid && pg.observedState == "missing"
    },
})
