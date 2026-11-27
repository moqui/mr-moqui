# app-startup-proto

A prototype that measures the startup steps of [plans/App-Startup.md](../../plans/App-Startup.md) in headless
Chromium. It is a measurement, not a test: run it when asked.

| Variant | Page |
|---|---|
| `a` | Now: the page and its data from the network, the shell libraries blocking. |
| `b` | S2 and S3: the page from the service worker; an inline script draws the last data from `localStorage`. |
| `d` | The service worker keeps the page with the last content inside it. |
| `e` | As `d`, the first screen only, with no shell libraries. |
| `f` | As `b`, with no service worker: the page from the HTTP cache. |

```sh
NODE_PATH=<playwright node_modules> scripts/app-startup-proto/run.sh <webroot libs dir>
```

It prints, per variant and CPU slowdown (1 and 4 times), the median / p90 in ms from navigation start of the first
content paint on a cold start and on a reload, and of the fresh data.

## Android

[android/](android/) is a small app with no Gradle that measures cold and warm starts on a device or an emulator. It
loads variant `b` from the same server in a WebView. The extra `mode` selects what it draws:

| Mode | First frame |
|---|---|
| `native` | A native list of 15 rows: the floor for any app on the device. |
| `web` | The WebView only. Content shows when the page has painted. |
| `s7` | S7: the last snapshot as a native list. The WebView loads behind it and replaces it when it has painted. |

With `SYNC=true` the app makes the WebView before its first frame, and else after it.

```sh
JAVA_HOME=<a JDK> scripts/app-startup-proto/android/build.sh   # needs build-tools 35 and android-35
adb install -r ~/.cache/app-startup-apk/startproto.apk
python3 <work dir>/server.py 18777 &   # the site and server.py as run.sh copies them
adb reverse tcp:18777 tcp:18777
scripts/app-startup-proto/android/measure.sh s7 10
```

It prints `TotalTime` (first frame) and `Fully drawn` (WebView content) per cold start, and `WaitTime` per warm start.
