// The order the client specs run in (crates/moqui-host/tests/it/test_client.rs), and the server they run against with
// --serve: this component's Basics spec makes the fixtures (crates/moqui/tests/cli/test_client.rs).
suite(["Http", "Sessions", "WebSocket", "Ssh", "Load", "Skipped", "Served"], {
    server: { components: ["moqui-test-fixture"], specs: ["Basics"] },
})
