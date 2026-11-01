// The edge link's System screens (plans/Edge-Node.md §5.3, §5.7): Edge Node (its status, and the Pair wizard's two steps) and Edge
// Clients (with its host names) render in qvt and tui with no error, each with the text of this node's case: no link in this process (a build
// without it, or one where no HTTP listener started it), a local node not paired yet, or an edge node (node_role=edge).
// No edge node is reached. moqui test starts no HTTP listener, which installs the link, so a plain run is the first
// case and the others skip. --serve starts the listener only after the specs, so the local and edge cases are checked
// over HTTP on a served Maximal build (-D node_role=edge, and -D webapp_https_listen_port for an edge node that takes
// links).
// Run: ./moq test tools --spec EdgeScreens
const systemRoot = "component://tools/screen/System.xml"
const NEEDS_EDGE =
    "needs an edge node with the link installed; moqui test starts no HTTP listener, and --serve starts it after the specs: " +
    "check over HTTP on a served Maximal build with -D node_role=edge"

function render(path, mode, parameters) {
    let test = ec.screen.makeTest().rootScreen(systemRoot).renderMode(mode)
    if (mode === "qvt") test = test.withStaticData()
    const str = test.render(path, Object.assign({lastStandalone: "-2"}, parameters || {}), null)
    const output = String(str.output || "")
    ec.message.clearErrors()
    return {
        status: str.status,
        errors: str.errorMessages.join(" | "),
        output,
        broken: /Error rendering|Exception in render/.test(output)
    }
}
function linkStatus() {
    return ec.service.sync().name("org.moqui.impl.EdgeServices.get#LinkStatus").disableAuthz().call() || {}
}
// noLink: no link in this process; local: a local node; edge: an edge node with the link installed
function caseOf(link) {
    if (link.state === "notBuilt" || link.state === "notStarted") return "noLink"
    return link.nodeRole === "edge" ? "edge" : "local"
}

spec({
    setupSpec() {
        login("john.doe")
    }
})

// A served run leaves the code it makes behind; it expires in 15 minutes and pairs nothing.
feature("on an edge node, Make a pairing code shows a new code once and keeps only its hash", {
    given() {
        if (caseOf(linkStatus()) !== "edge") skip(NEEDS_EDGE)
    },
    when() {
        const codes = () => ec.entity.find("moqui.edge.EdgePairingCode").disableAuthz().useCache(false).count()
        const before = codes()
        const test = ec.screen.makeTest().rootScreen(systemRoot).renderMode("qvt").withStaticData()
        const made = test.render("EdgeClients/makePairingCode", {validMinutes: "15"}, "post")
        const first = String(test.render("EdgeClients", {lastStandalone: "-2"}, null).output || "")
        const second = String(test.render("EdgeClients", {lastStandalone: "-2"}, null).output || "")
        const after = codes()
        ec.message.clearErrors()
        return {madeErrors: made.errorMessages.join(" | "), first, second, added: after - before}
    },
    then({madeErrors, first, second, added}) {
        madeErrors == ""
        added == 1
        first.includes("It is shown only this once")
        !second.includes("It is shown only this once")
        first.includes("This edge node&#39;s link key fingerprint") || first.includes("This edge node's link key fingerprint")
    }
})

