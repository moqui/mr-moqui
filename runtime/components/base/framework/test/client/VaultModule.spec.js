/* This software is in the public domain under CC0 1.0 Universal plus a Grant of Patent License. */
// The vault module (crates/moqui-vault built to js/vault/moqui_vault.wasm, plans/Vault-Instances.md V0) in its worker
// in the test browser: the known-answer vectors are the bytes the native crate makes (testdata/vectors.json), and an
// item seals, opens and is forgotten on lock in the worker. No server: the page opens at https://moqui.test.
spec({ server: false, timeout: 60 })

browser.runInPage({
    scripts: ["/js/vault/VaultClient.js"],
    test: "/files/component/framework/test/client/pages/VaultModule.page.js",
})
