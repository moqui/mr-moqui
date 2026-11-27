#!/usr/bin/env bash
# Clippy, with warnings denied, on every feature set a build uses (docs/Build-Test.md), so a feature gate that only one
# package sees still compiles: the workspace on its defaults (the Maximal package with the dev tools, `cargo lint`), the
# Lite, Maximal (with and without the dev tools) and Enterprise packages of the binary, the host with no features (`cargo lint-min`) and with only
# the terminal client (the client-only build of `moqui tui --server`) and with only the SMTP listener (which shares the
# SSH server's request worker handoff and must build without SSH) and with only phone calls (sip, with the call media it
# joins) and with only XMPP (xmpp, without the call media), the vault crate natively and as its wasm module (whose
# tree must hold no getrandom), and the framework with none (what the browser build links); the entity crate alone (without the test-support the workspace's tests turn on); the report crate without
# xlsx (as the browser links it); the dev kit without its file watcher (as Lite links it); the Tools crate alone (on
# moqui-core with only the features it asks for); the Overture crate alone (its tests, which no host feature set builds); the drivers with no
# database client, with their tests; the mail crate without imap (the send half alone, as Lite links it); the
# terminal client on its own feature sets (the model alone is what moqui-core's tests link); the lib crate with only its
# container driver and its fake engine (docker-fake), with its tests and opt-in test; then the scripted TUI
# screens on embedded Postgres, which `cargo test` leaves out.
#   scripts/check-packages.sh [extra clippy args...]
set -euo pipefail
cd "$(dirname "$0")/.."
run() {
    echo "== cargo clippy $*" >&2
    cargo clippy "$@" -- -D warnings
}
run --workspace --all-targets "$@"
# The Dev package, commented out: the release build (Maximal with the dev tools) is the default now.
# run -p moqui --no-default-features --features dev --all-targets "$@"
run -p moqui --no-default-features --features maximal,devtools --all-targets "$@"
run -p moqui --no-default-features --features lite --all-targets "$@"
run -p moqui --no-default-features --features maximal --all-targets "$@"
run -p moqui --no-default-features --features enterprise --all-targets "$@"
run -p moqui-host --no-default-features --all-targets "$@"
run -p moqui-host --no-default-features --features tui --all-targets "$@"
run -p moqui-host --no-default-features --features inbound-smtp --all-targets "$@"
run -p moqui-host --no-default-features --features sip --all-targets "$@"
run -p moqui-host --no-default-features --features xmpp --all-targets "$@"
run -p moqui-host --no-default-features --features vault --all-targets "$@"
run -p moqui-host --no-default-features --features instances --all-targets "$@"
run -p moqui-host --no-default-features --features manage --all-targets "$@"
run -p moqui-vault --no-default-features --all-targets "$@"
run -p moqui-vault --no-default-features --features wasm --lib --target wasm32-unknown-unknown "$@"
# The vault's browser module takes its randomness from the page: no getrandom and no wasm-bindgen in its tree
# (scripts/vault-wasm.sh refuses to build one that has either).
echo "== cargo tree -p moqui-vault --target wasm32-unknown-unknown --features wasm: no getrandom or wasm-bindgen" >&2
vault_tree=$(cargo tree -p moqui-vault --target wasm32-unknown-unknown --no-default-features --features wasm -e normal \
    --prefix none --format '{p}')
if grep -E '^(getrandom|wasm-bindgen) ' <<<"$vault_tree"; then
    echo "the vault's wasm module links the crates above; it must take its randomness from the page" >&2
    exit 1
fi
run -p moqui-core --no-default-features --all-targets "$@"
run -p moqui-entity --all-targets "$@"
run -p moqui-report --no-default-features --all-targets "$@"
run -p moqui-devkit --all-targets "$@"
run -p moqui-tools --all-targets "$@"
run -p moqui-maps --all-targets "$@"
run -p moqui-drivers --all-targets "$@"
run -p moqui-mail --no-default-features --all-targets "$@"
run -p moqui-tui --no-default-features --features model --all-targets "$@"
run -p moqui-lib --features docker-fake --all-targets "$@"
echo "== cargo test -p moqui-host --test tui_screens" >&2
cargo test -p moqui-host --test tui_screens
echo "All package feature sets pass" >&2