// A served run with a paired local node: the host name is removed again.
feature("on an edge node, a host name is added for a paired node, listed, and removed", {
    given() {
        if (caseOf(linkStatus()) !== "edge") skip(NEEDS_EDGE)
        const out = ec.service.sync().name("org.moqui.impl.EdgeServices.get#EdgeClients").disableAuthz().call() || {}
        const node = Array.from(out.clientList || []).find((client) => client.statusId === "EdgeClientActive")
        if (!node) skip("needs a local node paired with this edge node")
        return {node: node.pairedNodeId}
    },
    when({node}) {
        const hostName = "edge-spec-host.example.com"
        const test = ec.screen.makeTest().rootScreen(systemRoot).renderMode("qvt").withStaticData()
        const added = test.render("EdgeClients/addHost", {hostName: "Edge-Spec-Host.example.com", pairedNodeId: node, modeEnumId: "EdgeHostTerminate"}, "post")
        const listed = String(test.render("EdgeClients", {lastStandalone: "-2"}, null).output || "")
        const removed = test.render("EdgeClients/removeHost", {hostName}, "post")
        const after = String(test.render("EdgeClients", {lastStandalone: "-2"}, null).output || "")
        ec.message.clearErrors()
        return {hostName, addedErrors: added.errorMessages.join(" | "), listed,
            removedErrors: removed.errorMessages.join(" | "), after}
    },
    then({hostName, addedErrors, listed, removedErrors, after}) {
        addedErrors == ""
        listed.includes(hostName)
        listed.includes("Terminate")
        removedErrors == ""
        !after.includes(hostName)
    }
})

// what step 1 kept in the session; the address does not answer, so a check made again would fail
feature("Connect shows the answer step 1 kept in the session, and asks the edge node again for another address", {
    when() {
        const url = "https://127.0.0.1:1"
        const fingerprint = "ab".repeat(32)
        ec.web.sessionAttributes.edgeCheck = JSON.stringify({
            url,
            edgeName: "Spec edge",
            fingerprint,
            binaryVersion: "0.1.0",
            linkVersion: "1",
            publicNames: ["edge.example.com"],
            features: [],
            compatible: true,
            message: ""
        })
        const kept = render("EdgeNode/Pair/Connect", "tui", {lastStandalone: "true", url})
        const other = render("EdgeNode/Pair/Connect", "tui", {lastStandalone: "true", url: "https://127.0.0.1:2"})
        ec.web.sessionAttributes.remove("edgeCheck")
        return {kept, other, fingerprint}
    },
    then({kept, other, fingerprint}) {
        kept.status == 200 && !kept.broken && kept.errors == ""
        kept.output.includes("Spec edge")
        kept.output.includes(fingerprint)
        other.status == 200 && !other.broken && other.errors == ""
        !other.output.includes("Spec edge")
        other.output.includes("did not answer")
    }
})

feature("Edge Node and Edge Clients render the #nodeCase case in qvt and tui", {
    where: [
        {
            nodeCase: "noLink",
            texts: {
                EdgeNode: "edge link",
                "EdgeNode/Pair/EdgeUrl": "edge link",
                "EdgeNode/Pair/Connect": "address first",
                EdgeClients: "edge link"
            }
        },
        {
            nodeCase: "local",
            texts: {
                EdgeNode: "Pair with an edge node",
                "EdgeNode/Pair/EdgeUrl": "Pairing code",
                "EdgeNode/Pair/Connect": "address first",
                EdgeClients: "This node is not an edge node"
            }
        },
        {
            nodeCase: "edge",
            texts: {
                EdgeNode: "This node is an edge node",
                "EdgeNode/Pair/EdgeUrl": "This node is an edge node",
                "EdgeNode/Pair/Connect": "address first",
                EdgeClients: "Make a pairing code"
            }
        }
    ],
    given({nodeCase}) {
        const link = linkStatus()
        if (caseOf(link) !== nodeCase) skip(`this node is the ${caseOf(link)} case (state ${link.state})`)
    },
    when({texts}) {
        const problems = []
        for (const [path, text] of Object.entries(texts)) {
            for (const mode of ["qvt", "tui"]) {
                // a wizard step alone: in qvt its wizard loads the step in the browser
                const shown = render(path, mode, path.includes("/Pair/") ? {lastStandalone: "true"} : {})
                if (shown.status != 200 || shown.broken || shown.errors || !shown.output.includes(text))
                    problems.push(`${mode} ${path}: ${shown.status} ${shown.errors} ${shown.output.slice(0, 300)}`)
            }
        }
        return {problems}
    },
    then({problems}) {
        problems.join("\n") == ""
    }
})
