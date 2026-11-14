// System › Edge Clients on a node that takes links (plans/Edge-Node2.md §4.10): its Clients, Domains and Ports tabs,
// and the rules of the services they call. A host name has one pairing: the first claim holds, another pairing's add
// is refused with the generic "another node has this name", a revoked pairing gets none; a pairing's own queue cap is
// above 0 or unset; the services refuse on a node that takes no links. Through the screen an administrator (System's
// ADMIN) adds and removes a host name and a user without System cannot. A tunnel port goes to one active pairing,
// never on this node's own ports. Each tab is rendered once in qvt and once in
// tui (their state chips are render-mode sections), with all of its checks on that render. No link runs under moqui
// test: node_role is set to edge around each step, and the pairings are rows.
// Run: ./moq test framework --spec EdgeScreens
const S = "org.moqui.edge.EdgeServices."
const WEBROOT = "component://webroot/screen/webroot.xml"
const CLIENT = "moqui.edge.EdgeClient"
const HOST = "moqui.edge.EdgeHost"
const PORT = "moqui.edge.EdgePort"
// tunnel ports for the spec's pairings, none of this node's own
const SSH_PORT = "47322"
const XMPP_PORT = "47323"
const A = "edge-screens-spec-a"
const B = "edge-screens-spec-b"
const R = "edge-screens-spec-r"
const NODES = [A, B, R]
const NONE = "EDGE_SCREENS_SPEC_NONE"
const ERP = "erp.edge-screens-spec.example.test"
const SHOP = "shop.edge-screens-spec.example.test"
const SETUP_A = {
    mailDomains: ["branch-a.edge-screens-spec.example.test"],
    xmppDomain: "xmpp-a.edge-screens-spec.example.test",
    sipDomain: "sip-a.edge-screens-spec.example.test",
    chatBridgeIds: ["EDGE_SCREENS_BRIDGE"],
    passSites: ["forum-a"]
}

function sys(run) {
    const wasDisabled = ec.artifactExecution.disableAuthz()
    try {
        return run()
    } finally {
        if (!wasDisabled) ec.artifactExecution.enableAuthz()
    }
}
/** Run `fn` as a dedicated edge node would (node_role edge), with the property put back after. */
const edge = (fn) => withProperty("node_role", "edge", fn)
/** A service with authorization off (the rules, not the access), its errors returned. */
function invoke(name, parameters) {
    let out = {}
    let thrown = ""
    try {
        out = sys(() => ec.service.sync().name(S + name).parameters(parameters || {}).call()) || {}
    } catch (err) {
        thrown = String(err.message || err)
    }
    const error = thrown + (ec.message.hasError() ? ec.message.getErrorsString() : "")
    ec.message.clearErrors()
    return Object.assign({}, out, {error})
}
/** A screen render or a transition as `username`, with authorization on. */
function render(username, path, parameters, method, mode) {
    ec.user.internalLoginUser(username)
    ec.artifactExecution.enableAuthz()
    try {
        let test = ec.screen.makeTest().rootScreen(WEBROOT)
        if (mode) test = test.renderMode(mode)
        // a qvt render of a server-static screen has its data apart: with it, the output has both
        if (mode === "qvt") test = test.withStaticData()
        // as the clients ask for a tab: qvt loads each level on its own, the terminal the tab with its parent
        const standalone = mode === "qvt" ? {lastStandalone: "true"} : mode ? {lastStandalone: "-2"} : {}
        const out = test.render(path, Object.assign(standalone, parameters || {}), method || null)
        const text = String(out.output || "")
        // a widget that fails writes its error into the output, not into errorMessages
        const broken = /Error rendering|Exception in render/.test(text)
        return {status: out.status, text, broken, errors: Array.from(out.errorMessages || []).join(" | ")}
    } finally {
        ec.message.clearErrors()
        ec.user.internalLoginUser("john.doe")
        ec.artifactExecution.disableAuthz()
    }
}
const hostRow = (name) => sys(() => ec.entity.find(HOST).condition("hostName", name).one())
function client(pairedNodeId, statusId, setup) {
    const row = ec.entity.makeValue(CLIENT)
    row.setFields({pairedNodeId, keyFingerprint: "ab".repeat(32), statusId, binaryVersion: "0.1.0-spec", linkVersion: 1}, true, null, null)
    if (setup) row.setupJson = JSON.stringify(setup)
    row.create()
}

