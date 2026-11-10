// The browser walks of the Monitor screens, the data grid and Tools › Load (./moq accept tools), against the framework's client fixtures.
suite(["Monitoring", "DataGrid", "LoadRunner"], {
    server: { components: ["framework", "tools"], specs: ["ClientFixtures"], conf: "conf/MoquiLoadTestConf.xml" },
})
