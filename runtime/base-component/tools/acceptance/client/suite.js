// The browser walks of the Monitor screens, the data grid, its Calc mode and Tools › Load (./moq accept tools), against the framework's client fixtures.
suite(["Monitoring", "DataGrid", "CalcGrid", "LoadRunner"], {
    server: { components: ["framework", "tools"], specs: ["ClientFixtures"], conf: "conf/MoquiLoadTestConf.xml" },
})