spec({
    setupSpec() {
        ec.user.internalLoginUser("john.doe")
        sys(() => {
            client(A, "EdgeClientActive", SETUP_A)
            client(B, "EdgeClientActive", null)
            client(R, "EdgeClientRevoked", null)
            const account = ec.entity.makeValue("moqui.security.UserAccount")
            account.setFields({userId: NONE, username: "edge.screens.spec.none", userFullName: "Edge Screens None", disabled: "N"}, true, null, null)
            account.create()
        })
    },
    cleanupSpec() {
        ec.message.clearErrors()
        sys(() => {
            ec.entity.find(HOST).condition("pairedNodeId", "in", NODES).deleteAll()
            ec.entity.find(PORT).condition("pairedNodeId", "in", NODES).deleteAll()
            ec.entity.find(CLIENT).condition("pairedNodeId", "in", NODES).deleteAll()
            ec.entity.find("moqui.security.UserAccount").condition("userId", NONE).deleteAll()
        })
    }
})

feature("a host name has one pairing: the first claim holds, refused for another pairing without naming the holder", {
    when() {
        context.out = edge(() => ({
            added: invoke("add#EdgeHost", {hostName: "ERP.Edge-Screens-Spec.example.test:443", pairedNodeId: A}),
            taken: invoke("add#EdgeHost", {hostName: ERP, pairedNodeId: B, modeEnumId: "EdgeHostTerminate"}),
            mode: invoke("add#EdgeHost", {hostName: ERP, pairedNodeId: A, modeEnumId: "EdgeHostTerminate"}),
            revoked: invoke("add#EdgeHost", {hostName: SHOP, pairedNodeId: R}),
            badName: invoke("add#EdgeHost", {hostName: "not a name", pairedNodeId: A}),
            badMode: invoke("add#EdgeHost", {hostName: SHOP, pairedNodeId: A, modeEnumId: "Other"}),
            listB: invoke("get#EdgeHosts", {pairedNodeId: B}),
            removed: invoke("remove#EdgeHost", {hostName: ERP.toUpperCase()}),
            again: invoke("remove#EdgeHost", {hostName: ERP})
        }))
        // a node that takes no links has no host names, domains or queue caps
        context.plain = {
            hosts: invoke("get#EdgeHosts"),
            add: invoke("add#EdgeHost", {hostName: SHOP, pairedNodeId: A}),
            domains: invoke("get#EdgeDomains"),
            cap: invoke("update#EdgeClient", {pairedNodeId: A, queueMaxMegabytes: "5"})
        }
    },
    then() {
        const out = context.out
        out.added.error === ""
        out.added.hostName === ERP
        out.added.modeEnumId === "EdgeHostPassthrough"
        out.taken.error.includes("another node has this name")
        !out.taken.error.includes(A)
        out.mode.error === ""
        out.mode.modeEnumId === "EdgeHostTerminate"
        out.revoked.error.includes("revoked")
        out.badName.error.includes("is not a host name")
        out.badMode.error !== ""
        out.listB.hostList.length === 0
        out.removed.removed === true
        out.again.removed === false
        hostRow(ERP) == null
        const plain = context.plain
        plain.hosts.error.includes("only a node that takes links")
        plain.add.error.includes("only a node that takes links")
        plain.domains.error.includes("only a node that takes links")
        plain.cap.error.includes("only a node that takes links")
    }
})

