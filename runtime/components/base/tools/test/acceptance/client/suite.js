// The browser walks of the Monitor screens, the data grid, its Calc mode, Tools › Load and Maps (./moq accept tools), against the
// framework's client fixtures, the Maps walks' own (MapsWalkFixtures, MapsLocalFixtures) and the CalcGrid share walk's
// (CalcShareFixtures). The server is the framework and
// tools alone (MapsFrameworkOnly: an installation without Mantle), with maps@external-requests false (MapsLocalOnly; no other
// walk asks an outside host).
suite(["Monitoring", "DataGrid", "CalcGrid", "LoadRunner", "MapsBrowseWalk", "MapsDialogWalk", "MapsLocalOnly", "MapsFrameworkOnly"], {
    server: { components: ["framework", "tools"], specs: ["ClientFixtures", "MapsWalkFixtures", "MapsLocalFixtures", "CalcShareFixtures"], conf: "conf/MoquiLoadTestConf.xml",
        properties: { moqui_conf__maps__external_requests: "false" } },
})
