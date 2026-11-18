// Runs after Basics, which it names in dependsOn, so `--spec Dependent` runs Basics first.
spec({
    dependsOn: ["Basics"],
    setup() { ec.artifactExecution.disableAuthz() },
    cleanup() { ec.artifactExecution.enableAuthz() },
})

feature("data from the spec it depends on is there", {
    expect() {
        ec.entity.find("moqui.basic.Enumeration").condition("enumTypeId", "FixtureType").count() == 5
        ec.user.nowTimestamp != "1265198400000"
    },
})

feature("the component's test/MoquiConf.xml merges in a test run", {
    expect() {
        ec.factory.getConfValue("moqui-test-fixture@test-conf") == "test"
    },
})
