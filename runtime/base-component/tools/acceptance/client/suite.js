// The browser walk of the Monitor screens (./moq accept tools), against the framework's client fixtures.
suite(["Monitoring"], {
    server: { components: ["framework", "tools"], specs: ["ClientFixtures"], conf: "conf/MoquiLoadTestConf.xml" },
})
