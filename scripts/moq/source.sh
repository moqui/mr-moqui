# Source archives (a release one at a tag) and offline builds. Keep source replacement local to an extracted archive.
enable_offline_build() {
    export CARGO_NET_OFFLINE=true npm_config_offline=true npm_config_update_notifier=false
    if [ -f "$ROOT/vendor/cargo-config.toml" ]; then
        local home_dir=$ROOT/target/offline-cargo-home config
        mkdir -p "$home_dir" || die "could not prepare the offline Cargo home"
        config=$(cat "$ROOT/vendor/cargo-config.toml") || return 1
        # An env var in a TOML path is literal. A relative path in this CARGO_HOME config
        # resolves against its parent, target/.
        write_if_changed "$home_dir/config.toml" "$config"
        export CARGO_HOME="$home_dir"
        export npm_config_cache="$ROOT/vendor/npm"
        local pg
        pg=$("$ROOT/scripts/pg-payload.sh" path) || return 1
        pg=$ROOT/vendor/postgres/${pg##*/}
        [ -f "$pg.sha256" ] || die "source release has no Postgres payload for ${pg##*/}"
        (cd "$ROOT/vendor/postgres" && sha256_check "${pg##*/}.sha256") || die "Postgres checksum failed"
        export MOQUI_PG_ARCHIVE="$pg"
    fi
}

sha256_check() {
    if command -v sha256sum >/dev/null 2>&1; then sha256sum -c "$1"; else shasum -a 256 -c "$1"; fi
}

source_export() {
    local repo=$1 dest=$2
    [ -z "$(git -C "$repo" status --porcelain)" ] || die "source-archive needs a clean checkout: $repo"
    mkdir -p "$dest" || return 1
    git -C "$repo" archive HEAD | tar -xf - -C "$dest" || return 1
    printf '%s %s\n' "$(git -C "$repo" rev-parse HEAD)" "$(rel "$repo")" >>"$SOURCE_REVISIONS"
}

task_source_archive() (
    set -e
    # BSD tar otherwise stores AppleDouble files for macOS extended attributes.
    export COPYFILE_DISABLE=1
    [ $# -le 1 ] || die "source-archive takes an optional output file"
    local label out temp tree repo target payload app
    local -a sync addons
    # The label names the archive in SOURCE-RELEASE.txt: the tag at HEAD for a release, else the commit. The clean
    # checkouts and vendor/revisions.txt are what make the archive reproducible, with or without a tag.
    if label=$(git describe --tags --exact-match HEAD 2>/dev/null); then
        :
    elif [ "${MOQUI_CI_EVENT:-}" = tag ] && [ "${MOQUI_CI_COMMIT:-}" = "$(git rev-parse HEAD)" ]; then
        # The CI runner can have a shallow detached checkout of the tagged commit.
        label=${MOQUI_CI_REF#refs/tags/}
    else
        label="commit $(git rev-parse --short=12 HEAD)" || die "source-archive needs a git checkout"
    fi
    out=$(abs_path "${1:-mr-moqui-source.tar.gz}")
    [ ! -e "$out" ] || die "source archive exists: $out"
    [ -d "$(dirname "$out")" ] || die "output directory does not exist"
    dry_run && { info "Archive $label with locked npm, cargo and Postgres inputs in $out"; return; }
    js_node
    mkdir -p "$ROOT/target"
    temp=$(mktemp -d "$ROOT/target/source-archive.XXXXXX")
    local part=

    trap 'rm -rf -- "$temp"; [ -z "$part" ] || rm -f -- "$part"' EXIT
    tree=$temp/mr-moqui
    mkdir -p "$tree/vendor"
    SOURCE_REVISIONS=$tree/vendor/revisions.txt
    source_export "$ROOT" "$tree"
    for repo in "$RUNTIME_DIR"/components/*/*; do
        [ -e "$repo/.git" ] || continue
        source_export "$repo" "$tree/runtime/components/${repo#"$RUNTIME_DIR/components/"}"
    done
    sync=(--sync "$ROOT/crates/moqui-client-tauri/Cargo.toml" --sync "$ROOT/crates/moqui-mobile/Cargo.toml")
    addons=()
    while IFS= read -r repo; do addons+=("$repo"); done < <(component_crate_dirs "$RUNTIME_DIR/$ADDON_SET" "$RUNTIME_DIR/$TEST_SET")
    while IFS= read -r repo; do framework_dev_crate "$repo" || addons+=("$repo"); done < <(component_crate_dirs "$ROOT/runtime/$DEV_SET")
    if [ ${#addons[@]} -gt 0 ]; then
        app=$temp/app
        write_app_crate "$app" "${addons[@]}"
        cargo fetch --manifest-path "$app/Cargo.toml"
        cp "$app/Cargo.lock" "$tree/vendor/app-Cargo.lock"
        sync+=(--sync "$app/Cargo.toml")
    fi
    cargo vendor --locked --versioned-dirs "${sync[@]}" "$tree/vendor/cargo" >"$temp/cargo-config"
    sed 's|^directory = .*|directory = "../vendor/cargo"|' "$temp/cargo-config" >"$tree/vendor/cargo-config.toml"
    node "$JS_SOURCE/libs/source-cache.mjs" "$tree/vendor/npm"
    mkdir -p "$tree/vendor/postgres"
    for target in aarch64-unknown-linux-gnu x86_64-unknown-linux-gnu aarch64-apple-darwin x86_64-apple-darwin x86_64-pc-windows-msvc; do
        "$ROOT/scripts/pg-payload.sh" strip "$target"
        payload=$("$ROOT/scripts/pg-payload.sh" path "$target")
        cp "$payload" "$tree/vendor/postgres/"
        (cd "$tree/vendor/postgres" && sha256_line "${payload##*/}" >"${payload##*/}.sha256")
    done
    printf 'Source archive of %s. Install the Rust toolchain in rust-toolchain.toml, its wasm32-unknown-unknown target, Node 22.13 or later with npm, and the system prerequisites in docs/Build-Test.md. Build with ./moq build --offline --release. Included revision and platform records are under vendor/.\n' "$label" >"$tree/SOURCE-RELEASE.txt"
    part=$(mktemp "$out.XXXXXX")
    tar --no-xattrs -C "$temp" -cf - mr-moqui | gzip -1 >"$part"
    mv -- "$part" "$out"
    du -h "$out"
)

help_source_archive() { echo 'Package the sources of a clean checkout (labelled with its tag when HEAD has one), component revisions, npm and Cargo sources, and release-platform Postgres payloads for an offline build.'; }
