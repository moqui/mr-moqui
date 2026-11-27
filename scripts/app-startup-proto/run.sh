#!/usr/bin/env bash
# Usage: scripts/app-startup-proto/run.sh LIBS_DIR
#   LIBS_DIR: a built webroot libs directory (vue/vue.global.prod.js, quasar/quasar.umd.prod.js, quasar/quasar.prod.css).
#   Needs Node and Playwright (NODE_PATH to its node_modules); CHROME may name a Chromium binary.
# Takes about 8 minutes. Not a test: a measurement, run when asked (plans/App-Startup.md, Quick test).
set -euo pipefail
here=$(cd "$(dirname "$0")" && pwd)
libs=${1:?LIBS_DIR}
work=$(mktemp -d "${XDG_CACHE_HOME:-$HOME/.cache}/app-startup-proto.XXXXXX")
trap 'kill "$server" 2>/dev/null || true; rm -rf "$work"' EXIT
cp -r "$here/site" "$here/server.py" "$work/"
mkdir -p "$work/site/libs"
cp "$libs/vue/vue.global.prod.js" "$libs/quasar/quasar.umd.prod.js" "$libs/quasar/quasar.prod.css" "$work/site/libs/"
port=${PORT:-18777}
python3 "$work/server.py" "$port" &
server=$!
PROFILES=$work node "$here/run.js" "$port"
