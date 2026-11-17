// The acceptance walks of BasicComms (./moq accept basic-comms): fake-media calls and an encrypted room, run against
// basic-comms with its test conf (call media on a free loopback port) and the users and room of test/CallClientFixtures
// and test/CryptoClientFixtures; the phone's chat in the bundled shell (PhoneChat, and reactions, who reacted and Info by
// touch, PhoneReactions; the details tabs and a voice note at both widths, ChatDetails; the palette, Activity, Threads, the
// groups and the mail line, ChatDesktop; offline: server: false). ChatPalette is the palette's walk on the web. CallDirect
// is the direct path (peer to peer); CallNetwork, the calls on shaped networks, runs only under sudo
// scripts/call-network.sh and skips here, saying why.
suite(["CallFlow", "CallGuestFlow", "ChatLabels", "EncryptedCallFlow", "EncryptedChatFlow", "PhoneChat", "PhoneReactions", "ChatDetails", "ChatDesktop",
    "ChatPalette", "CallDirect", "CallNetwork"], {
    server: { components: ["basic-comms"], specs: ["CallClientFixtures", "CryptoClientFixtures"] },
})
