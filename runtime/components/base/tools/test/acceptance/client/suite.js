// The browser walks of the Monitor screens, the data grid, its Calc mode, Tools › Load and Maps (./moq accept tools), against the
// framework's client fixtures and the Maps walks' own (MapsWalkFixtures).
suite(["Monitoring", "DataGrid", "CalcGrid", "LoadRunner", "MapsBrowseWalk", "MapsDialogWalk"], {
    server: { components: ["framework", "tools"], specs: ["ClientFixtures", "MapsWalkFixtures"], conf: "conf/MoquiLoadTestConf.xml" },
})
