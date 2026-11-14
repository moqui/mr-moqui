/* This software is in the public domain under CC0 1.0 Universal plus a Grant of Patent License. */
// The semantic find of a page (js/MoquiFind.js): sentences and their exact ranges, the word find with no model, and the
// bar over a server that ranks (a stub of /find/rank).
spec({ server: false, timeout: 60 })
browser.runInPage({
    scripts: ["/js/MoquiFind.js"],
    test: "/files/component/webroot/test/client/pages/MoquiFind.page.js",
})
