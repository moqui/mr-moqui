// The acceptance walks of BasicComms (./moq accept basic-comms): fake-media calls and an encrypted room, run against
// basic-comms with its test conf (call media on a free loopback port) and the users and room of test/CallClientFixtures
// and test/CryptoClientFixtures.
suite(["CallFlow", "CallGuestFlow", "EncryptedCallFlow", "EncryptedChatFlow"], {
    server: { components: ["basic-comms"], specs: ["CallClientFixtures", "CryptoClientFixtures"] },
})
