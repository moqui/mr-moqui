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
