#!/usr/bin/env bash
# Build the images from the Maximal package: the server's, localhost/moqui:latest, and the one pipeline jobs run in,
# localhost/moqui-build:latest (Containerfile.build; docs/interfaces/Pipelines.md "Images").
#   podman/build.sh                 ./moq build --release (Maximal with the dev tools), then the server image; for a
#                                   production image build Maximal without them first and use --nobuild
#                                   (docs/Run-Deploy.md, Production checklist)
#   podman/build.sh --nobuild       images only, using the existing target/release/moqui
#   podman/build.sh --in-build-image   the binary built in localhost/moqui-build (Debian 13) into target/build-image,
#                                   for a host whose glibc is newer than the images' (Ubuntu 26.04: 2.43, Debian 13:
#                                   2.41), where a binary built on the host does not start in them
#   podman/build.sh --build-image   the moqui-build image too
#   podman/build.sh --only-build-image   the moqui-build image and not the server's
#   podman/build.sh --chromium --java    with --build-image: Chromium (PDF, the browser tests) and a Java runtime
#                                        (OpenSearch, which ./moq opensearch-download gets) in moqui-build
# With no podman on PATH, docker is used.
set -euo pipefail

root=$(cd "$(dirname "$0")/.." && pwd)
nobuild=0
in_build_image=0
server_image=1
build_image=0
chromium=0
java=0
for arg in "$@"; do
    case "$arg" in
        --nobuild) nobuild=1 ;;
        --in-build-image) in_build_image=1 ;;
        --build-image) build_image=1 ;;
        --only-build-image) build_image=1; server_image=0 ;;
        --chromium) chromium=1 ;;
        --java) java=1 ;;
        *)
            echo "Unknown argument: $arg" >&2
            echo "Usage: podman/build.sh [--nobuild | --in-build-image] [--build-image | --only-build-image] [--chromium] [--java]" >&2
            exit 1
            ;;
    esac
done

if command -v podman >/dev/null 2>&1; then
    engine=podman
    # docker format keeps the image HEALTHCHECK. Podman's default OCI format drops it.
    format=(--format docker)
elif command -v docker >/dev/null 2>&1; then
    engine=docker
    format=()
else
    echo "neither podman nor docker is on PATH" >&2
    exit 1
fi
binary=$root/target/release/moqui
if [ "$in_build_image" -eq 1 ]; then
    # ./moq build --release in the build image, on this tree, with a target directory of its own (the host's target/
    # stays the host's), the host's cargo registry and Postgres archive cache shared; as the container's root, which
    # rootless Podman maps to this user, so what it writes is this user's
    "$engine" image inspect localhost/moqui-build:latest >/dev/null 2>&1 || {
        echo "no localhost/moqui-build:latest: podman/build.sh --only-build-image first (on a host it runs on), or pull one" >&2
        exit 1
    }
    mkdir -p "$root/target/build-image" "$HOME/.cargo/registry" "$HOME/.theseus"
    "$engine" run --rm -u 0 -v "$root:/src" -v "$root/target/build-image:/src/target" \
        -v "$HOME/.cargo/registry:/workspace/.cargo/registry" -v "$HOME/.theseus:/workspace/home/.theseus" \
        -w /src localhost/moqui-build:latest ./moq build --release
    binary=$root/target/build-image/release/moqui
elif [ "$nobuild" -eq 0 ]; then
    (cd "$root" && ./moq build --release)
fi
# The vault's browser module, which both images carry under components/base and git does not keep: ./moq build made it;
# with --nobuild it is built here when it is missing or stale.
"$root/moq" build-js || exit 1
"$root/scripts/vault-wasm.sh" --quiet || {
    echo "could not build the vault's browser module (scripts/vault-wasm.sh)" >&2
    exit 1
}

if [ ! -x "$binary" ]; then
    echo "No release binary at $binary. Run ./moq build --release, or drop --nobuild." >&2
    exit 1
