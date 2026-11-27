# shellcheck shell=bash
# moq's the binary, the conf and properties of a run, the packages, features and embedding, and ./moq build. Sourced by moq, which holds the task table; bash 3.2, as moq is.

# ========== binary, conf, and properties ==========

# Set by parse_run_opts.
OPT_PACKAGE=
# Set by the --embed options (set_embed_opt) of build, run and archive: the level of the runtime built in, and the
# programs built in.
OPT_EMBED=
OPT_EMBED_OPENSEARCH=
OPT_EMBED_CHROMIUM=
OPT_CONF=
OPT_NOBUILD=
OPT_LOCATION=
OPT_PROPS=()
OPT_REST=()

valid_property() {
    case $1 in
        '' | [0-9]* | *[!A-Za-z0-9_]*) return 1 ;;
    esac
    return 0
}

add_property() {
    local name=${1%%=*}
    case $1 in *=*) ;; *) die "-D needs name=value, not '$1'" ;; esac
    if ! valid_property "$name"; then
        case $name in
            *.*) die "-D $name: use the underscore form of the property name ($(printf '%s' "$name" | tr . _))" ;;
            *) die "-D $name: not a valid property name" ;;
        esac
    fi
    OPT_PROPS+=("$1")
}

# $1 is "run" (the first word that is not an option starts the binary's arguments), "load" (options anywhere), or
# "test" (like load, but options moq does not know go to the binary, as moqui test's own --spec, --db, ...).
# Everything else lands in OPT_REST.
parse_run_opts() {
    local mode=$1 arg took
    shift
    OPT_PACKAGE=
    OPT_EMBED=
    OPT_EMBED_OPENSEARCH=
    OPT_EMBED_CHROMIUM=
    OPT_CONF=
    OPT_NOBUILD=
    OPT_LOCATION=
    OPT_PROPS=()
    OPT_REST=()
    while [ $# -gt 0 ]; do
        arg=$1
        took=$(set_embed_opt "$@") || exit 1
        if [ -n "$took" ]; then
            set_embed_opt "$@" >/dev/null
            shift "$took"
            continue
        fi
        case $arg in
            --lite) set_package lite ;;
            --debug) set_package debug ;;
            --release) set_package release ;;
            --enterprise) set_package enterprise ;;
            -c | --conf)
                [ $# -ge 2 ] || die "$arg needs a file"
                OPT_CONF=$2
                shift
                ;;
            --conf=*) OPT_CONF=${arg#--conf=} ;;
            -p | --port | --http-port | --port=* | --http-port=*)
                local port
                if [ "$arg" = "${arg#*=}" ]; then
                    [ $# -ge 2 ] || die "$arg needs a port number"
                    port=$2
                    shift
                else
                    port=${arg#*=}
                fi
                case $port in
                    off) ;;
                    '' | *[!0-9]*) die "port '$port' is not a number or off" ;;
                    *) OPT_PROPS+=("moqui_conf__webapp_list__webapp__WEBROOT__http_port=$port") ;;
                esac
                OPT_PROPS+=("moqui_conf__webapp_list__webapp__WEBROOT__http_listen_port=$port")
                ;;
            --https-port | --https-port=*)
                local https_port
                if [ "$arg" = "${arg#--https-port=}" ]; then
                    [ $# -ge 2 ] || die "$arg needs a port number or off"
                    https_port=$2
                    shift
                else
                    https_port=${arg#--https-port=}
                fi
                case $https_port in off) ;; '' | *[!0-9]*) die "https port '$https_port' is not a number or off" ;; esac
                OPT_PROPS+=("moqui_conf__webapp_list__webapp__WEBROOT__https_listen_port=$https_port")
                ;;
            -D)
                [ $# -ge 2 ] || die "-D needs name=value"
                add_property "$2"
                shift
                ;;
            -D?*) add_property "${arg#-D}" ;;
            --nobuild) OPT_NOBUILD=1 ;;
            -l | --location)
                [ "$mode" = load ] || die "unknown option $arg"
                [ $# -ge 2 ] || die "$arg needs a file"
                OPT_LOCATION=$2
                shift
                ;;
            --location=*)
                [ "$mode" = load ] || die "unknown option $arg"
                OPT_LOCATION=${arg#--location=}
                ;;
            --)
                shift
                OPT_REST+=("$@")
                return 0
                ;;
            -*)
                if [ "$mode" = test ]; then
                    OPT_REST+=("$arg")
                    shift
                    continue
                fi
                if [ "$mode" = run ]; then
                    die "unknown option $arg (moqui's own arguments go after its command, or after --)"
                fi
                die "unknown option $arg"
                ;;
            *)
                if [ "$mode" = run ]; then
                    OPT_REST+=("$@")
                    return 0
                fi
                OPT_REST+=("$arg")
                ;;
        esac
        shift
    done
}

