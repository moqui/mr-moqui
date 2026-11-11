// The order the framework's client specs run in (moqui test-client framework), and the server they run against with
// --serve: the framework alone under the load test conf, which is the production conf (no foreign origin allowed) with
// the tarpits off, with the users test/ClientFixtures.spec.js makes (plans/archive/ClientTestHarness.md 6, 7, 8). The
// load run and the tarpit case are acceptance tests (acceptance/client).
suite(["SecurityAccess", "SecurityHeaders", "SecurityCors", "SecurityAuthn", "SecurityScreens", "SecurityRest", "SecurityDevice"], {
    server: { components: ["framework"], specs: ["ClientFixtures"], conf: "conf/MoquiLoadTestConf.xml" },
})
