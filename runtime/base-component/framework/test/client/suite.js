// The order the framework's client specs run in (moqui test-client framework), and the server they run against with
// --serve: the framework alone under the load test conf, which is the production conf (no foreign origin allowed) with
// the screen, transition and service tarpits off so LoadBasic is not answered 429, with the users
// test/ClientFixtures.spec.js makes (plans/archive/ClientTestHarness.md 6, 7, 8). SecurityTarpit is listed so a server with
// the tarpits on runs it; under this conf tarpit-enabled is false and those two features skip.
suite(["SecurityAccess", "SecurityHeaders", "SecurityCors", "SecurityAuthn", "SecurityErrors", "SecurityWebSocket",
    "SecurityScreens", "SecurityStatus", "SecurityProxy", "SecurityRest", "SecurityMessages", "SecurityAuthnMore",
    "SecurityMisc", "SecurityTarpit", "LoadBasic"], {
    server: { components: ["framework"], specs: ["ClientFixtures"], conf: "conf/MoquiLoadTestConf.xml" },
})