feature("a pairing's own queue cap is above 0, or unset for the conf's", {
    when() {
        context.out = edge(() => ({
            set: invoke("update#EdgeClient", {pairedNodeId: A, queueMaxMegabytes: "64"}),
            zero: invoke("update#EdgeClient", {pairedNodeId: A, queueMaxMegabytes: "0"}),
            word: invoke("update#EdgeClient", {pairedNodeId: A, queueMaxMegabytes: "lots"}),
            stranger: invoke("update#EdgeClient", {pairedNodeId: "edge-screens-spec-x", queueMaxMegabytes: "5"}),
            clients: invoke("get#EdgeClients"),
            unset: invoke("update#EdgeClient", {pairedNodeId: B}),
            after: invoke("get#EdgeClients")
        }))
    },
    then() {
        const out = context.out
        out.set.error === ""
        out.zero.error.includes("above 0")
        out.word.error.includes("not a whole number")
        out.stranger.error.includes("no local node")
        const a = out.clients.clientList.find((row) => row.pairedNodeId === A)
        a.queueMaxMegabytes == 64
        a.linked === false
        out.unset.error === ""
        out.after.clientList.find((row) => row.pairedNodeId === B).queueMaxMegabytes == null
    }
})

feature("an administrator adds and removes a host name on Domains; a user without System cannot", {
    when() {
        context.out = edge(() => ({
            none: render("edge.screens.spec.none", "apps/system/EdgeClients/Domains/addHost", {hostName: SHOP, pairedNodeId: B}, "post"),
            noneRow: hostRow(SHOP),
            admin: render("john.doe", "apps/system/EdgeClients/Domains/addHost", {hostName: SHOP, pairedNodeId: B, modeEnumId: "EdgeHostTerminate"}, "post"),
            adminRow: hostRow(SHOP),
            removed: render("john.doe", "apps/system/EdgeClients/Domains/removeHost", {hostName: SHOP}, "post"),
            removedRow: hostRow(SHOP)
        }))
    },
    then() {
        const out = context.out
        out.noneRow == null
        out.none.errors.includes("is not authorized")
        out.adminRow != null
        out.adminRow.pairedNodeId === B
        out.adminRow.modeEnumId === "EdgeHostTerminate"
        out.removedRow == null
    }
})

feature("Clients shows each pairing with its state, version, queue cap and the link not running", {
    when() {
        context.out = edge(() => {
            invoke("update#EdgeClient", {pairedNodeId: A, queueMaxMegabytes: "64"})
            return {
                qvt: render("john.doe", "apps/system/EdgeClients/Clients", {}, null, "qvt"),
                tui: render("john.doe", "apps/system/EdgeClients/Clients", {}, null, "tui")
            }
        })
    },
    then() {
        const {qvt, tui} = context.out
        qvt.status === 200
        qvt.errors === ""
        !qvt.broken
        qvt.text.includes(A) && qvt.text.includes(B) && qvt.text.includes(R)
        qvt.text.includes("Offline") && qvt.text.includes("Revoked")
        qvt.text.includes("64 MB")
        qvt.text.includes("0.1.0-spec")
        // pairing codes and revoking go through the link, which does not run here
        !qvt.text.includes("Make a pairing code")
        !qvt.text.includes("revokeClient")
        qvt.text.includes("Queue cap")
        // the tui template of the state chip, under Edge Clients' own notice
        tui.status === 200
        tui.errors === ""
        !tui.broken
        tui.text.includes("This node takes no links now")
        tui.text.includes(A)
        tui.text.includes("Revoked")
        tui.text.includes("64 MB")
    }
})