# The packages (docs/Build-Test.md): release (the Maximal package with the dev tools, the default and --release), and
# lite, debug and enterprise from --lite, --debug and --enterprise. Each builds with its own cargo profile, so their
# artifacts and binaries stay apart, and with the package feature of crates/moqui (or of the generated crate in
# target/moqui-app). The debug package's profile is dev, cargo's own debug profile.
# The Dev package (default: the develop profile, -F dev, target/develop/moqui) is commented out: release is the default
# now, with everything Dev had; likely to be removed.
package_profile() {
    case $1 in
        # default) echo develop ;;
        debug) echo dev ;;
        *) echo "$1" ;;
    esac
}

package_features() {
    case $1 in
        # default) echo dev ;;
        lite) echo lite ;;
        debug) echo maximal,devtools ;;
        release) echo maximal,devtools ;;
        enterprise) echo enterprise ;;
    esac
}

# The profile's directory under target/: cargo puts the dev profile in target/debug.
package_dir() {
    case $1 in
        # default) echo develop ;;
        *) echo "$1" ;;
    esac
}

# Record the package an option asked for ($1: lite, debug, release, or enterprise). Two different ones are an error.
set_package() {
    [ -z "$OPT_PACKAGE" ] || [ "$OPT_PACKAGE" = "$1" ] || die "--$OPT_PACKAGE and --$1 ask for two packages"
    OPT_PACKAGE=$1
}

# An --embed option of build, run or archive: $1 is the option, $2 the word after it. Sets OPT_EMBED,
# OPT_EMBED_OPENSEARCH or OPT_EMBED_CHROMIUM and prints how many words it took (1 or 2); prints nothing when $1 is not
# one. Called in a subshell for the count and again for the setting.
set_embed_opt() {
    local level
    case $1 in
        --embed)
            [ $# -ge 2 ] || die "--embed needs a level: framework, production or all"
            level=$2
            ;;
        --embed=*) level=${1#--embed=} ;;
        --embed-opensearch)
            OPT_EMBED_OPENSEARCH=1
            echo 1
            return
            ;;
        --embed-chromium)
            OPT_EMBED_CHROMIUM=1
            echo 1
            return
            ;;
        *) return ;;
    esac
    case $level in
        framework | production | all) ;;
        *) die "--embed takes framework, production or all, not '$level'" ;;
    esac
    [ -z "$OPT_EMBED" ] || [ "$OPT_EMBED" = "$level" ] || die "--embed $OPT_EMBED and --embed $level ask for two levels"
    OPT_EMBED=$level
    case $1 in --embed) echo 2 ;; *) echo 1 ;; esac
}

# The cargo features the --embed options add, each after a comma.
embed_features() {
    case $OPT_EMBED in
        production) printf ',embed-production' ;;
        all) printf ',embed-all' ;;
    esac
    [ -z "$OPT_EMBED_OPENSEARCH" ] || printf ',embed-opensearch'
    [ -z "$OPT_EMBED_CHROMIUM" ] || printf ',embed-chromium'
}

