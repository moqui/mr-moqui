// The webroot's acceptance client specs (./moq accept webroot): ThemeLayout's offline pages at https://moqui.test, no
// server; WebrootTabCapture, a media test of the call's real tab capture (its own Chromium with capture flags); and
// TerminalWalk on a server under the dev conf (a dev instance, so the terminal offers shells on the server).
suite(["ThemeLayout", "WebrootTabCapture", "TerminalWalk"], {
    server: { components: ["webroot"], conf: "conf/MoquiDevConf.xml" },
})
