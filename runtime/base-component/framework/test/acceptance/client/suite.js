// The framework's acceptance client specs (./moq accept framework): a load run, and the tarpit case, which runs on a
// server with the tarpits on (under the load test conf tarpit-enabled is false and its features skip). The server is
// the framework under the load test conf with the users of test/ClientFixtures.spec.js.
suite(["SecurityTarpit", "LoadBasic"], {
    server: { components: ["framework"], specs: ["ClientFixtures"], conf: "conf/MoquiLoadTestConf.xml" },
})
