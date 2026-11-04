// m-markdown on the vendored Marked, in the page. Was js/WebrootVue.qvt.markdown.test.js.
// The marked UMD installs globalThis.marked. WebrootVue.qvt.js does not start the app on this page.
spec({ server: false, timeout: 60 })

browser.runInPage({
    scripts: ["/js/MoquiLib.js", "/js/WebrootVue.qvt.js", "/libs/marked/marked.umd.min.js"],
    test: "/files/runtime/base-component/webroot/test/client/pages/WebrootVue.qvt.markdown.page.js",
})

// From WebrootCode.spec.js, merged here to share a spec run.
// The code editor's helpers and the vendored CodeMirror bundle, in the page. Was js/WebrootCode.test.js.

browser.runInPage({
    scripts: ["/js/MoquiLib.js", "/libs/codemirror/codemirror.min.js", "/js/WebrootCode.js"],
    test: "/files/runtime/base-component/webroot/test/client/pages/WebrootCode.page.js",
})

// From WebrootVue.qvt.code.spec.js, merged here to share a spec run.
// m-code on the vendored highlight.js, in the page. Was js/WebrootVue.qvt.code.test.js.

browser.runInPage({
    scripts: ["/js/MoquiLib.js", "/js/WebrootVue.qvt.js", "/libs/highlightjs/highlight.min.js"],
    test: "/files/runtime/base-component/webroot/test/client/pages/WebrootVue.qvt.code.page.js",
})

// From WebrootVue.qvt.coedit.spec.js, merged here to share a spec run.
// m-coedit's provider on the vendored Tiptap bundle, in the page. Was js/WebrootVue.qvt.coedit.test.js.

browser.runInPage({
    scripts: ["/libs/tiptap/tiptap.min.js", "/js/MoquiLib.js", "/js/WebrootVue.qvt.js"],
    test: "/files/runtime/base-component/webroot/test/client/pages/WebrootVue.qvt.coedit.page.js",
})
