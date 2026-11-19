// System › Print › Printers (tools/screen/System/Print/Printer/PrinterList.xml) makes and changes a printer as Admin's
// Connections do, through store#Connection (plans/Security-Review-Medium.md SEC-RTE-83): a new printer waits for a test
// print, a field the printer kind does not take (allowPrivateNetwork, disabled) is not written, and the form-list's update
// changes the name and the location only.
// Run: ./moq test tools --spec SystemPrinters
const PRINTER = "moqui.basic.print.NetworkPrinter"
const ID = "SPEC_SYS_PRINTER"
const printers = (transition, parameters) => {
    const out = ec.screen.makeTest().rootScreen("component://tools/screen/System.xml")
        .render("Print/Printer/PrinterList/" + transition, parameters, "post")
    ec.message.clearAll()
    return out
}
const row = () => {
    const wasDisabled = ec.artifactExecution.disableAuthz()
    try {
        return ec.entity.find(PRINTER).condition("networkPrinterId", ID).useCache(false).one()
    } finally {
        if (!wasDisabled) ec.artifactExecution.enableAuthz()
    }
}

spec({
    setupSpec() { login("john.doe") },
    cleanupSpec() {
        const wasDisabled = ec.artifactExecution.disableAuthz()
        ec.entity.find("moqui.server.ConnectionCheck").condition({adapterKey: "printer", connectionKey: ID}).deleteAll()
        ec.entity.find(PRINTER).condition("networkPrinterId", ID).deleteAll()
        if (!wasDisabled) ec.artifactExecution.enableAuthz()
    },
})

feature("a printer made on System › Print waits for a test print and takes only the printer kind's fields", {
    when() {
        printers("createNetworkPrinter", {networkPrinterId: ID, serverHost: "print.example.com", serverPort: "631",
            printerName: "front", description: "Spec front desk", allowPrivateNetwork: "Y", disabled: "N"})
        const made = row()
        printers("updateNetworkPrinter", {networkPrinterId: ID, description: "Spec renamed", location: "Lobby",
            serverHost: "taker.example.net", allowPrivateNetwork: "Y"})
        return {made: made ? made.getMap() : null, after: row()?.getMap()}
    },
    then({made, after}) {
        made != null
        made.disabled == "Y"
        made.allowPrivateNetwork != "Y"
        after.description == "Spec renamed"
        after.location == "Lobby"
        after.serverHost == "print.example.com"
        after.allowPrivateNetwork != "Y"
    },
})
