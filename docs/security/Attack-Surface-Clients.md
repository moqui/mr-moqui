# Attack surface: clients

The desktop and mobile apps and the other clients. This is one file of the attack surface map. The index, how to read a row, the defaults and the lock-down advice are in [Attack-Surface.md](Attack-Surface.md).

## The desktop app's own node

A node the desktop app runs in its own process may have no HTTP or cluster port (`moqui_host::start_embedded_with`, `crates/moqui-host/src/in_process.rs`). Its window's requests and sockets reach the router in process, through the same checks as a listener's, from the loopback address, as a sidecar window's do over loopback HTTP. The app keeps the node's session cookie for the window and sets `Host`, `Cookie` and the forwarded and edge link headers itself, never from the page (`crates/moqui-desktop/src/node/scheme.rs`). Nothing outside the app's process reaches that path.
