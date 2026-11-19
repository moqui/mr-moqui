// The framework's security proofs (moqui test-client framework --proofs), filed by OWASP ASVS 5.0.0 requirement:
// docs/security/proofs/README.md. With --serve they run against the server test/client runs against, the framework
// alone under the load test conf (the production conf with the tarpits off) with the users of test/ClientFixtures.spec.js;
// against any other server in the deployment profile. matrix is the responsibility matrix the run's summary
// (proofs.json, proofs.md) joins its results to, relative to this component's directory.
suite(["V6Authentication", "V7SessionManagement"], {
    server: { components: ["framework"], specs: ["ClientFixtures"], conf: "conf/MoquiLoadTestConf.xml" },
    matrix: "../../../../docs/security/proofs",
})