feature("Domains shows each pairing's host names and what its setup claims, and the conflicts", {
    when() {
        context.out = edge(() => {
            invoke("add#EdgeHost", {hostName: ERP, pairedNodeId: A, modeEnumId: "EdgeHostTerminate"})
            const out = {
                qvt: render("john.doe", "apps/system/EdgeClients/Domains", {}, null, "qvt"),
                tui: render("john.doe", "apps/system/EdgeClients/Domains", {}, null, "tui")
            }
            invoke("remove#EdgeHost", {hostName: ERP})
            return out
        })
    },
    then() {
        const {qvt, tui} = context.out
        qvt.status === 200
        qvt.errors === ""
        !qvt.broken
        qvt.text.includes(ERP)
        qvt.text.includes("Terminate")
        qvt.text.includes(SETUP_A.mailDomains[0])
        qvt.text.includes(SETUP_A.xmppDomain)
        qvt.text.includes(SETUP_A.sipDomain)
        qvt.text.includes("EDGE_SCREENS_BRIDGE")
        qvt.text.includes("forum-a")
        qvt.text.includes("Replica conflicts")
        // a revoked pairing claims nothing
        !qvt.text.includes(R)
        // the tui template of the mode chip
        tui.status === 200
        tui.errors === ""
        !tui.broken
        tui.text.includes(ERP)
        tui.text.includes("Terminate")
    }
})

feature("a tunnel port goes to one active pairing, never on this node's own ports; Ports lists them", {
    when() {
        context.out = edge(() => {
            const out = {
                ssh: invoke("add#EdgePort", {port: SSH_PORT, pairedNodeId: A, tunnelKind: "ssh"}),
                same: invoke("add#EdgePort", {port: SSH_PORT, pairedNodeId: A, tunnelKind: "ssh"}),
                taken: invoke("add#EdgePort", {port: SSH_PORT, pairedNodeId: B, tunnelKind: "ssh"}),
                own: invoke("add#EdgePort", {port: "443", pairedNodeId: B, tunnelKind: "xmpps"}),
                revoked: invoke("add#EdgePort", {port: XMPP_PORT, pairedNodeId: R, tunnelKind: "xmpp"}),
                kind: invoke("add#EdgePort", {port: XMPP_PORT, pairedNodeId: B, tunnelKind: "telnet"}),
                xmpp: invoke("add#EdgePort", {port: XMPP_PORT, pairedNodeId: B, tunnelKind: "xmpp"}),
                listB: invoke("get#EdgePorts", {pairedNodeId: B}),
                qvt: render("john.doe", "apps/system/EdgeClients/Ports", {}, null, "qvt"),
                tui: render("john.doe", "apps/system/EdgeClients/Ports", {}, null, "tui"),
                removed: invoke("remove#EdgePort", {port: XMPP_PORT}),
                again: invoke("remove#EdgePort", {port: XMPP_PORT})
            }
            invoke("remove#EdgePort", {port: SSH_PORT})
            return out
        })
        context.plain = invoke("add#EdgePort", {port: XMPP_PORT, pairedNodeId: B, tunnelKind: "xmpp"})
    },
    then() {
        const out = context.out
        out.ssh.error === ""
        out.ssh.port == Number(SSH_PORT)
        out.same.error === ""
        out.taken.error.includes("remove it first")
        out.own.error.includes("this node's own")
        out.revoked.error.includes("not an active paired node")
        out.kind.error.includes("tunnelKind")
        out.xmpp.error === ""
        out.listB.portList.length === 1
        out.listB.portList[0].tunnelKind === "xmpp"
        // no link runs here, so nothing listens yet
        out.listB.portList[0].listening === false
        out.removed.removed === true
        out.again.removed === false
        context.plain.error.includes("only a node that takes links")
        const {qvt, tui} = out
        qvt.status === 200
        qvt.errors === ""
        !qvt.broken
        qvt.text.includes(SSH_PORT) && qvt.text.includes(XMPP_PORT)
        qvt.text.includes("XMPP, STARTTLS")
        qvt.text.includes("Add a port")
        // the tui template of the state chip
        tui.status === 200
        tui.errors === ""
        !tui.broken
        tui.text.includes(SSH_PORT)
        tui.text.includes("Waiting")
    }
})