# Where the program archives a build embeds are kept, as crates/moqui-embedded/build.rs finds them: MOQUI_EMBED_CACHE,
# else moqui/embed in the user's cache directory.
embed_cache_dir() {
    if [ -n "${MOQUI_EMBED_CACHE:-}" ]; then
        printf '%s\n' "$MOQUI_EMBED_CACHE"
    elif moq_on_windows && [ -n "${LOCALAPPDATA:-}" ]; then
        printf '%s\n' "$(cygpath -u "$LOCALAPPDATA" 2>/dev/null || printf '%s' "$LOCALAPPDATA")/moqui/embed"
    else
        printf '%s\n' "${XDG_CACHE_HOME:-$HOME/.cache}/moqui/embed"
    fi
}

# The Rust target of this host, which names the archives in the cache: rustc's, else from uname.
embed_host_target() {
    local target
    target=$(rustc -vV 2>/dev/null | sed -n 's/^host: //p')
    if [ -z "$target" ]; then
        case $(uname -s)/$(uname -m) in
            Linux/x86_64 | Linux/amd64) target=x86_64-unknown-linux-gnu ;;
            Linux/aarch64 | Linux/arm64) target=aarch64-unknown-linux-gnu ;;
            Darwin/arm64 | Darwin/aarch64) target=aarch64-apple-darwin ;;
            Darwin/x86_64) target=x86_64-apple-darwin ;;
            *) die "no Rust target known for $(uname -s) $(uname -m)" ;;
        esac
    fi
    printf '%s\n' "$target"
}

