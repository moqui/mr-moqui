// The users BasicComms's client specs (test/client, moqui test-client basic-comms) call each other as: john.doe and
// calls.client.b, with a password of its own. moqui test-client basic-comms --serve runs this spec on its server's
// scratch database; for another server run it on that server's database:
//     moqui test basic-comms --spec CallClientFixtures --db existing
// The row is made only when it is missing. Not demo data, so no production load reaches it.
const USERNAME = "calls.client.b"
const PASSWORD = "CallsClient1!!"

function sys(run) {
    const wasDisabled = ec.artifactExecution.disableAuthz()
    try {
        return run()
    } finally {
        if (!wasDisabled) ec.artifactExecution.enableAuthz()
    }
}

feature("the client specs' callee exists with its password", {
    when() {
        const existing = sys(() => ec.entity.find("moqui.security.UserAccount").condition("username", USERNAME).one())
        const userId = existing != null ? existing.userId : sys(() => call("org.moqui.impl.UserServices.create#UserAccount",
            {username: USERNAME, newPassword: PASSWORD, newPasswordVerify: PASSWORD, userFullName: "Calls Client B"})).userId
        return {userId}
    },
    then({userId}) {
        userId != null
    }
})
