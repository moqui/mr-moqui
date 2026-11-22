# shellcheck shell=bash
# moq's ./moq test, test-client, test-load, test-framework, test-apps, test-all, test-clients, accept, accept-all and autodoc. Sourced by moq, which holds the task table; bash 3.2, as moq is.

# ========== test and accept tasks ==========

task_test() {
    parse_run_opts test "$@"
    local bin
    bin=$(moqui_binary "${OPT_PACKAGE:-release}") || exit 1
    apply_run_env ""
    exec_moqui "$bin" test "${OPT_REST[@]}"
}

# The client reads no conf: -p N is the server at http://localhost:N, and -c and -D are not passed on.
task_test_client() {
    parse_run_opts test "$@"
    local bin prop
    local -a server
    server=()
    for prop in "${OPT_PROPS[@]}"; do
        case $prop in
            moqui_conf__webapp_list__webapp__WEBROOT__http_port=*) server=(--server "http://localhost:${prop#moqui_conf__webapp_list__webapp__WEBROOT__http_port=}") ;;
        esac
    done
    bin=$(moqui_binary "${OPT_PACKAGE:-release}") || exit 1
    exec_moqui "$bin" test-client "${server[@]}" "${OPT_REST[@]}"
}

# As test-client: the client reads no conf, so -p N is the server at http://localhost:N.
task_test_load() {
    parse_run_opts test "$@"
    local bin prop
    local -a server
    server=()
    for prop in "${OPT_PROPS[@]}"; do
        case $prop in
            moqui_conf__webapp_list__webapp__WEBROOT__http_port=*) server=(--server "http://localhost:${prop#moqui_conf__webapp_list__webapp__WEBROOT__http_port=}") ;;
        esac
    done
    # moq runs from the repo root: --out, the --compare files and a path target are the caller's, made absolute
    local -a args
    local arg comparing=''
    args=()
    while [ ${#OPT_REST[@]} -gt 0 ]; do
        arg=${OPT_REST[0]}
        OPT_REST=("${OPT_REST[@]:1}")
        case $arg in
            --out)
                args+=("$arg")
                if [ ${#OPT_REST[@]} -gt 0 ]; then
                    args+=("$(abs_path "${OPT_REST[0]}")")
                    OPT_REST=("${OPT_REST[@]:1}")
                fi
                comparing=''
                ;;
            --compare) args+=("$arg"); comparing=1 ;;
            --script | --server | --ssh | --param | --users | --ramp-up | --duration | --iterations | --rate | --scenario | \
                --label | --log-level | --timeout)
                args+=("$arg")
                if [ ${#OPT_REST[@]} -gt 0 ]; then
                    args+=("${OPT_REST[0]}")
                    OPT_REST=("${OPT_REST[@]:1}")
                fi
                comparing=''
                ;;
            -*) args+=("$arg"); comparing='' ;;
            *)
                if [ -n "$comparing" ] || [ -e "$MOQ_CWD/$arg" ] || is_abs_path "$arg"; then
                    args+=("$(abs_path "$arg")")
                else
                    args+=("$arg")
                fi
                ;;
        esac
    done
    bin=$(moqui_binary "${OPT_PACKAGE:-release}") || exit 1
    exec_moqui "$bin" test-load "${server[@]}" "${args[@]}"
}

# moqui autodoc with the caller's directory as the base: --out defaults to ./autodoc there, and the path options are
# made absolute from it, since moq runs the binary from ROOT.
task_autodoc() {
    local arg flag value has_out='' bin
    local -a args
    args=()
    OPT_PACKAGE=
    OPT_NOBUILD=
    while [ $# -gt 0 ]; do
        arg=$1
        case $arg in
            --lite) set_package lite ;;
            --debug) set_package debug ;;
            --release) set_package release ;;
            --enterprise) set_package enterprise ;;
            --nobuild) OPT_NOBUILD=1 ;;
            --)
                shift
                args+=("$@")
                break
                ;;
            --out | --templates | --project | --java | --out=* | --templates=* | --project=* | --java=*)
                if [ "$arg" = "${arg#*=}" ]; then
                    flag=$arg
                    [ $# -ge 2 ] && [ -n "$2" ] || die "$flag needs a directory"
                    value=$2
                    shift
                else
                    flag=${arg%%=*}
                    value=${arg#*=}
                    [ -n "$value" ] || die "$flag needs a directory"
                fi
                [ "$flag" != --out ] || has_out=1
                args+=("$flag" "$(abs_path "$value")")
                ;;
            *) args+=("$arg") ;;
        esac
        shift
    done
    [ -n "$has_out" ] || args+=(--out "$MOQ_CWD/autodoc")
    bin=$(moqui_binary "${OPT_PACKAGE:-release}") || exit 1
    exec_moqui "$bin" autodoc "${args[@]}"
}