# What the --embed options need before cargo runs: the OpenSearch and Chromium archives in the cache (downloaded when
# missing, unless MOQUI_OPENSEARCH_ARCHIVE or MOQUI_CHROMIUM_ARCHIVE names one), and for the production and all levels
# the runtime to pack (MOQUI_EMBED_RUNTIME) and its component directories (MOQUI_EMBED_COMPONENTS), so a component added
# since the last build is packed.
prepare_embed() {
    local cache='' target=''
    if [ -n "$OPT_EMBED_OPENSEARCH" ] || [ -n "$OPT_EMBED_CHROMIUM" ]; then
        cache=$(embed_cache_dir)
        target=$(embed_host_target) || exit 1
    fi
    if [ -n "$OPT_EMBED_OPENSEARCH" ] && [ -z "${MOQUI_OPENSEARCH_ARCHIVE:-}" ] &&
        [ ! -f "$cache/opensearch-$OPENSEARCH_VERSION-$target.tar.gz" ]; then
        [ "${CARGO_NET_OFFLINE:-}" != true ] || die "offline build needs the OpenSearch archive in $cache"
        info "The OpenSearch archive to embed is not in $cache yet; getting it" >&2
        (task_opensearch_download --cache-only) >&2 || die "could not get the OpenSearch archive"
    fi
    if [ -n "$OPT_EMBED_CHROMIUM" ] && [ -z "${MOQUI_CHROMIUM_ARCHIVE:-}" ] &&
        [ ! -f "$cache/chromium-$target.tar.gz" ]; then
        [ "${CARGO_NET_OFFLINE:-}" != true ] || die "offline build needs the Chromium archive in $cache"
        info "The Chromium archive to embed is not in $cache yet; getting it" >&2
        (task_chromium_download --cache-only) >&2 || die "could not get the Chromium archive"
    fi
    case $OPT_EMBED in
        production | all)
            js_report_fonts || return 1
            MOQUI_EMBED_RUNTIME=$RUNTIME_DIR
            MOQUI_EMBED_COMPONENTS=$(cd "$RUNTIME_DIR/components" 2>/dev/null && ls -d -- */*/ 2>/dev/null | tr '\n' ' ')
            export MOQUI_EMBED_RUNTIME MOQUI_EMBED_COMPONENTS
            ;;
    esac
}

binary_path() {
    local name=moqui
    if moq_on_windows; then
        name=moqui.exe
    fi
    printf '%s\n' "$ROOT/target/$(package_dir "$1")/$name"
}

# cargo build for a package ($1: lite, debug, release, or enterprise), with any more cargo arguments after it. With
# component crates in runtime/components/addon (a Cargo.toml beside a component.xml) the binary is the generated crate in
# target/moqui-app, which links them with the framework; without, it is crates/moqui. Either way it lands at
# binary_path.
build_binary() {
    local package=$1
    shift
    need_cargo
    [ "$package" = enterprise ] || ensure_pg_archive
    # before cargo: an --embed production or all build packs the whole webroot, the module with it
    ensure_js
    ensure_vault_wasm
    prepare_embed
    local -a package_args
    package_args=(--profile "$(package_profile "$package")" --no-default-features
        --features "$(package_features "$package")$(embed_features)")
    local -a addons
    local addon
    while IFS= read -r addon; do addons+=("$addon"); done \
        < <(component_crate_dirs "$RUNTIME_DIR/$ADDON_SET" "$RUNTIME_DIR/$TEST_SET")
    # A dev component crate other than the framework's own (which crates/moqui links) needs the generated crate too.
    while IFS= read -r addon; do
        framework_dev_crate "$addon" || { addons+=("$addon"); }
    done < <(component_crate_dirs "$ROOT/runtime/$DEV_SET")
    if [ ${#addons[@]} -eq 0 ]; then
        run_cmd cargo build "${package_args[@]}" -p moqui "$@" >&2 || die "cargo build failed"
        return
    fi
    local app=$ROOT/target/moqui-app
    write_app_crate "$app" "${addons[@]}"
    local manifest=$app/Cargo.toml target_dir=$ROOT/target
    if moq_on_windows; then
        manifest=$(win_path "$manifest")
        target_dir=$(win_path "$target_dir")
    fi
    run_cmd cargo build "${package_args[@]}" --manifest-path "$manifest" --target-dir "$target_dir" "$@" >&2 \
        || die "cargo build failed"
}

# The Postgres archive a package with embedded Postgres embeds (crates/moqui-drivers/build.rs): downloaded and stripped of
# psql by scripts/pg-payload.sh strip the first time, unless MOQUI_PG_ARCHIVE names one.
ensure_pg_archive() {
    local payload=$ROOT/scripts/pg-payload.sh archive
    [ -z "${MOQUI_PG_ARCHIVE:-}" ] && [ -x "$payload" ] || return 0
    archive=$("$payload" path) || die "could not find where the Postgres archive is kept"
    [ ! -f "$archive" ] || return 0
    [ "${CARGO_NET_OFFLINE:-}" != true ] || die "offline build needs $archive"
    info "The embedded Postgres archive is not there yet; getting it ($(rel "$payload") strip)" >&2
    run_cmd "$payload" strip >&2 || die "could not get the embedded Postgres archive"
}

ensure_js() {
    [ "${MOQ_JS_ENSURED:-}" != "$JS_LIBS" ] || return 0
    task_build_js >&2 || die "could not build browser libraries. Run ./moq build-js"
    MOQ_JS_ENSURED=$JS_LIBS
    export MOQ_JS_ENSURED
}

# The vault's browser module (js/vault/moqui_vault.wasm in the webroot), which is not kept in git: scripts/vault-wasm.sh
# builds it when it is missing or stale and says nothing when it is up to date, which takes well under a second. It runs
# once in a task and the moq tasks it starts (VAULT_WASM_ENSURED, exported). $1 soft: a failure is a warning (a task
# that runs moqui with --nobuild; the vault screens and the VaultModule spec then say the module is not built), else it
# ends the task. Nothing when the script is not there (a tree without the vault crate).
ensure_vault_wasm() {
    local script=$ROOT/scripts/vault-wasm.sh
    [ -z "${VAULT_WASM_ENSURED:-}" ] && [ -f "$script" ] || return 0
    if run_cmd bash "$script" --quiet >&2; then
        VAULT_WASM_ENSURED=1
        export VAULT_WASM_ENSURED
        return 0
    fi
    [ "${1:-}" = soft ] || die "could not build the vault's browser module ($(rel "$script"))"
    warn "could not build the vault's browser module ($(rel "$script")); the vault screens answer that it is not built"
}

# The component directories under each of $@ that have a Rust crate: a Cargo.toml beside the component.xml.
component_crate_dirs() {
    local base dir
    for base in "$@"; do
        [ -d "$base" ] || continue
        for dir in "$base"/*/; do
            dir=${dir%/}
            [ -f "$dir/component.xml" ] && [ -f "$dir/Cargo.toml" ] && printf '%s\n' "$dir"
        done
    done
}

