// The users BasicComms's client specs (test/client, moqui test-client basic-comms) call each other as: john.doe,
// calls.client.b and calls.client.c, with a password of their own. moqui test-client basic-comms --serve runs this spec on its server's
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

function ensureUser(username, fullName) {
    const existing = sys(() => ec.entity.find("moqui.security.UserAccount").condition("username", username).one())
    return existing != null ? existing.userId : sys(() => call("org.moqui.impl.UserServices.create#UserAccount",
        {username, newPassword: PASSWORD, newPasswordVerify: PASSWORD, userFullName: fullName})).userId
}

feature("the client specs' callees exist with their password", {
    when() {
        // calls.client.c is the third person of EncryptedCallFlow's call (W8)
        return {userId: ensureUser(USERNAME, "Calls Client B"), thirdId: ensureUser("calls.client.c", "Calls Client C")}
    },
    then({userId, thirdId}) {
        userId != null
        thirdId != null
    }
})