# The suites of test-framework, test-apps and test-all: each runs whatever the last did, then a summary lists them
# and the task exits 1 when any failed. SUITE_RESULTS and SUITE_FAILED are the run's. Those tasks and accept build the
# vault's browser module first (ensure_vault_wasm), once for the moq runs they start.
suite() {
    local name=$1 status
    shift
    heading "== $name"
    run_cmd "$@"
    status=$?
    if [ $status -eq 0 ]; then
        SUITE_RESULTS+=("passed  $name")
    else
        SUITE_RESULTS+=("FAILED  $name (exit $status)")
        SUITE_FAILED=1
    fi
}

# suite_summary <title>: the list of the suites run, and the exit status.
suite_summary() {
    local line
    printf '\n'
    heading "$1"
    for line in "${SUITE_RESULTS[@]}"; do
        printf '  %s\n' "$line"
    done
    return $SUITE_FAILED
}

# The workers of moqui test in test-framework, test-apps and test-all: MOQ_TEST_WORKERS, 4 by default.
TEST_WORKERS=${MOQ_TEST_WORKERS:-4}

# parse_suite_opts <task> <args...>: the package options and --nobuild into SUITE_MOQ_OPTS.
parse_suite_opts() {
    local task=$1 arg
    shift
    SUITE_MOQ_OPTS=()
    SUITE_RESULTS=()
    SUITE_FAILED=0
    while [ $# -gt 0 ]; do
        arg=$1
        case $arg in
            --lite | --debug | --release | --enterprise | --nobuild) SUITE_MOQ_OPTS+=("$arg") ;;
            *) die "unknown option $arg ($task takes a package option and --nobuild)" ;;
        esac
        shift
    done
}

