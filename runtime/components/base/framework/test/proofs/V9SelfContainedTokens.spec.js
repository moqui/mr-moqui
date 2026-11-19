// OWASP ASVS 5.0.0 V9 Self-contained Tokens: the framework's proofs, each feature filed under the requirement its name
// starts with (docs/security/proofs/V9-Self-contained-Tokens.md). Ported from Moqui Java's pytest proofs; a comment names
// the Python test, as in test/client.
//
// A system message posted to /rest/sm carries an HMAC of its body, checked before its content is read. The remotes
// SEC_SMR_HMAC (an HMAC) and SEC_SMR_HMAC_TS (an HMAC with a timestamp) and their secret are rows that
// test/ClientFixtures.spec.js makes, so these skip in the deployment profile: without the remote every post gets the same
// refusal and would pass for the wrong reason. The session cookie's MAC and the LFS, MCP and SSO tokens are proved
// in-process.

const MESSAGE_TYPE = "SEC_SMT_TEST"
const REMOTE_HMAC = "SEC_SMR_HMAC"
const REMOTE_TS = "SEC_SMR_HMAC_TS"
const SECRET = "sec-hmac-test-secret"
const HEADER = "X-Moqui-Signature"
const BODY = '{"probe":true}'
// The public text of a refusal, the same for a missing, a bad and a late signature.
const REFUSED = "system message not accepted"
const postMessage = (remote, body, signature) => {
    const headers = { "Content-Type": "application/json" }
    if (signature != null) headers[HEADER] = signature
    return session().post(`/rest/sm/${MESSAGE_TYPE}/${remote}`, { body, headers, csrf: false })
}
const needsRemotes = () => needsFixtures("the system message remotes SEC_SMR_HMAC and SEC_SMR_HMAC_TS and their secret")

// test_api_rest.py: test_system_message_hmac_missing_header_is_403
// Does not prove: any endpoint but /rest/sm.
feature("v5.0.0-9.1.1: a system message with no HMAC signature header is refused", {
    given() { needsRemotes() },
    when() { return { res: postMessage(REMOTE_HMAC, BODY) } },
    then({ res }) {
        res.status == 403
        res.text.toLowerCase().includes(REFUSED)
    },
})

// test_api_rest.py: test_system_message_hmac_bad_signature_is_403, test_system_message_hmac_valid_does_not_require_login
// Does not prove: any endpoint but /rest/sm.
feature("v5.0.0-9.1.1: a system message with a bad HMAC is refused, one with a good HMAC accepted", {
    given() { needsRemotes() },
    when() {
        return {
            bad: postMessage(REMOTE_HMAC, BODY, "not-a-valid-hmac"),
            good: postMessage(REMOTE_HMAC, BODY, hmac.sha256Base64(SECRET, BODY)),
        }
    },
    then({ bad, good }) {
        bad.status == 403
        bad.text.toLowerCase().includes(REFUSED)
        // the control: the right HMAC is accepted, with no sign-in
        good.status == 200
        !good.text.toLowerCase().includes("hmac verify failed")
        good.text.toLowerCase().includes("systemmessageidlist")
    },
})

// test_api_rest.py: test_system_message_hmac_timestamp_outside_window_is_403, test_system_message_hmac_timestamp_in_window_is_accepted
// Does not prove: any endpoint but /rest/sm; the token windows of LFS, MCP and SSO are proved in-process.
feature("v5.0.0-9.2.1: a system message timestamped outside its window is refused, one inside accepted", {
    given() { needsRemotes() },
    when() {
        // 400 seconds ago, as in the Python proof: the receive window is five minutes
        const late = Math.floor(Date.now() / 1000) - 400
        const now = Math.floor(Date.now() / 1000)
        return {
            outside: postMessage(REMOTE_TS, BODY, hmac.sha256Timestamp(SECRET, BODY, late)),
            inside: postMessage(REMOTE_TS, BODY, hmac.sha256Timestamp(SECRET, BODY, now)),
        }
    },
    then({ outside, inside }) {
        outside.status == 403
        outside.text.toLowerCase().includes(REFUSED)
        // the control: a remote that refused every timestamp would pass the refusal alone
        inside.status == 200
        inside.text.toLowerCase().includes("systemmessageidlist")
    },
})
