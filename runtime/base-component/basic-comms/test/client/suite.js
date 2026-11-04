// The order BasicComms's client specs run in (moqui test-client basic-comms), and the server they run against with
// --serve: basic-comms with its test conf and the users of CallClientFixtures and the room of CryptoClientFixtures.
// The call walks are acceptance tests (acceptance/client).
suite(["CryptoFlow", "EncryptedChatFlow"], {
    server: { components: ["basic-comms"], specs: ["CallClientFixtures", "CryptoClientFixtures"] },
})
