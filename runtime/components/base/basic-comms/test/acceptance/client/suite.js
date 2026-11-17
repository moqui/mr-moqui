// The acceptance walks of BasicComms (./moq accept basic-comms): fake-media calls and an encrypted room, run against
// basic-comms with its test conf (call media on a free loopback port) and the users and room of test/CallClientFixtures
// and test/CryptoClientFixtures; and the phone's chat in the bundled shell (PhoneChat, offline: server: false).
suite(["CallFlow", "CallGuestFlow", "ChatLabels", "EncryptedCallFlow", "EncryptedChatFlow", "PhoneChat"], {
    server: { components: ["basic-comms"], specs: ["CallClientFixtures", "CryptoClientFixtures"] },
})
