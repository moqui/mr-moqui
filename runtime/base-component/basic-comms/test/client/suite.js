// The order BasicComms's client specs run in (moqui test-client basic-comms), and the server they run against with
// --serve: basic-comms with its test conf (call media on a free loopback port) and the users of CallClientFixtures.
suite(["CallFlow"], {
    server: { components: ["basic-comms"], specs: ["CallClientFixtures"] },
})
