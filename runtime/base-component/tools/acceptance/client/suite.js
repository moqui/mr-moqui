// The browser walks of the Monitor screens and the data grid (./moq accept tools), against the framework's client fixtures.
suite(["Monitoring", "DataGrid"], {
    server: { components: ["framework", "tools"], specs: ["ClientFixtures"], conf: "conf/MoquiLoadTestConf.xml" },
})
