#!/usr/bin/env bash
# Build the images from the Maximal package: the server's, localhost/moqui:latest, and the one pipeline jobs run in,
# localhost/moqui-build:latest (Containerfile.build; docs/interfaces/Pipelines.md "Images").
#   podman/build.sh                 ./moq build --release, then the server image
#   podman/build.sh --nobuild       images only, using the existing target/release/moqui
#   podman/build.sh --build-image   the moqui-build image too
#   podman/build.sh --only-build-image   the moqui-build image and not the server's
#   podman/build.sh --chromium --java    with --build-image: Chromium (PDF, the browser tests) and a Java runtime
#                                        (OpenSearch, which ./moq opensearch-download gets) in moqui-build
# With no podman on PATH, docker is used.
set -euo pipefail

root=$(cd "$(dirname "$0")/.." && pwd)
nobuild=0
server_image=1
build_image=0
chromium=0
java=0
for arg in "$@"; do
    case "$arg" in
        --nobuild) nobuild=1 ;;
        --build-image) build_image=1 ;;
        --only-build-image) build_image=1; server_image=0 ;;
        --chromium) chromium=1 ;;
        --java) java=1 ;;
        *)
            echo "Unknown argument: $arg" >&2
            echo "Usage: podman/build.sh [--nobuild] [--build-image | --only-build-image] [--chromium] [--java]" >&2
            exit 1
            ;;
    esac
done

if [ "$nobuild" -eq 0 ]; then
    (cd "$root" && ./moq build --release)
fi

binary=$root/target/release/moqui
if [ ! -x "$binary" ]; then
    echo "No release binary at $binary. Run ./moq build --release, or drop --nobuild." >&2
    exit 1
fi
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
