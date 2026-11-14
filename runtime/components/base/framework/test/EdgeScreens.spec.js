// System › Edge Clients on a node that takes links (plans/Edge-Node2.md §4.10): its Clients, Domains, Ports and Balancing tabs,
// and the rules of the services they call. A host name has one pairing: the first claim holds, another pairing's add
// is refused with the generic "another node has this name", a revoked pairing gets none; a pairing's own queue cap is
// above 0 or unset; the services refuse on a node that takes no links. Through the screen an administrator (System's
// ADMIN) adds and removes a host name and a user without System cannot. A tunnel port goes to one active pairing,
// never on this node's own ports. A pool's rows (plans/Edge-Node2.md §4.4, §4.5): a pairing's hold of new nodes, an
// approval, a takeover cleared, a member's key forgotten and Deliver again since, each refused for a pairing that is
// not one and on a node that takes no links, and the takeover cleared through Clients by an administrator but not by a
// user without System. On a local node, System › Edge Node shows its own cluster and drains and resumes itself. Each
// tab is rendered once in qvt and once in tui (their state chips are render-mode sections), with all of its checks on
// that render; Domains shows each host's sticky cookie, and Balancing the conf's defaults. Edge cluster (plans/Edge-Node2.md
// §4.6) lists this node's own cluster from its rows with each member's link address, one that is not https marked as
// not used, the member holding the SIP trunk lock (none once its lease lapsed), the front node it is paired with from
// what pairing wrote (plans/Edge-Node2.md §4.7; none without), and get#EdgeCluster refuses on a node that takes no
// links. No link runs under moqui test: node_role is set to front_edge around each step, the pairings
// are rows, and no member is linked. A local node's links to each edge member (System › Edge Node, Edge cluster) need
// a running link: their rows are get#LinkStatus's edgeMemberList, shaped in moqui-core's service::edge tests.
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
const MEMBER = "moqui.server.ClusterMember"
const LOCK = "moqui.edge.EdgeSenderLock"
// a member of this node's cluster that stopped a day ago, with its link address
const EDGE2 = "edge-screens-spec-edge2"
const EDGE2_URL = "https://edge2.edge-screens-spec.example.test"
// the front node this node's cluster is paired with, as pairing writes it to .env
const FRONT_URL = "https://front.edge-screens-spec.example.test"
// the front's member ids, as pairing writes them
const FRONT_ID = "edge-screens-spec-front,edge-screens-spec-front2"
const STANDBY = "edge-screens-spec-standby"
const FIRST = "edge-screens-spec-first"
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
/** Run `fn` as a dedicated edge node would (node_role front_edge), with the property put back after. */
const edge = (fn) => withProperty("node_role", "front_edge", fn)
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
/** Note on pairing `node` that the standby took it over from the first machine, as admission does. */
function takeover(node) {
    sys(() => ec.entity.makeValue(CLIENT).setAll({pairedNodeId: node, takeoverDate: ec.user.nowTimestamp,
        takeoverFromId: FIRST, takeoverToId: STANDBY, lastMemberId: STANDBY}).update())
}
const clientRow = (node) => sys(() => ec.entity.find(CLIENT).condition("pairedNodeId", node).useCache(false).one())
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
        // a failed step may leave this node drained
        invoke("drain#EdgeMember", {drain: false})
        sys(() => {
            ec.entity.find(HOST).condition("pairedNodeId", "in", NODES).deleteAll()
            ec.entity.find(PORT).condition("pairedNodeId", "in", NODES).deleteAll()
            ec.entity.find(CLIENT).condition("pairedNodeId", "in", NODES).deleteAll()
            ec.entity.find(MEMBER).condition("instanceId", EDGE2).deleteAll()
            ec.entity.find(LOCK).condition("lockName", "sipTrunks").deleteAll()
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

feature("Clients shows each pairing with its state, version, queue cap, new node hold and takeover, the link not running", {
    when() {
        context.out = edge(() => {
            invoke("update#EdgeClient", {pairedNodeId: A, queueMaxMegabytes: "64"})
            invoke("hold#NewMembers", {pairedNodeId: B, hold: true})
            takeover(A)
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
        // the pool: B holds a new node for approval, A was taken over and its first machine waits; no member linked
        qvt.text.includes("Held for approval") && qvt.text.includes("Admitted, with a notice")
        qvt.text.includes("Takeovers") && qvt.text.includes(STANDBY) && qvt.text.includes(FIRST)
        qvt.text.includes("Refused until you let it link again") && qvt.text.includes("clearTakeover")
        qvt.text.includes("No member has linked since this node started")
        qvt.text.includes("Deliver again") && qvt.text.includes("New nodes")
        // the tui template of the state chip, under Edge Clients' own notice
        tui.status === 200
        tui.errors === ""
        !tui.broken
        tui.text.includes("This node takes no links now")
        tui.text.includes(A)
        tui.text.includes("Revoked")
        tui.text.includes("64 MB")
        tui.text.includes(STANDBY)
    }
})

feature("Domains shows each pairing's host names and what its setup claims, and the conflicts", {
    when() {
        context.out = edge(() => {
            invoke("add#EdgeHost", {hostName: ERP, pairedNodeId: A, modeEnumId: "EdgeHostTerminate", stickyCookie: "erp_member"})
            invoke("add#EdgeHost", {hostName: SHOP, pairedNodeId: A})
            // Add a host name again with no cookie keeps the host's own
            const again = render("john.doe", "apps/system/EdgeClients/Domains/addHost", {hostName: ERP, pairedNodeId: A, modeEnumId: "EdgeHostTerminate"}, "post")
            const out = {
                again, againRow: hostRow(ERP),
                qvt: render("john.doe", "apps/system/EdgeClients/Domains", {}, null, "qvt"),
                tui: render("john.doe", "apps/system/EdgeClients/Domains", {}, null, "tui")
            }
            invoke("remove#EdgeHost", {hostName: ERP})
            invoke("remove#EdgeHost", {hostName: SHOP})
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
        // a terminate host is sticky by its own cookie, with a dialog to change it; a passthrough one by the client's address
        qvt.text.includes("Sticky by") && qvt.text.includes("erp_member")
        qvt.text.includes("By client address")
        qvt.text.includes("The sticky cookie of " + ERP)
        out.again.errors === ""
        out.againRow.stickyCookie === "erp_member"
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
                // an smtp port is a front node's (plans/Edge-Node2.md §4.7): this node is none
                smtp: invoke("add#EdgePort", {port: XMPP_PORT, pairedNodeId: B, tunnelKind: "smtp"}),
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
        out.smtp.error.includes("front node's")
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

feature("a pool's rows: hold, approve, clear a takeover, forget a key, deliver again; refused for a stranger and off an edge node", {
    when() {
        sys(() => ec.entity.makeValue(CLIENT).setAll({pairedNodeId: B,
            memberKeys: JSON.stringify([[FIRST, "ab".repeat(32)], [STANDBY, "cd".repeat(32)]])}).update())
        takeover(B)
        context.out = edge(() => ({
            hold: invoke("hold#NewMembers", {pairedNodeId: B, hold: true}),
            holdStranger: invoke("hold#NewMembers", {pairedNodeId: "edge-screens-spec-x", hold: true}),
            approve: invoke("approve#EdgeMember", {pairedNodeId: B, instanceId: STANDBY}),
            approveBad: invoke("approve#EdgeMember", {pairedNodeId: B, instanceId: "two words"}),
            held: clientRow(B),
            release: invoke("hold#NewMembers", {pairedNodeId: B}),
            forget: invoke("forget#EdgeMember", {pairedNodeId: B, instanceId: FIRST}),
            forgetAgain: invoke("forget#EdgeMember", {pairedNodeId: B, instanceId: FIRST}),
            clear: invoke("clear#EdgeTakeover", {pairedNodeId: B}),
            after: clientRow(B),
            deliver: invoke("deliver#ItemsAgain", {pairedNodeId: B, sinceDate: ec.user.nowTimestamp}),
            deliverNoDate: invoke("deliver#ItemsAgain", {pairedNodeId: B}),
            deliverStranger: invoke("deliver#ItemsAgain", {pairedNodeId: "edge-screens-spec-x", sinceDate: ec.user.nowTimestamp}),
            members: invoke("get#EdgeMembers"),
            balancing: invoke("get#EdgeBalancing"),
            ping: invoke("ping#EdgeClient", {pairedNodeId: B, instanceId: STANDBY})
        }))
        context.plain = {
            hold: invoke("hold#NewMembers", {pairedNodeId: B, hold: true}),
            clear: invoke("clear#EdgeTakeover", {pairedNodeId: B}),
            members: invoke("get#EdgeMembers"),
            balancing: invoke("get#EdgeBalancing")
        }
    },
    then() {
        const out = context.out
        out.hold.error === ""
        out.hold.hold === true
        out.holdStranger.error.includes("is not paired")
        out.approve.approved === true
        out.approveBad.error.includes("not an instance id")
        out.held.holdNewMember === "Y"
        out.held.approvedMemberId === STANDBY
        out.release.hold === false
        out.forget.forgotten === true
        out.forgetAgain.forgotten === false
        out.clear.cleared === true
        // the displaced node may link again; the takeover stays noted, and the other member's key stays pinned
        out.after.holdNewMember === "N"
        out.after.takeoverFromId == null
        out.after.takeoverToId === STANDBY
        out.after.takeoverDate != null
        // the forgotten id is kept as forgotten, so its next link is a new node's, not the last member coming back
        out.after.memberKeys.includes(JSON.stringify([FIRST, "forgotten"])) && out.after.memberKeys.includes("cd".repeat(32))
        out.deliver.error === ""
        out.deliver.itemCount == 0
        out.deliverNoDate.error.includes("Since Date")
        out.deliverStranger.error.includes("is not paired")
        // no link runs here: no member is linked, and a member's ping has nowhere to go
        out.members.error === ""
        out.members.memberList.length === 0
        // with no link, balancing's defaults from the conf and no members
        out.balancing.error === ""
        out.balancing.stickyCookie === "moqui.session"
        out.balancing.setAsideFailures == 3
        out.balancing.memberList.length === 0
        out.ping.error !== ""
        context.plain.hold.error.includes("only a node that takes links")
        context.plain.clear.error.includes("only a node that takes links")
        context.plain.members.error.includes("only a node that takes links")
        context.plain.balancing.error.includes("only a node that takes links")
    }
})

feature("an administrator lets a displaced node link again on Clients; a user without System cannot", {
    when() {
        context.out = edge(() => {
            takeover(A)
            return {
                none: render("edge.screens.spec.none", "apps/system/EdgeClients/Clients/clearTakeover", {pairedNodeId: A}, "post"),
                noneRow: clientRow(A),
                admin: render("john.doe", "apps/system/EdgeClients/Clients/clearTakeover", {pairedNodeId: A}, "post"),
                adminRow: clientRow(A)
            }
        })
    },
    then() {
        const out = context.out
        out.none.errors.includes("is not authorized")
        out.noneRow.takeoverFromId === FIRST
        out.admin.errors === ""
        out.adminRow.takeoverFromId == null
        out.adminRow.takeoverToId === STANDBY
    }
})

feature("a local node's Edge Node shows its own cluster, and it drains and resumes itself", {
    when() {
        // paired by its .env, with the link not running in this process (no node_role: a local node)
        context.out = withProperty("edge_node_url", "https://edge.edge-screens-spec.example.test", () => {
            const before = invoke("get#LocalCluster")
            const drained = invoke("drain#EdgeMember", {drain: true})
            const during = invoke("get#LocalCluster")
            const out = {
                before, drained, during,
                qvt: render("john.doe", "apps/system/EdgeNode/Status", {}, null, "qvt"),
                tui: render("john.doe", "apps/system/EdgeNode/Status", {}, null, "tui")
            }
            out.resumed = invoke("drain#EdgeMember", {drain: false})
            out.after = invoke("get#LocalCluster")
            return out
        })
    },
    then() {
        const out = context.out
        out.before.error === ""
        out.before.instanceId !== ""
        out.before.memberList.some((member) => member.own && member.instanceId === out.before.instanceId)
        out.drained.state === "draining"
        out.during.state === "draining"
        out.resumed.error === ""
        out.resumed.state !== "draining"
        out.after.state !== "draining"
        const {qvt, tui} = out
        qvt.status === 200
        qvt.errors === ""
        !qvt.broken
        qvt.text.includes("This node's cluster")
        qvt.text.includes(out.before.instanceId)
        qvt.text.includes("Draining") && qvt.text.includes("Resume")
        !qvt.text.includes("Drain this node")
        // the tui template of the members' chip
        tui.status === 200
        tui.errors === ""
        !tui.broken
        tui.text.includes("This node's cluster")
        tui.text.includes("(this node)")
        tui.text.includes("Draining")
    }
})

feature("Balancing shows the conf's defaults and, with no link, no member sent work", {
    when() {
        context.out = edge(() => ({
            qvt: render("john.doe", "apps/system/EdgeClients/Balancing", {}, null, "qvt"),
            tui: render("john.doe", "apps/system/EdgeClients/Balancing", {}, null, "tui")
        }))
    },
    then() {
        const {qvt, tui} = context.out
        qvt.status === 200
        qvt.errors === ""
        !qvt.broken
        qvt.text.includes("moqui.session")
        qvt.text.includes("After 3 failures in a row, for 30 s")
        qvt.text.includes("No member has been sent work since this node started")
        !qvt.text.includes("Balancing could not be read")
        // the tui render of the tab under Edge Clients' own notice (its chip is a render-mode section)
        tui.status === 200
        tui.errors === ""
        !tui.broken
        tui.text.includes("This node takes no links now")
        tui.text.includes("moqui.session")
    }
})

feature("Edge cluster lists this node's cluster with each member's link address, with no link running", {
    when() {
        sys(() => ec.entity.makeValue(MEMBER).setAll({instanceId: EDGE2, heartbeat: new Date(Date.now() - 86400000),
            binaryVersion: "0.1.0-spec", edgeLinkUrl: EDGE2_URL}).create())
        // this node registers the trunks: it holds the lock row its SIP engine would renew (no engine runs here)
        const own = String(edge(() => invoke("get#EdgeCluster")).memberList.find((member) => member.thisMember).instanceId)
        sys(() => ec.entity.makeValue(LOCK).setAll({lockName: "sipTrunks", instanceId: own,
            leaseExpireDate: new Date(Date.now() + 600000), claimedDate: new Date()}).createOrUpdate())
        // this node's own address is not https, so its Hello answer would send none
        // paired with a front node by its .env, the link not running here
        context.out = edge(() => withProperty("edge_link_url", "http://edge1.edge-screens-spec.example.test", () =>
            withProperty("front_node_url", FRONT_URL, () => withProperty("front_node_id", FRONT_ID, () => ({
                cluster: invoke("get#EdgeCluster"),
                qvt: render("john.doe", "apps/system/EdgeClients/EdgeCluster", {}, null, "qvt"),
                tui: render("john.doe", "apps/system/EdgeClients/EdgeCluster", {}, null, "tui")
            })))))
        // a lapsed lease registers nothing
        sys(() => ec.entity.makeValue(LOCK).setAll({lockName: "sipTrunks", leaseExpireDate: new Date(Date.now() - 60000)}).update())
        context.lapsed = edge(() => invoke("get#EdgeCluster"))
        context.plain = invoke("get#EdgeCluster")
    },
    then() {
        const out = context.out
        out.cluster.error === ""
        const own = out.cluster.memberList.find((member) => member.thisMember)
        own.live === true
        own.linkUrl === "http://edge1.edge-screens-spec.example.test"
        own.linkUrlUsable === false
        const other = out.cluster.memberList.find((member) => member.instanceId === EDGE2)
        other.live === false
        other.thisMember === false
        other.linkUrl === EDGE2_URL
        other.linkUrlUsable === true
        own.registersTrunks === true
        other.registersTrunks === false
        context.lapsed.memberList.every((member) => member.registersTrunks === false)
        // the front pairing from .env, one row not started here; none without it
        out.cluster.frontList.length === 1
        out.cluster.frontList[0].url === FRONT_URL
        out.cluster.frontList[0].instanceId === FRONT_ID
        out.cluster.frontList[0].state === "notStarted"
        context.lapsed.frontList.length === 0
        other.heartbeatAgeMs > 3600000
        const {qvt, tui} = out
        qvt.status === 200
        qvt.errors === ""
        !qvt.broken
        qvt.text.includes(own.instanceId + " (this node)")
        qvt.text.includes("(not used: not an https address)")
        qvt.text.includes(EDGE2_URL)
        qvt.text.includes("Stopped")
        qvt.text.includes("Registers them")
        // one live member needs no address; the warning is for a live member with none beside another
        qvt.text.includes("so it needs no link address")
        !qvt.text.includes("Members with no link address")
        !qvt.text.includes("could not be read")
        qvt.text.includes("Front node")
        qvt.text.includes(FRONT_URL)
        qvt.text.includes("Not started here")
        !qvt.text.includes("No front node. Behind one")
        // the tui template of the live chip
        tui.status === 200
        tui.errors === ""
        !tui.broken
        tui.text.includes("(this node)")
        tui.text.includes("Stopped")
        tui.text.includes(EDGE2_URL)
        tui.text.includes("edge-screens-spec-front, edge-screens-spec-front2")
        context.plain.error.includes("only a node that takes links")
    }
})
