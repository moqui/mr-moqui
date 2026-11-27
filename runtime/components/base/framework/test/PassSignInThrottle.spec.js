// PASS sign-in (start#Session, complete#Session of public/Account.js) is counted per requester and per user name, as
// registration, verification and recovery are (plans/archive/Security-Review-Medium.md SEC-PSA-01): past the user name's limit
// a sign-in is refused before any password is checked, and another user name is not held by it.
// Run: ./moq test framework --spec PassSignInThrottle
const ACCOUNT = "org.moqui.public.AccountServices."
const HELD = "pass.throttle.spec.held"
const FREE = "pass.throttle.spec.free"

function signIn(username) {
    try {
        ec.service.sync().name(ACCOUNT + "start#Session").parameters({input: {username, password: "Not-the-password-1"}}).call()
        return {said: ec.message.errorsString}
    } catch (err) {
        return {said: String(err.message || err)}
    } finally {
        ec.message.clearAll()
    }
}

feature("past ten sign-ins of one user name in ten minutes the next is refused, and another name is not", {
    when() {
        ec.user.logoutUser()
        // the user name's ten attempts of this window, counted as the throttle counts them
        for (let i = 0; i < 10; i++)
            ec.service.sync().name(ACCOUNT + "record#Attempt").parameters({input: {key: "session:address:" + HELD, limit: 10,
                windowMillis: 600000}}).requireNewTransaction(true).call()
        return {held: signIn(HELD), free: signIn(FREE)}
    },
    then({held, free}) {
        held.said.includes("Wait before signing in again")
        !free.said.includes("Wait before signing in again")
    }
})
