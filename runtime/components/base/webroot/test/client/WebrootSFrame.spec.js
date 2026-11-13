// SFrame (RFC 9605) on WebCrypto, in the page (js/WebrootSFrame.js; W8, docs/interfaces/WebRTC.md "Encrypted calls"):
// every test vector of the RFC's Appendix C (crates/moqui-rtc/testdata/sframe-rfc9605.json, which
// crates/moqui-rtc/tests/sframe.rs checks against the sframe crate), and the call's frame layout.
spec({ server: false, timeout: 60 })

browser.runInPage({
    scripts: ["/js/WebrootSFrame.js"],
    test: "/files/component/webroot/test/client/pages/WebrootSFrame.page.js",
})
