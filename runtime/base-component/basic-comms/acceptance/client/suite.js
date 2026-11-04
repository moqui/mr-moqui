// The acceptance walks of BasicComms (./moq accept basic-comms): fake-media calls, run against basic-comms with its
// test conf (call media on a free loopback port) and the users of test/CallClientFixtures.
suite(["CallFlow", "CallGuestFlow", "EncryptedCallFlow"], {
    server: { components: ["basic-comms"], specs: ["CallClientFixtures"] },
})
