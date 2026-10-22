#!/usr/bin/env bash
# Clippy, with warnings denied, on every feature set a build uses (docs/Packaging.md), so a feature gate that only one
# package sees still compiles: the workspace on its defaults (the Maximal package, `cargo lint`), the Default and
# Enterprise packages of the binary, the host with no features (`cargo lint-min`) and with only the terminal client (the
# client-only build of `moqui tui --server`), and the framework with none (what
# the browser build links); the terminal client on its own feature sets (the model alone is what moqui-core's tests
# link); then the scripted TUI screens on embedded Postgres, which `cargo test` leaves out.
#   scripts/check-packages.sh [extra clippy args...]
set -euo pipefail
cd "$(dirname "$0")/.."
run() {
    echo "== cargo clippy $*" >&2
    cargo clippy "$@" -- -D warnings
}
run --workspace --all-targets "$@"
run -p moqui --no-default-features --features lite --all-targets "$@"
run -p moqui --no-default-features --features enterprise --all-targets "$@"
run -p moqui-host --no-default-features --all-targets "$@"
run -p moqui-host --no-default-features --features tui --all-targets "$@"
run -p moqui-core --no-default-features --all-targets "$@"
run -p moqui-tui --no-default-features --features model --all-targets "$@"
echo "== cargo test -p moqui-host --test tui_screens" >&2
cargo test -p moqui-host --test tui_screens
echo "All package feature sets pass" >&2