# Whether the component crate in $1 is one of the framework's dev components, tracked in this repository and linked
# by crates/moqui under its devtools feature.
framework_dev_crate() {
    case $1 in
        "$ROOT/runtime/$DEV_SET/dev-aide" | "$ROOT/runtime/$DEV_SET/example") return 0 ;;
    esac
    return 1
}

# The [package] name in the Cargo.toml in $1.
crate_package_name() {
    sed -n '/^\[package\]/,/^\[/s/^name[[:space:]]*=[[:space:]]*"\([^"]*\)".*/\1/p' "$1/Cargo.toml" | head -n 1
}

# Whether the crate in $1 declares the feature $2 in its [features] table.
crate_has_feature() {
    sed -n '/^\[features\]/,/^\[/p' "$1/Cargo.toml" | grep -q "^$2[[:space:]]*="
}

# Write $2 to the file $1 when its content differs, so cargo sees no change on a build with the same components.
write_if_changed() {
    if [ -f "$1" ] && [ "$(cat "$1")" = "$2" ]; then
        return 0
    fi
    if dry_run; then
        show_cmd write "$1"
        return 0
    fi
    printf '%s\n' "$2" >"$1" || die "could not write $1"
}

# The repository's [patch.crates-io] table, its paths made absolute, for a crate that is a workspace of its own: cargo
# takes patches only from the root manifest of the workspace it builds, so without these such a crate builds the
# crates.io releases the repository replaces, a security fix among them (docs/Build-Test.md Forks;
# plans/Security-Review.md SEC-CLI-126).
root_patch_table() {
    [ -f "$ROOT/Cargo.toml" ] || return 0
    awk -v root="$ROOT" '
        /^\[/ { patches = ($0 == "[patch.crates-io]") }
        patches {
            gsub(/path = "/, "path = \"" root "/")
            print
        }' "$ROOT/Cargo.toml"
}

# Generate the crate in $1 that links the component crates in $2... with the framework: the components/base crates that
# crates/moqui links, then the add-on ones. The crates of runtime/components/dev (in $2... too, or the framework's own)
# are optional dependencies that the devtools feature turns on. Its Cargo.lock starts from the repository's, so the
# reviewed versions stay and only the component crates' own dependencies are added.
write_app_crate() {
    local app=$1 dir package deps="" uses="" unoptimized="" crate tui_features= devtools_features=
    shift
    local -a dirs
    while IFS= read -r dir; do dirs+=("$dir"); done < <(component_crate_dirs "$ROOT/runtime/$BASE_SET")
    while IFS= read -r dir; do
        framework_dev_crate "$dir" && dirs+=("$dir")
    done < <(component_crate_dirs "$ROOT/runtime/$DEV_SET")
    dirs+=("$@")
    for dir in "${dirs[@]}"; do
        package=$(crate_package_name "$dir")
        [ -n "$package" ] || die "$(rel "$dir")/Cargo.toml has no [package] name"
        unoptimized+=$'\n'"[profile.dev.package.$package]"$'\n'"opt-level = 0"$'\n'"debug = true"$'\n'
        case $dir in
            "$ROOT/runtime/$DEV_SET/"*)
                deps+="$package = { path = \"$dir\", optional = true }"$'\n'
                devtools_features+=", \"dep:$package\""
                uses+="#[cfg(feature = \"devtools\")]"$'\n'"use ${package//-/_} as _;"$'\n'
                ;;
            *)
                deps+="$package = { path = \"$dir\" }"$'\n'
                uses+="use ${package//-/_} as _;"$'\n'
                ;;
        esac
        # A component crate with its own terminal components (a tui feature, plans/Screen-TUI-Impl.md 2.9) has them
        # registered when the binary has the TUI.
        if crate_has_feature "$dir" tui; then
            tui_features+=", \"$package/tui\""
        fi
        info "Linking component crate $package ($(rel "$dir"))" >&2
    done
    if ! dry_run; then
        mkdir -p "$app/src" || die "could not create $(rel "$app")"
    fi
    for crate in moqui-lib moqui-db moqui-entity moqui-core moqui-llm moqui-git moqui-mail moqui-report moqui-collab moqui-tools moqui-calc moqui-maps moqui-rtc moqui-sip moqui-channels moqui-xmpp moqui-dav moqui-devkit moqui-embedded moqui-drivers moqui-vault moqui-vault-server moqui-instance moqui-tui moqui-host; do
        unoptimized+=$'\n'"[profile.dev.package.$crate]"$'\n'"opt-level = 0"$'\n'"debug = true"$'\n'
    done
    write_if_changed "$app/Cargo.toml" "# Generated by ./moq build from the component crates in runtime. Written again on each build; do not edit.