fi
# The binary must start on the images' base: one built on a host with a newer glibc than it fails there with "version
# `GLIBC_2.xx' not found", and the images would be made anyway. Asked of the base itself, which also finds a library it
# lacks.
base=$(sed -n 's/^FROM \([^ ]*\).*/\1/p' "$root/podman/Containerfile" | head -1)
if ! started=$("$engine" run --rm -v "$binary:/moqui:ro" "$base" /moqui version 2>&1); then
    echo "$binary does not run on the images' base $base:" >&2
    printf '%s\n' "$started" | sed 's/^/    /' | head -5 >&2
    echo "Build it there: podman/build.sh --in-build-image (it needs localhost/moqui-build)." >&2
    exit 1
fi

copy_tree() {
    local src=$1 dest=$2
    mkdir -p "$dest"
    tar -C "$src" --exclude=.git --exclude=target --exclude=node_modules -cf - . | tar -C "$dest" -xf -
}

stage=$(mktemp -d)
trap 'rm -rf "$stage"' EXIT

if [ "$server_image" -eq 1 ]; then
    server=$stage/server
    mkdir -p "$server"
    cp -a "$binary" "$server/moqui"
    chmod 755 "$server/moqui"
    cp -a "$root/podman/entrypoint.sh" "$server/entrypoint.sh"
    chmod 755 "$server/entrypoint.sh"
    cp -a "$root/LICENSE.md" "$server/LICENSE.md"
    cp -a "$root/runtime/conf/MoquiProductionConf.xml" "$server/MoquiConf.xml"
    mkdir -p "$server/conf"
    cp -a "$root/runtime/conf/." "$server/conf/"
    mkdir -p "$server/components/addon"
    cp -a "$root/runtime/components/components.xml" "$server/components/components.xml"
    copy_tree "$root/runtime/components/base" "$server/components/base"
    if [ -d "$root/runtime/components/addon" ] && [ -n "$(ls -A "$root/runtime/components/addon")" ]; then
        copy_tree "$root/runtime/components/addon" "$server/components/addon"
    fi
    "$engine" build "${format[@]}" -f "$root/podman/Containerfile" -t localhost/moqui:latest "$server"
    echo "Built localhost/moqui:latest"
fi

if [ "$build_image" -eq 1 ]; then
    build=$stage/build
    mkdir -p "$build/tree"
    cp -a "$binary" "$build/moqui"
    chmod 755 "$build/moqui"
    # The tree the built-in pipeline of a component tests in: what is tracked, with no add-on components (a job gets
    # the ones its component depends on), no databases, logs or build output.
    (cd "$root" && git archive --format=tar HEAD) | tar -C "$build/tree" -xf -
    # with the vault's browser module and its stamp, which are not tracked, so ./moq test there finds it up to date
    libs=runtime/components/base/webroot/screen/webroot/libs
    cp -a "$root/$libs/." "$build/tree/$libs/"
    # Reuse the staged selection when a component job verifies the read-only /opt/moqui tree.
    perl -MJSON::PP -0777 -ne 'print join(" ", @{decode_json($_)}), "\n"' \
        "$root/$libs/fonts-extra/installed.json" >"$build/tree/runtime/components/base/webroot/build/fonts/selected.txt"
    vault=runtime/components/base/webroot/screen/webroot/js/vault
    cp -a "$root/$vault/moqui_vault.wasm" "$root/$vault/moqui_vault.wasm.json" "$build/tree/$vault/"
    rust_version=$(sed -n 's/^channel *= *"\(.*\)"$/\1/p' "$root/rust-toolchain.toml")
    if [ -z "$rust_version" ]; then
        echo "rust-toolchain.toml names no channel" >&2
        exit 1
    fi
    version=$("$binary" version | sed -n 's/^moqui \([^ ]*\).*/\1/p')
    "$engine" build "${format[@]}" -f "$root/podman/Containerfile.build" \
        --build-arg "RUST_VERSION=$rust_version" \
        --build-arg "WITH_CHROMIUM=$chromium" \
        --build-arg "WITH_JAVA=$java" \
        -t localhost/moqui-build:latest ${version:+-t "localhost/moqui-build:$version"} "$build"
    echo "Built localhost/moqui-build:latest${version:+ and localhost/moqui-build:$version} (Rust $rust_version)"
fi
