// The call store's rules of a call's flow, in the page (js/WebrootCall.js; plans/Comms-Messaging.md §8.3 items 1-7): the side
// panel or the call screen, the voice-to-video prompt, Move to this device, and a declined call's quick text. No media,
// no server: the calls in two browsers are the acceptance walks (basic-comms test/acceptance/client/CallFlow).
spec({ server: false, timeout: 60 })

browser.runInPage({
    scripts: ["/js/MoquiLib.js", "/js/MoquiL10n.js", "/js/WebrootFloats.js", "/js/WebrootCallKeys.js", "/js/WebrootCall.js"],
    test: "/files/component/webroot/test/client/pages/WebrootCall.page.js",
})
