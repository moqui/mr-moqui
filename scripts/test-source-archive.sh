#!/usr/bin/env bash
# The CI runner needs its server during checkout and cache restore. Only the build container is offline.
set -euo pipefail
[ "$#" -eq 1 ] || { echo "usage: $0 mr-moqui-source.tar.gz" >&2; exit 1; }
root=$(cd "$(dirname "$0")/.." && pwd)
if command -v podman >/dev/null 2>&1; then engine=podman; else engine=docker; fi
# The ACME reachability test connects to the test node through this loopback name.
"$engine" run --rm -i --pull=never --network none --add-host check.localhost:127.0.0.1 --user 1000:1000 \
    --cap-drop ALL --cap-add SYS_CHROOT --security-opt no-new-privileges \
    --security-opt "seccomp=$root/podman/chromium-seccomp.json" \
    -e HOME=/workspace/home -e CARGO_HOME=/workspace/cargo-home -e CARGO_BUILD_JOBS=4 \
    -e CARGO_PROFILE_DEV_DEBUG=0 -e CARGO_PROFILE_TEST_DEBUG=0 -e CARGO_INCREMENTAL=0 \
    -e "MOQUI_CI_COMMIT=${MOQUI_CI_COMMIT:-}" --entrypoint /bin/bash "${MOQUI_BUILD_IMAGE:-localhost/moqui-build:latest}" -ec '
        unset MOQUI_HOME
        mkdir -p "$HOME" "$CARGO_HOME" /workspace/check
        cd /workspace/check
        tar -xzf -
        cd mr-moqui
        if [ -n "$MOQUI_CI_COMMIT" ]; then grep -Fx "$MOQUI_CI_COMMIT ." vendor/revisions.txt; fi
        ./moq build --offline --release
        # The workspace test links several copies of the maximal host. Link one at a time on an 8 GiB runner.
        CARGO_BUILD_JOBS=1 RUST_TEST_THREADS=1 ./moq test-framework
    ' < "$1"