# The moqui binary of crates/moqui, with the add-on component crates linked too.

[package]
name = \"moqui-app\"
version = \"0.1.0\"
edition = \"2021\"
publish = false

# The library names the component crates, so a program that hosts the node itself (crates/moqui-desktop) links them
# through this crate too.
[lib]
name = \"moqui_app\"
path = \"src/lib.rs\"

[[bin]]
name = \"moqui\"
path = \"src/main.rs\"

# The packages, as crates/moqui has them (docs/Build-Test.md). ./moq build picks one with --no-default-features.
[features]
default = [\"maximal\", \"devtools\"]
# dev = [\"moqui-host/dev\", \"tui\", \"devtools\"]
lite = [\"moqui-host/lite\", \"tui\"]
enterprise = [\"moqui-host/enterprise\", \"tui\"]
maximal = [\"moqui-host/maximal\", \"tui\"]
inbound-smtp = [\"moqui-host/inbound-smtp\"]
embed-production = [\"moqui-host/embed-production\"]
embed-all = [\"moqui-host/embed-all\"]
embed-opensearch = [\"moqui-host/embed-opensearch\"]
embed-chromium = [\"moqui-host/embed-chromium\"]
tui = [\"moqui-host/tui\"${tui_features}]
devtools = [\"moqui-host/devtools\"${devtools_features}]

[dependencies]
moqui-host = { path = \"$ROOT/crates/moqui-host\", default-features = false }
moqui-core = { path = \"$ROOT/crates/moqui-core\", default-features = false }
${deps}
# The global allocator, as crates/moqui has it (src/main.rs); not on MSVC.
[target.'cfg(not(target_env = \"msvc\"))'.dependencies]
tikv-jemallocator = \"0.7\"

# Its own workspace: not a member of the repository's.
[workspace]

# The repository's crate patches: a workspace of its own takes none of the repository's (root_patch_table in
# scripts/moq/build.sh).
$(root_patch_table)

# As in the repository's workspace, a debug build leaves the framework and component crates at opt-level 0 with full
# debug info and optimizes the other dependencies, with line tables only (.cargo/config.toml). Here they are not workspace
# members, so they are named.
${unoptimized}"
    write_if_changed "$app/src/lib.rs" "// Generated by ./moq build. Written again on each build; do not edit. The component crates, each named so that its
// submit_component! entry stays in a binary that calls link().

${uses}
/// Keeps the component crates named above in the binary that calls it.
pub fn link() {}"
    write_if_changed "$app/src/main.rs" "// Generated by ./moq build. Written again on each build; do not edit. The same as crates/moqui/src/main.rs with
// the add-on component crates named too, through the library.

// jemalloc, as crates/moqui/src/main.rs has it (plans/archive/Profiling.md Round 37).
#[cfg(not(target_env = \"msvc\"))]
#[global_allocator]
static GLOBAL: tikv_jemallocator::Jemalloc = tikv_jemallocator::Jemalloc;

fn main() {
    moqui_app::link();
    moqui_host::main();
}"
    local seed=$ROOT/Cargo.lock
    [ ! -f "$ROOT/vendor/app-Cargo.lock" ] || seed=$ROOT/vendor/app-Cargo.lock
    if [ ! -f "$app/Cargo.lock" ] || [ "$seed" -nt "$app/Cargo.lock" ]; then
        run_cmd cp "$seed" "$app/Cargo.lock" || die "could not copy Cargo.lock"
    fi
}

# The binary for a package, built first so it is not stale; with --nobuild only when it is not there. With --nobuild
# and no package option, the newer of the release and debug binaries. Which one is used goes to stderr, with a warning
# when a Rust source, Cargo.toml or Cargo.lock under crates or runtime/components is newer than it.
moqui_binary() {
    local package=$1 bin newer
    if [ -n "$OPT_NOBUILD" ] && [ -z "$OPT_PACKAGE" ]; then
        local release debug
        release=$(binary_path release)
        debug=$(binary_path debug)
        if [ -x "$debug" ] && { [ ! -x "$release" ] || [ "$debug" -nt "$release" ]; }; then
            package=debug
        fi
    fi
    bin=$(binary_path "$package")
    if [ -z "$OPT_NOBUILD" ]; then
        build_binary "$package"
    elif [ ! -x "$bin" ]; then
        info "$(rel "$bin") is not built, building it" >&2
        build_binary "$package"
    else
        ensure_vault_wasm soft
    fi
    if [ ! -x "$bin" ] && ! dry_run; then
        die "$(rel "$bin") is not there after the build"
    fi
    if [ -x "$bin" ]; then
        info "using $(rel "$bin")" >&2
        if [ -n "$OPT_NOBUILD" ]; then
            newer=$(find "$ROOT/crates" "$ROOT/runtime/components" -path '*/target' -prune -o \
                \( -name '*.rs' -o -name Cargo.toml -o -name Cargo.lock \) -newer "$bin" -print -quit 2>/dev/null)
            [ -z "$newer" ] || warn "$(rel "$bin") is older than $(rel "$newer"); drop --nobuild to build it again"
        fi
    fi
    printf '%s\n' "$bin"
}

# The binary for the package asked for (--lite, --debug, --release, --enterprise), else the Maximal package.
any_binary() {
    moqui_binary "${OPT_PACKAGE:-release}"
}

# MOQUI_CONF as an absolute path. A relative name is tried in the runtime directory, then from the caller's directory.
resolve_conf() {
    local conf=$1
    if is_abs_path "$conf"; then
        [ -f "$conf" ] || die "conf file $conf not found"
    elif [ -f "$RUNTIME_DIR/$conf" ]; then
        conf=$RUNTIME_DIR/$conf
    elif [ -f "$MOQ_CWD/$conf" ]; then
        conf=$MOQ_CWD/$conf
    else
        die "conf file $conf not found in $(rel "$RUNTIME_DIR") or $(rel "$MOQ_CWD")"
    fi
    printf '%s\n' "$conf"
}

# Exports the conf and properties from parse_run_opts. $1 is the default conf, or empty for the binary's default.
apply_run_env() {
    local conf=${OPT_CONF:-$1} prop
    if [ -n "$conf" ]; then
        conf=$(resolve_conf "$conf") || exit 1
        export_var MOQUI_CONF "$conf"
    fi
    for prop in "${OPT_PROPS[@]}"; do
        export_var "${prop%%=*}" "${prop#*=}"
    done
}

# ========== build task ==========
task_build() {
    local -a cargo_args
    local took
    cargo_args=()
    OPT_PACKAGE=
    OPT_EMBED=
    OPT_EMBED_OPENSEARCH=
    OPT_EMBED_CHROMIUM=
    while [ $# -gt 0 ]; do
        took=$(set_embed_opt "$@") || exit 1
        if [ -n "$took" ]; then
            set_embed_opt "$@" >/dev/null
            shift "$took"
            continue
        fi
        case $1 in
            --offline) enable_offline_build || return 1; cargo_args+=(--offline) ;;
            --lite) set_package lite ;;
            --debug) set_package debug ;;
            --release) set_package release ;;
            --enterprise) set_package enterprise ;;
            --)
                shift
                cargo_args+=("$@")
                break
                ;;
            *) cargo_args+=("$1") ;;
        esac
        shift
    done
    build_binary "${OPT_PACKAGE:-release}" "${cargo_args[@]}"
}


# ========== help ==========

help_build() {
    cat <<EOF
Builds a moqui package with cargo in this repo (docs/Build-Test.md): the Maximal package with the
dev tools by default, or Lite, Debug or Enterprise. Each has its own cargo profile, features and binary:
  Maximal     cargo build --profile release --no-default-features -F maximal,devtools -p moqui
                                                                                 target/release/moqui
  Lite        cargo build --profile lite --no-default-features -F lite -p moqui        target/lite/moqui
  Debug       cargo build --profile dev --no-default-features -F maximal,devtools -p moqui
                                                                                 target/debug/moqui
  Enterprise  cargo build --profile enterprise --no-default-features -F enterprise -p moqui
                                                                                 target/enterprise/moqui
Other words go to cargo as they are. The tasks that run moqui (run, load, test, save-db, ...) build
the binary they need first, so it is never stale; --nobuild skips that when the binary is there.

Before cargo, every build makes the vault's browser module (webroot js/vault/moqui_vault.wasm, not
kept in git) with scripts/vault-wasm.sh when it is missing or stale, adding the wasm32-unknown-unknown
target with rustup when that is missing; it is quick when the module is up to date. archive, the test
and accept tasks, and the tasks that run moqui with --nobuild do the same.

A component in runtime/components/addon with a Cargo.toml beside its component.xml is a Rust crate: the
build generates target/moqui-app, a crate that links those crates with the framework, and builds
it to the same binary path. See runtime/components/addon/README.md. A crate of a dev component in
runtime/components/dev is linked only into the packages with the dev tools (Maximal and Debug).
  ./moq build
  ./moq build --lite
  ./moq build --enterprise -- --timings

A build for distribution also strips psql from the bundled Postgres first; see README.md
(scripts/pg-payload.sh strip, then check).

With MOQ_SCCACHE=1 (opt-in; off by default) the cargo runs of moq (build, desktop, test-all, and
the builds of the tasks that run moqui) use sccache as the rustc wrapper, so worktrees of this repo
share compiled dependencies; incremental builds of the workspace crates work as before. When sccache
is not on the PATH, moq warns, says how to install it, and builds without it. A wrapper you set
(RUSTC_WRAPPER, CARGO_BUILD_RUSTC_WRAPPER, or rustc-wrapper in a cargo config) is kept. The cache
directory and size are sccache's own settings, SCCACHE_DIR and SCCACHE_CACHE_SIZE
(docs/Build-Test.md, Compile cache).

A binary can carry more than the framework: --embed production builds in every component of the
runtime that ./moq archive would put in, so the binary alone (gzipped, ./moq archive --binary) is
the deployment, and --embed-opensearch and --embed-chromium build in those programs, which unpack
into install/ and run as their own processes, as the embedded Postgres does. Their archives come
from the cache ./moq opensearch-download and ./moq chromium-download fill (got when missing).
  ./moq build --embed production --embed-opensearch --embed-chromium

Options:
$PACKAGE_OPTIONS_HELP
$EMBED_OPTIONS_HELP
      --               End of moq options; the rest goes to cargo
EOF
}
