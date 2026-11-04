#!/usr/bin/env python3
# This software is in the public domain under CC0 1.0 Universal plus a Grant of Patent License.
"""The mobile app's frame check (crates/moqui-mobile/patches/README.md), over the WebView's DevTools socket of a debug
build: the page shown has the IPC and Tauri's internals, and a frame of another origin it adds has neither, so it cannot
invoke a command. With an app path the shell first opens that page of the server (`moquiDesktop.openRemote`, an account
signed in), and the frames go into the server's page, the case that matters: the shell's own CSP refuses frames.
scripts/android-smoke.sh runs it; it needs `adb forward tcp:<port> localabstract:<the WebView's devtools socket>` done
first, and the websocket-client module.

    android-frames.py <port> [/qapps/...]
"""
import json
import sys
import time
import urllib.request

import websocket

port = sys.argv[1]
app_path = sys.argv[2] if len(sys.argv) > 2 else None
pages = json.load(urllib.request.urlopen(f"http://127.0.0.1:{port}/json"))
page = next((p for p in pages if p.get("type") == "page" and "moqui-" in p.get("url", "")), None)
if page is None:
    sys.exit(f"FAILED: no page of the app among {[p.get('url') for p in pages]}")
# no Origin header: the DevTools socket refuses one it was not started to allow
ws = websocket.create_connection(page["webSocketDebuggerUrl"], timeout=10, suppress_origin=True)
next_id = 0
# the frames' default contexts as they come, by frame
latest = {}


def note(message):
    if message.get("method") == "Runtime.executionContextCreated":
        context = message["params"]["context"]
        aux = context.get("auxData", {})
        if aux.get("isDefault") and aux.get("frameId") and aux.get("frameId") != page["id"]:
            latest[aux["frameId"]] = context["id"]


def call(method, **params):
    global next_id
    next_id += 1
    ws.send(json.dumps({"id": next_id, "method": method, "params": params}))
    while True:
        message = json.loads(ws.recv())
        note(message)
        if message.get("id") == next_id:
            return message


def evaluate(expression, context_id=None):
    params = {"expression": expression, "returnByValue": True, "awaitPromise": True}
    if context_id is not None:
        params["contextId"] = context_id
    answer = call("Runtime.evaluate", **params)
    return answer.get("result", {}).get("result", {}).get("value")


def settle(seconds):
    deadline = time.time() + seconds
    while time.time() < deadline:
        try:
            ws.settimeout(1)
            note(json.loads(ws.recv()))
        except websocket.WebSocketTimeoutException:
            continue
    ws.settimeout(10)


if app_path:
    opened = evaluate(f"moquiDesktop.openRemote({json.dumps(app_path)}).then(() => 'opened', (e) => String(e))")
    print(f"the shell opens {app_path}: {opened}")
    settle(8)
print(f"page: {evaluate('location.href')}")
PROBE = "[typeof window.ipc, typeof window.__TAURI_INTERNALS__, typeof window.moquiDesktop].join(',')"
main = evaluate(PROBE)
print(f"main frame: ipc, internals, moquiDesktop = {main}")
call("Runtime.enable")
# frames as a server page could have: a data: document (an opaque origin), the web's, and one of the page's own
# origin (it may have the IPC object, which takes nothing from a frame, but no adapter)
evaluate(
    "(() => { for (const src of ['data:text/html,<p>frame</p>', 'https://example.com/', '/favicon.ico']) {"
    " const f = document.createElement('iframe'); f.src = src; document.body.appendChild(f); } return 1; })()"
)
settle(10)
failed = False
foreign = 0
for frame_id, context_id in latest.items():
    href = evaluate("location.href", context_id)
    found = evaluate(PROBE, context_id)
    if href is None:
        continue
    print(f"frame at {href[:60]}: ipc, internals, moquiDesktop = {found}")
    own = "moqui-" in href
    if not own and not href.startswith("chrome-error:"):
        foreign += 1
    if (own and not str(found).endswith(",undefined")) or (not own and found != "undefined,undefined,undefined"):
        failed = True
if foreign == 0 and app_path:
    print("FAILED: no frame of another origin loaded")
    failed = True
elif foreign == 0:
    print("the shell's CSP refused the frames of other origins (give a server path to check a server page)")
if app_path:
    # the server page has no shell command, with or without a nonce it made up, and cannot go to the shell itself
    refused = evaluate(
        "window.__TAURI_INTERNALS__.invoke('client_state', {nonce: 'guess'}).then(() => 'answered', (e) => 'refused: ' + e)"
    )
    print(f"client_state from the server page: {refused}")
    if not str(refused).startswith("refused"):
        failed = True
    evaluate("location.href = 'http://moqui-shell.localhost/'; 1")
    settle(3)
    still = evaluate("location.origin")
    print(f"the server page's own navigation to the shell: still on {still}")
    if still != "http://moqui-app.localhost":
        failed = True
if main is None or not main.startswith("object,object"):
    print("FAILED: the main frame has no IPC")
    failed = True
print("FAILED: a frame can reach the IPC or the adapter" if failed else "ok: only the main frame has the IPC")
sys.exit(1 if failed else 0)