# The framework's suites: cargo test, the tests of this script, and the specs, client specs and security proofs of
# components/base and components/dev.
framework_suites() {
    export MOQUI_SHARED_RUNTIME=off
    local name
    local -a server client proofs
    server=()
    client=()
    proofs=()
    while IFS= read -r name; do
        [ "$name" = demo ] || server+=("$name")
    done < <(spec_components test "$BASE_SET" "$DEV_SET")
    while IFS= read -r name; do
        client+=("$name")
    done < <(served_components test/client "$BASE_SET" "$DEV_SET")
    while IFS= read -r name; do
        proofs+=("$name")
    done < <(served_components test/proofs "$BASE_SET" "$DEV_SET")
    need_cargo
    suite "cargo test --workspace" cargo test --workspace
    suite "scripts/moq-test.sh" bash scripts/moq-test.sh
    if [ ${#server[@]} -gt 0 ]; then
        suite "moqui test (framework)" "$MOQ" test "${SUITE_MOQ_OPTS[@]}" -- "${server[@]}" --workers "$TEST_WORKERS"
    fi
    suite "moqui test demo" "$MOQ" test "${SUITE_MOQ_OPTS[@]}" --nobuild -D demo_disabled=false demo
    # the binary is built by now. Offline specs open no server; --serve runs the suites that declare one.
    suite "moqui test-client webroot --offline" "$MOQ" test-client "${SUITE_MOQ_OPTS[@]}" --nobuild -- webroot --offline
    suite "moqui test-client runtime/$DEV_SET/dev-aide --offline" "$MOQ" test-client "${SUITE_MOQ_OPTS[@]}" \
        --nobuild -- "runtime/$DEV_SET/dev-aide" --offline
    if [ ${#client[@]} -gt 0 ]; then
        suite "moqui test-client --serve (framework)" "$MOQ" test-client "${SUITE_MOQ_OPTS[@]}" --nobuild -- --serve \
            "${client[@]}"
    fi
    if [ ${#proofs[@]} -gt 0 ]; then
        suite "moqui test-client --serve --proofs (framework)" "$MOQ" test-client "${SUITE_MOQ_OPTS[@]}" --nobuild -- \
            --serve --proofs "${proofs[@]}"
    fi
}

# The applications' suites: the specs, client specs and security proofs of the components under
# runtime/components/addon. No cargo; the binary is built only when it is missing.
apps_suites() {
    local name
    local -a server client proofs
    server=()
    client=()
    proofs=()
    while IFS= read -r name; do
        server+=("$name")
    done < <(spec_components test "$ADDON_SET" "$TEST_SET")
    while IFS= read -r name; do
        client+=("$name")
    done < <(served_components test/client "$ADDON_SET" "$TEST_SET")
    while IFS= read -r name; do
        proofs+=("$name")
    done < <(served_components test/proofs "$ADDON_SET" "$TEST_SET")
    if [ ${#server[@]} -eq 0 ] && [ ${#client[@]} -eq 0 ] && [ ${#proofs[@]} -eq 0 ]; then
        info "No application component under runtime/components/addon has specs."
        return 0
    fi
    if [ ${#server[@]} -gt 0 ]; then
        suite "moqui test (apps)" "$MOQ" test "${SUITE_MOQ_OPTS[@]}" --nobuild -- "${server[@]}" --workers "$TEST_WORKERS"
    fi
    if [ ${#client[@]} -gt 0 ]; then
        suite "moqui test-client --serve (apps)" "$MOQ" test-client "${SUITE_MOQ_OPTS[@]}" --nobuild -- --serve \
            "${client[@]}"
    fi
    if [ ${#proofs[@]} -gt 0 ]; then
        suite "moqui test-client --serve --proofs (apps)" "$MOQ" test-client "${SUITE_MOQ_OPTS[@]}" --nobuild -- \
            --serve --proofs "${proofs[@]}"
    fi
}

task_test_framework() {
    parse_suite_opts test-framework "$@"
    ensure_vault_wasm
    framework_suites
    suite_summary test-framework
}

task_test_apps() {
    parse_suite_opts test-apps "$@"
    ensure_vault_wasm
    apps_suites
    suite_summary test-apps
}

# parse_accept_opts <task> <args...>: the components into ACCEPT_NAMES, --spec into ACCEPT_SPECS, --junit (a dated
# directory under it) into ACCEPT_JUNIT, and the package options and --nobuild into SUITE_MOQ_OPTS.
parse_accept_opts() {
    local task=$1 arg
    shift
    ACCEPT_NAMES=()
    ACCEPT_SPECS=
    ACCEPT_JUNIT=
    SUITE_MOQ_OPTS=()
    SUITE_RESULTS=()
    SUITE_FAILED=0
    while [ $# -gt 0 ]; do
        arg=$1
        case $arg in
            --lite | --debug | --release | --enterprise | --nobuild) SUITE_MOQ_OPTS+=("$arg") ;;
            --spec)
                [ $# -ge 2 ] && [ "$task" = accept ] || die "$task: --spec needs a name (accept only)"
                ACCEPT_SPECS=$2
                shift
                ;;
            --junit)
                [ $# -ge 2 ] || die "--junit needs a directory"
                ACCEPT_JUNIT="$(abs_path "$2")/$(date +%Y%m%d-%H%M%S)"
                shift
                ;;
            -*) die "unknown option $arg ($task takes --spec, --junit, a package option and --nobuild)" ;;
            *)
                [ "$task" = accept ] || die "unknown argument $arg (accept-all runs every component)"
                ACCEPT_NAMES+=("$arg")
                ;;
        esac
        shift
    done
}

# in_names <name>: whether ACCEPT_NAMES is empty or holds the name.
in_names() {
    local name
    [ ${#ACCEPT_NAMES[@]} -gt 0 ] || return 0
    for name in "${ACCEPT_NAMES[@]}"; do
        [ "$name" != "$1" ] || return 0
    done
    return 1
}

# has_spec_file <tree> <component>: whether the component's <tree> holds one of the ACCEPT_SPECS files.
has_spec_file() {
    local tree=$1 name=$2 d spec
    local set
    for set in "${COMPONENT_SETS[@]}"; do for d in "$RUNTIME_DIR/$set"/*/; do
        [ -f "${d}component.xml" ] && [ "$(component_name_of "${d%/}")" = "$name" ] || continue
        for spec in ${ACCEPT_SPECS//,/ }; do
            if [ "$tree" = test/acceptance ]; then
                [ -z "$(find "${d}$tree" -path "${d}$tree/client" -prune -o -name "$spec.spec.js" -print -quit)" ] || return 0
            else
                [ -z "$(find "${d}$tree" -name "$spec.spec.js" -print -quit 2>/dev/null)" ] || return 0
            fi
        done
    done; done
    return 1
}

# spec_is_offline <file>: whether the spec's own spec({ ... }) call sets server: false (comment lines left out)
spec_is_offline() {
    perl -e 'local $/; my $s = <> // ""; $s =~ s{^\s*//.*$}{}mg; exit($s =~ /\bspec\(\s*\{[^{}]*?\bserver\s*:\s*false\b/s ? 0 : 1)' "$1"
}

# named_specs_offline <name>: whether every spec of ACCEPT_SPECS in the component's test/acceptance/client is an offline
# one (server: false), so a suite that declares a server runs them with --offline (its --serve run skips them).
named_specs_offline() {
    local name=$1 d spec file set found=0
    for set in "${COMPONENT_SETS[@]}"; do for d in "$RUNTIME_DIR/$set"/*/; do
        [ -f "${d}component.xml" ] && [ "$(component_name_of "${d%/}")" = "$name" ] || continue
        for spec in ${ACCEPT_SPECS//,/ }; do
            file=$(find "${d}test/acceptance/client" -name "$spec.spec.js" -print -quit 2>/dev/null)
            [ -n "$file" ] || continue
            spec_is_offline "$file" || return 1
            found=1
        done
    done; done
    [ "$found" = 1 ]
}

# Refuses a component of ACCEPT_NAMES (all when empty) that still has its acceptance suite in a top-level acceptance/,
# which moved to test/acceptance/ (plans/archive/Components-Dir.md), so its specs are not left out without a word.
refuse_stale_acceptance() {
    local set d
    for set in "${COMPONENT_SETS[@]}"; do for d in "$RUNTIME_DIR/$set"/*/; do
        [ -d "${d}acceptance" ] && [ -f "${d}component.xml" ] || continue
        in_names "$(component_name_of "${d%/}")" || continue
        die "the acceptance suite is in test/acceptance/ now; move acceptance/ there (git mv acceptance test/acceptance) in $(rel "${d%/}")"
    done; done
}

# The acceptance suites of the components of ACCEPT_NAMES (all when empty).
accept_suites() {
    local name
    local -a server client offline spec junit demo
    refuse_stale_acceptance
    server=()
    client=()
    offline=()
    spec=()
    junit=()
    demo=()
    [ -z "$ACCEPT_SPECS" ] || spec=(--spec "$ACCEPT_SPECS")
    [ -z "$ACCEPT_JUNIT" ] || junit=(--junit "$ACCEPT_JUNIT")
    while IFS= read -r name; do
        in_names "$name" || continue
        [ -z "$ACCEPT_SPECS" ] || has_spec_file test/acceptance "$name" || continue
        server+=("$name")
        [ "$name" != demo ] || demo=(-D demo_disabled=false)
    done < <(spec_components test/acceptance "${COMPONENT_SETS[@]}")
    while IFS= read -r name; do
        in_names "$name" || continue
        [ -z "$ACCEPT_SPECS" ] || has_spec_file test/acceptance/client "$name" || continue
        if [ -n "$ACCEPT_SPECS" ] && named_specs_offline "$name"; then offline+=("$name"); continue; fi
        client+=("$name")
    done < <(served_components test/acceptance/client "${COMPONENT_SETS[@]}")
    while IFS= read -r name; do
        in_names "$name" || continue
        [ -z "$ACCEPT_SPECS" ] || has_spec_file test/acceptance/client "$name" || continue
        offline+=("$name")
    done < <(offline_components test/acceptance/client "${COMPONENT_SETS[@]}")
    if [ ${#server[@]} -eq 0 ] && [ ${#client[@]} -eq 0 ] && [ ${#offline[@]} -eq 0 ]; then
        info "No acceptance specs${ACCEPT_SPECS:+ named $ACCEPT_SPECS} in ${ACCEPT_NAMES[*]:-any component}."
        return 0
    fi
    if [ ${#server[@]} -gt 0 ]; then
        suite "moqui test --acceptance" "$MOQ" test "${SUITE_MOQ_OPTS[@]}" "${demo[@]}" -- "${server[@]}" --acceptance \
            "${spec[@]}" "${junit[@]}"
    fi
    if [ ${#client[@]} -gt 0 ]; then
        suite "moqui test-client --serve --acceptance" "$MOQ" test-client "${SUITE_MOQ_OPTS[@]}" --nobuild -- --serve \
            --acceptance "${client[@]}" "${spec[@]}" "${junit[@]}"
    fi
    if [ ${#offline[@]} -gt 0 ]; then
        suite "moqui test-client --offline --acceptance" "$MOQ" test-client "${SUITE_MOQ_OPTS[@]}" --nobuild -- --offline \
            --acceptance "${offline[@]}" "${spec[@]}" "${junit[@]}"
    fi
}

task_accept() {
    parse_accept_opts accept "$@"
    ensure_vault_wasm
    accept_suites
    suite_summary accept
}

task_accept_all() {
    parse_accept_opts accept-all "$@"
    ensure_vault_wasm
    accept_suites
    need_cargo
    suite "opt-in host tests" cargo test -p moqui-host --test gate3 --test client_browser --test tui_screens
    suite "gate3_browser" cargo test -p moqui-host --test gate3_browser
    suite "cargo test-slow" cargo test-slow
    suite "moqui cli ignored tests" cargo test -p moqui --test cli -- --ignored
    suite "generation on both script engines" cargo test -p moqui-core --lib -- --ignored \
        screen::generate::tests::every_screens_widgets_generate_the_same_on_both_engines
    if [ -n "${MOQUI_TEST_MYSQL:-}" ]; then
        suite "mysql driver network bounds" cargo test -p moqui-drivers --features mysql --lib -- --ignored mysql::
    fi
    suite_summary accept-all
}

# Every regression suite: the framework's, then the applications'.
task_test_all() {
    parse_suite_opts test-all "$@"
    ensure_vault_wasm
    framework_suites
    apps_suites
    suite_summary test-all
}

# The client crates test-clients runs, in its order: name|directory. moqui-client is a workspace member; the others are
# workspaces of their own (Tauri and CEF stay out of the server builds), which cargo test --workspace does not reach.
CLIENT_CRATES='client|crates/moqui-client
tauri|crates/moqui-client-tauri
mobile|crates/moqui-mobile
desktop|crates/moqui-desktop'

# client_crate_named <name> <directory>: whether CLIENT_NAMES is empty or names the crate, by its short name or its
# crate's.
client_crate_named() {
    local name
    [ ${#CLIENT_NAMES[@]} -gt 0 ] || return 0
    for name in "${CLIENT_NAMES[@]}"; do
        [ "$name" != "$1" ] && [ "$name" != "${2##*/}" ] || return 0
    done
    return 1
}

# The app crates whose Cargo.lock is committed (SEC-CLI-104): the mobile app and the Tauri glue. The desktop app's is
# not, as it links the generated target/moqui-app, which differs from one checkout to the next.
LOCKED_APPS="crates/moqui-client-tauri crates/moqui-mobile"

# Whether the app crate in $1 (a path under the root) keeps a committed lock.
locked_app() {
    case " $LOCKED_APPS " in *" $1 "*) return 0 ;; esac
    return 1
}

# The lock of the app crate in $1 (a path under the root), before a build: a committed one as it is, seeded from the
# repository's only when it is missing (./moq update-app-locks brings the repository's versions in); the desktop app's
# copied from the repository's whenever that is newer, so the crates it shares are the reviewed versions.
seed_app_lock() {
    local dir=$1
    if locked_app "$dir"; then
        [ -f "$ROOT/$dir/Cargo.lock" ] || run_cmd cp "$ROOT/Cargo.lock" "$ROOT/$dir/Cargo.lock" \
            || die "could not copy Cargo.lock to $dir"
        return 0
    fi
    if [ ! -f "$ROOT/$dir/Cargo.lock" ] || [ "$ROOT/Cargo.lock" -nt "$ROOT/$dir/Cargo.lock" ]; then
        run_cmd cp "$ROOT/Cargo.lock" "$ROOT/$dir/Cargo.lock" || die "could not copy Cargo.lock to $dir"
    fi
}

# Each package of the lock file $1, one a line: name, version and source (- for a path).
lock_packages() {
    awk '/^\[\[package\]\]/ { if (n != "") print n, v, s; n = ""; v = ""; s = "-" }
         /^name = / { n = $3; gsub(/"/, "", n) }
         /^version = / { v = $3; gsub(/"/, "", v) }
         /^source = / { s = $3; gsub(/"/, "", s) }
         END { if (n != "") print n, v, s }' "$1"
}

# The crates.io packages of the app lock $1 at another version than the repository's lock has in the same semver
# range: name, the app's version and the repository's, one a line. crates/moqui-client/tests/app_workspaces.rs makes
# the same check on every push.
app_lock_drift() {
    awk 'function range(version, part) {
             split(version, part, /[.+-]/)
             return part[1] > 0 ? part[1] : (part[2] > 0 ? "0." part[2] : "0.0." part[3])
         }
         FNR == NR { if ($3 ~ /^registry/) root[$1 " " range($2)] = $2; next }
         $3 ~ /^registry/ { key = $1 " " range($2); if ((key in root) && root[key] != $2) print $1, $2, root[key] }' \
        <(lock_packages "$ROOT/Cargo.lock") <(lock_packages "$1")
}

# The repository's versions of the crates the committed app locks share with it, each brought in by cargo update
# --precise, so the apps build what the workspace reviewed and the rest of their locks stays (SEC-CLI-104).
task_update_app_locks() {
    [ $# -eq 0 ] || die "update-app-locks takes no arguments"
    need_cargo
    local dir name old new changed=
    for dir in $LOCKED_APPS; do
        seed_app_lock "$dir"
        while read -r name old new; do
            [ -n "$name" ] || continue
            info "$dir: $name $old -> $new" >&2
            run_cmd cargo update --manifest-path "$ROOT/$dir/Cargo.toml" -p "$name@$old" --precise "$new" \
                || die "could not update $name in $dir/Cargo.lock"
            changed=1
        done < <(app_lock_drift "$ROOT/$dir/Cargo.lock")
    done
    [ -n "$changed" ] || info "The app locks have the repository's versions of the crates they share"
}

help_update_app_locks() {
    cat <<EOF
Brings the repository's versions of the crates the committed app locks share with it into them:
crates/moqui-client-tauri/Cargo.lock and crates/moqui-mobile/Cargo.lock, each crates.io package that
the repository's Cargo.lock has at another version in the same semver range, by cargo update
--precise, so the apps build the versions the workspace reviewed and the rest of their locks stays.
crates/moqui-client/tests/app_workspaces.rs fails on every push while one differs. Run it after the
repository's Cargo.lock changes, then commit the app locks. The desktop app's lock is not committed
(it links the generated target/moqui-app) and is copied from the repository's at each build.
  ./moq update-app-locks
EOF
}

# The apps' dependencies checked against the RustSec advisory database (cargo deny), for the targets they build for:
# their crates are outside the workspace, whose deny.toml covers it for one Linux target (SEC-CLI-104).
task_deny_apps() {
    [ $# -eq 0 ] || die "deny-apps takes no arguments"
    need_cargo
    command -v cargo-deny >/dev/null 2>&1 || dry_run || die "no cargo-deny: cargo install --locked cargo-deny"
    local line dir target failed=
    for line in "crates/moqui-client-tauri|x86_64-unknown-linux-gnu" "crates/moqui-mobile|aarch64-linux-android" \
        "crates/moqui-desktop|aarch64-apple-darwin" "crates/moqui-desktop|x86_64-unknown-linux-gnu"; do
        dir=${line%%|*}
        target=${line#*|}
        seed_app_lock "$dir"
        [ "$dir" != crates/moqui-desktop ] || write_desktop_app_crate
        info "cargo deny: $dir for $target" >&2
        run_cmd cargo deny --manifest-path "$ROOT/$dir/Cargo.toml" --target "$target" check advisories sources \
            || failed="$failed $dir ($target)"
    done
    [ -z "$failed" ] || die "cargo deny found advisories, or a source it does not take, in:$failed"
}

help_deny_apps() {
    cat <<EOF
Checks the dependencies of the mobile app, the desktop app and the Tauri glue against the RustSec
advisory database and the allowed sources (cargo deny check advisories sources, with the repository's
deny.toml), each for the targets it builds for: their crates are outside the workspace, whose own
check covers the repository's crates. Needs cargo-deny (cargo install --locked cargo-deny) and the
network. Not the bans or licenses checks: the apps' Tauri stack brings ring and MPL-2.0 crates that
the workspace refuses.
  ./moq deny-apps
EOF
}

# The tests of the client crates, the clients' MASVS proofs among them (docs/security/proofs/clients/README.md).
task_test_clients() {
    local arg line name dir known
    local -a cargo_args
    CLIENT_NAMES=()
    cargo_args=()
    SUITE_RESULTS=()
    SUITE_FAILED=0
    while [ $# -gt 0 ]; do
        arg=$1
        case $arg in
            --nobuild) ;;
            --)
                shift
                cargo_args+=("$@")
                break
                ;;
            -*) die "unknown option $arg (test-clients takes crate names, --nobuild, and cargo test's arguments after --)" ;;
            *)
                known=
                while IFS= read -r line; do
                    [ "$arg" != "${line%%|*}" ] && [ "$arg" != "${line##*/}" ] || known=1
                done <<<"$CLIENT_CRATES"
                [ -n "$known" ] || die "no client crate $arg: client, tauri, mobile or desktop"
                CLIENT_NAMES+=("$arg")
                ;;
        esac
        shift
    done
    local -a crates
    crates=()
    while IFS= read -r line; do
        client_crate_named "${line%%|*}" "${line#*|}" && crates+=("$line")
    done <<<"$CLIENT_CRATES"
    need_cargo
    # the crates outside the workspace build into the repository's target/ too, as ./moq desktop and android build them
    export_var CARGO_TARGET_DIR "$ROOT/target"
    for line in "${crates[@]}"; do
        name=${line%%|*}
        dir=${line#*|}
        if [ "$name" = client ]; then
            suite "cargo test -p moqui-client" cargo test -p moqui-client ${cargo_args[@]+"${cargo_args[@]}"}
            continue
        fi
        # a committed lock as it is (the shared crates' versions are the repository's, ./moq update-app-locks); the
        # desktop app's from the repository's, and cargo adds what the crate alone needs
        seed_app_lock "$dir"
        if [ "$name" = desktop ]; then
            # the desktop app links the framework and the component crates through target/moqui-app, with the
            # embedded Postgres of its node
            ensure_pg_archive
            write_desktop_app_crate
        fi
        suite "cargo test in $dir" cargo test --manifest-path "$dir/Cargo.toml" ${cargo_args[@]+"${cargo_args[@]}"}
    done
    suite_summary test-clients
}


# ========== help ==========

help_test() {
    cat <<EOF
Runs moqui test, the JavaScript specs in each component's test/ directory, with the Default
binary (or the package asked for) and the dev conf. A spec that requires a feature the binary
leaves out (spec({ requires: [...] })) is reported as skipped. A regression spec over 15 seconds is
named at the end of a run; --acceptance runs the acceptance suite (test/acceptance/) instead, only when
asked (./moq accept). Arguments go to moqui test; the moq options below are read first and any other option is
passed on, so moqui's own options work as they are:
  ./moq test mr-mantle
  ./moq test mr-mantle --db snapshot
  ./moq test mr-mantle --spec OrderToCashBasicFlow --feature "ship Sales Order"
  ./moq test mr-mantle --junit build/test-results
  ./moq test mr-mantle mr-marble --spec SimpleScreensRender --serve 127.0.0.1:18081
                                    after the specs pass, serve their database until Ctrl-C

Options (as for run): --lite, --debug, --release, --enterprise, -c FILE, -p, -D name=value, --nobuild, and
  --  End of moq options; everything after it goes to moqui test
EOF
}

help_test_client() {
    cat <<EOF
Runs moqui test-client, the client specs in each component's test/client/ directory, against a
server that is already running (./moq run in another terminal), over HTTP, WebSocket and headless
Chromium (moqui_conf__screen_facade__pdf__chromium_path, else the one of ./moq chromium-download,
else an installed one). It opens no database, binds no port and writes only its output directory,
so it runs beside the server in
the same runtime directory. Arguments go to moqui test-client; the moq options below are read first
and any other option is passed on:
  ./moq test-client framework
  ./moq test-client framework -p 8081          the server at http://localhost:8081
  ./moq test-client framework --spec SecurityAuthn --param user=john.doe
  ./moq test-client --server https://test.example.com --insecure
  ./moq test-client framework --serve          start the server its test/client/suite.js declares
                                               (a child moqui test --serve), run, then stop it
  ./moq test-client mr-marble --serve --spec FindScreens --param screen=Order/FindOrder --headed
--acceptance runs the acceptance client specs (test/acceptance/client/) instead, only when asked
(./moq accept). --proofs runs the security proofs (test/proofs/) instead and writes proofs.json and
proofs.md, the results by requirement; against a server it did not start the profile is deployment
(no fixture user signs in), with --serve fixtures (docs/security/proofs/README.md):
  ./moq test-client framework --proofs --serve
  ./moq test-client framework --proofs --server https://test.example.com --param user=auditor

Options: --lite, --debug, --release, --enterprise, --nobuild, and
  -p, --port N  The server at http://localhost:N (--server); the default is http://localhost:8080
  --            End of moq options; everything after it goes to moqui test-client
EOF
}

help_test_load() {
    cat <<EOF
Runs moqui test-load, a load script of a component (test/load/<Name>.load.js), against a server that
is already running (./moq run in another terminal, on conf/MoquiLoadTestConf.xml for the demo data's
tarpits). With no --script it lists the component's scripts. The result goes to
runtime/tmp/test-load/<run>/result.json, where Tools > Load > Runs lists it; Ctrl-C stops the users,
writes the result and exits 130. --out, the --compare files and a path target are read from the
directory moq runs in. The guide is docs/tools/Load-Runner.md. Arguments go to moqui test-load; the
moq options below are read first and any other option is passed on:
  ./moq test-load mr-marble                                  list the scripts and their params
  ./moq test-load mr-marble --script OrderToCash -p 8081     against http://localhost:8081
  ./moq test-load mr-marble --script OrderToCash --param pdf=false --users 20 --label "MR"
  ./moq test-load --compare a/result.json b/result.json

Options: --lite, --debug, --release, --enterprise, --nobuild, and
  -p, --port N  The server at http://localhost:N (--server); the default is http://localhost:8080
  --            End of moq options; everything after it goes to moqui test-load
EOF
}

help_test_framework() {
    cat <<EOF
Runs the framework's regression tests in turn, each whatever the ones before it did, then lists which
passed and exits 1 when any failed. For work on the framework: crates/, runtime/components/base,
runtime/components/dev, moq or the scripts.
  cargo test --workspace            the unit and integration tests of every crate
  scripts/moq-test.sh               the tests of this script
  moqui test <components> --workers 4
                                    the specs of components/base and components/dev (test/*.spec.js),
                                    on 4 workers (MOQ_TEST_WORKERS)
  moqui test demo                   the demo component's specs, with -D demo_disabled=false (it is off
                                    by default)
  moqui test-client webroot --offline
  moqui test-client runtime/components/dev/dev-aide --offline
                                    the offline client specs: pages at https://moqui.test, no server
  moqui test-client --serve <components>
                                    the client specs of components/base and components/dev whose
                                    test/client/suite.js declares a server, each against it
  moqui test-client --serve --proofs <components>
                                    their security proofs (test/proofs/), each against the server its
                                    test/proofs/suite.js declares, with the summary by requirement
None of them runs an acceptance test (docs/Build-Test.md#the-two-suites). The Chromium is the one PDF
and moqui test-client use: moqui_conf__screen_facade__pdf__chromium_path, else runtime/chromium
(./moq chromium-download), else an installed Chromium, Google Chrome or Edge.
Left out: the timings (cargo test --release --test entity_speed and the other *_speed tests) and
Clippy over every feature set (scripts/check-packages.sh, which runs tui_screens too).

Options:
$PACKAGE_OPTIONS_HELP
      --nobuild        Skip the build before moqui test and test-client
EOF
}

help_test_apps() {
    cat <<EOF
Runs the application tests, the regression suite of the components under runtime/components/addon, then lists
which passed and exits 1 when any failed. The command for application work, before a commit:
  moqui test <components> --workers 4
                                    the specs (test/*.spec.js) of each component under
                                    runtime/components/addon that has them, on 4 workers
                                    (MOQ_TEST_WORKERS)
  moqui test-client --serve <components>
                                    their client specs, each against the server its
                                    test/client/suite.js declares
  moqui test-client --serve --proofs <components>
                                    their security proofs (test/proofs/), each against the server
                                    its test/proofs/suite.js declares
No cargo build or test, no moq-test.sh, and no components/base or components/dev spec. The binary is
built only when it is missing (as --nobuild).

Options:
$PACKAGE_OPTIONS_HELP
      --nobuild        Accepted; the binary is built only when it is missing in any case
EOF
}

help_test_all() {
    cat <<EOF
Runs the whole regression suite: the suites of test-framework, then those of test-apps (./moq help
test-framework, ./moq help test-apps), each whatever the ones before it did, then lists which passed
and exits 1 when any failed. Before a release, after a framework change that apps depend on, or when
asked; not for application work. The acceptance suite is not part of it (./moq accept-all).

Options:
$PACKAGE_OPTIONS_HELP
      --nobuild        Skip the build before moqui test and test-client
EOF
}

help_test_clients() {
    cat <<EOF
Runs the tests of the client crates on this machine, each whatever the ones before it did, then lists
which passed and exits 1 when any failed: the desktop and mobile apps, the Tauri glue they share and
the client under all three, the clients' MASVS proofs among them (docs/security/proofs/clients/README.md).
All four by default, or those named:
  client    cargo test -p moqui-client          a workspace member, which test-framework runs too
  tauri     cargo test in crates/moqui-client-tauri
  mobile    cargo test in crates/moqui-mobile   the app's host logic, on this machine's webview
  desktop   cargo test in crates/moqui-desktop  after writing target/moqui-app (the crate that links the
                                                framework and the component crates), as ./moq desktop does
The last three are workspaces of their own, outside the repository's, so cargo test --workspace and
test-framework do not reach them. The glue's and the mobile app's Cargo.lock are committed and used as
they are (./moq update-app-locks brings the repository's versions in); the desktop app's starts from the
repository's when it has none or an older one. Each builds into target/. A run of its own, not part of test-framework or test-all: it
builds Tauri, and for the desktop app CEF and the framework again; the first run downloads CEF (about
1 GB) and needs the desktop app's prerequisites (docs/Build-Test.md#desktop-app). For a change to these
crates, or to what they link from the workspace, before a commit.
  ./moq test-clients
  ./moq test-clients mobile tauri
  ./moq test-clients desktop -- updates       a cargo test filter, for each crate run
Not run: the Android build (./moq android, scripts/check-android.sh), the Kotlin tests and the device
tests (crates/moqui-mobile/README.md).

Options:
      --nobuild        Accepted; cargo test builds what changed in any case
      --               End of moq options; the rest goes to each cargo test
EOF
}

help_accept() {
    cat <<EOF
Runs the acceptance suite of the components named, or of every component: moqui test --acceptance
over those with test/acceptance/*.spec.js, then moqui test-client --serve --acceptance over those whose
test/acceptance/client/suite.js declares a server, and moqui test-client --offline --acceptance over those whose suite.js
declares none. Then it lists which passed and exits 1 when any failed.
Acceptance tests run on a schedule, before a release, or when asked; an agent runs them only when
the user asks, or when it adds or changes one (AGENTS.md).
  ./moq accept mr-boss
  ./moq accept mr-boss --spec AppStructure     one spec (with what its dependsOn names), in
                                               the tree it is in
  ./moq accept --junit build/acceptance        a dated directory of JUnit XML under it

Options:
  --spec Name[,Name]   Only these specs
  --junit dir          JUnit XML into <dir>/<yyyyMMdd-HHmmss>
$PACKAGE_OPTIONS_HELP
      --nobuild        Skip the build before moqui test and test-client
EOF
}

help_accept_all() {
    cat <<EOF
Runs every acceptance test: ./moq accept over every component, then the framework's Rust acceptance
tests, which no regression command runs:
  cargo test -p moqui-host --test gate3 --test client_browser --test tui_screens
                                    the opt-in host tests (each builds its own feature set first)
  cargo test -p moqui-host --test gate3_browser
  cargo test-slow                   the host's #[ignore] tests
  cargo test -p moqui --test cli -- --ignored
  cargo test -p moqui-drivers --features mysql --lib -- --ignored mysql::
                                    only with MOQUI_TEST_MYSQL=user:pass@host:port/database: the network
                                    bounds of the MySQL driver against that MySQL or MariaDB server
For a scheduled runner and before a release, or when the user asks.

Options:
  --junit dir          JUnit XML of the specs into <dir>/<yyyyMMdd-HHmmss>
$PACKAGE_OPTIONS_HELP
      --nobuild        Skip the build before moqui test and test-client
EOF
}

help_autodoc() {
    cat <<EOF
Runs moqui autodoc: the Markdown docs of the artifacts, with Mermaid diagrams, written from a model-only
boot (no database) into a directory, by default ./autodoc in the directory moq is run from. The paths
of --out, --templates, --project and --java are read from that directory too; every other option goes
to moqui autodoc as it is (moqui help autodoc has them all, and docs/dev/Autodoc.md the tree).
  ./moq autodoc                                   every artifact, into ./autodoc
  ./moq autodoc --components my-site --clean      one component; --clean removes docs it wrote before
                                                  whose artifacts are gone
  ./moq autodoc --kinds entity --packages mantle.order --out order-docs

Options:
$PACKAGE_OPTIONS_HELP
      --nobuild        Skip the build (./moq build) that runs first. A binary that is not there is still built
      --               End of moq options; the rest goes to moqui autodoc, its paths as given
EOF
}
