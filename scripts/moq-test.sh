#!/usr/bin/env bash
# Tests for ./moq in a throwaway copy of the repo layout: a stub moqui binary, a small runtime, and local git and zip
# "remotes" for the component tasks. Nothing outside the temp directory is touched.
#   scripts/moq-test.sh
# Runs under bash 3.2 too:
#   docker run --rm -v "$PWD":/w -w /w bash:3.2 bash scripts/moq-test.sh
# Parts that need git, zip, unzip, or curl are skipped when those are missing.
set -o pipefail

# Every command below belongs to the throwaway fixture. A caller's runtime can contain directory symlinks into a
# live workspace, and its worker override changes the defaults these checks exercise. Explicit env overrides on
# individual checks still work; unsetting these here affects only this self-test process and its children.
unset MOQUI_RUNTIME MOQ_TEST_WORKERS VAULT_WASM_ENSURED

SRC=$(cd "$(dirname "$0")/.." && pwd)
# The physical path: moq resolves its root with cd -P, so its output names /private/var/... on macOS, where TMPDIR is
# /var/folders/.../T/ (a symlink, and a trailing slash).
FIX=$(cd "$(mktemp -d "${TMPDIR:-/tmp}/moq-test.XXXXXX")" && pwd -P)
trap 'rm -rf "$FIX"' EXIT
pass=0
fail=0
skipped=

ok() {
    pass=$((pass + 1))
}

bad() {
    fail=$((fail + 1))
    printf 'FAIL: %s\n' "$1"
    [ -z "${2:-}" ] || printf '%s\n' "$2" | sed 's/^/    | /'
}

# check <name> <expected exit> <grep -E pattern for the output, or ""> -- command...
check() {
    local name=$1 want=$2 pattern=$3 out status
    shift 4
    out=$("$@" 2>&1)
    status=$?
    if [ "$status" != "$want" ]; then
        bad "$name: exit $status, wanted $want" "$out"
    elif [ -n "$pattern" ] && ! printf '%s\n' "$out" | grep -Eq -- "$pattern"; then
        bad "$name: no match for /$pattern/" "$out"
    else
        ok
    fi
}

# lacks <name> <grep -E pattern> -- command...: the output must not match.
lacks() {
    local name=$1 pattern=$2 out
    shift 3
    out=$("$@" 2>&1)
    if printf '%s\n' "$out" | grep -Eq -- "$pattern"; then
        bad "$name: unwanted match for /$pattern/" "$out"
    else
        ok
    fi
}

have() {
    command -v "$1" >/dev/null 2>&1
}

# Run a shell command with stdout on a pseudo-terminal, so git will page. Prints the output.
# Returns 127 when script cannot provide a terminal (GNU script, then the macOS one).
run_on_tty() {
    local probe
    if ! have script; then
        return 127
    fi
    probe=$(script -q -c 'tty' /dev/null 2>&1) || probe=
    if printf '%s\n' "$probe" | grep -Eq '/dev/(pts|ttys|pty)/'; then
        script -q -c "$1" /dev/null
        return $?
    fi
    probe=$(script -q /dev/null tty 2>&1) || probe=
    if printf '%s\n' "$probe" | grep -Eq '/dev/(pts|ttys|pty)/'; then
        script -q /dev/null bash -c "$1"
        return $?
    fi
    return 127
}

# ========== fixture ==========

R=$FIX/root
mkdir -p "$R/target/release" "$R/runtime/components/base/framework/data" "$R/runtime/components/base/framework/conf" \
    "$R/runtime/conf" "$R/runtime/components/addon" \
    "$R/runtime/install/postgres/18.6.0" "$R/runtime/db/install/18.6.0" "$R/runtime/log" "$R/runtime/tmp" "$R/runtime/opensearch/data/nodes" "$R/runtime/opensearch/logs"
cp "$SRC/moq" "$R/moq"
chmod +x "$R/moq"
mkdir -p "$R/scripts/moq"
cp "$SRC"/scripts/moq/*.sh "$R/scripts/moq/"
cp "$SRC/scripts/js-state.pl" "$R/scripts/"
# Minimal verified webroot; CLI command tests run offline with the stub binary.
webroot="$R/runtime/components/base/webroot"
mkdir -p "$webroot/build" "$webroot/screen/webroot/libs"
touch "$webroot/screen/webroot/libs/NOTICES.md"
printf '{"output": "screen/webroot/libs", "projects": [], "script": "noop.sh"}\n' >"$webroot/build/js-build.json"
: >"$webroot/build/noop.sh"
echo '{"notices":{"path":"NOTICES.md","sha256":"e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"},"libraries":[]}' >"$webroot/screen/webroot/libs/MANIFEST.json"
mkdir -p "$R/crates/moqui-embedded" "$R/.cargo"
cp "$SRC/crates/moqui-embedded/archive-excludes.txt" "$R/crates/moqui-embedded/"
cp "$SRC/.cargo/config.toml" "$R/.cargo/"
cat >"$R/target/release/moqui" <<'EOF'
#!/bin/sh
# moqui stub: records its arguments and env, and fakes save-db / load-save.
echo "stub: $* conf=${MOQUI_CONF:-}" >>"$(dirname "$0")/calls.log"
case $1 in
    embedded) [ "$2" = checksum ] && echo "0000000000000000000000000000000000000000000000000000000000000000  runtime" ;;
    save-db) echo "dump of $2" >"$3" ;;
    conf) [ "$2" = env ] && printf '%s\n' moqui_conf__instance__purpose moqui_conf__entity_facade__datasource__TRANSACTIONAL__database_conf_name ;;
    load-save) [ -f "$3" ] || exit 3; mkdir -p "$2/data"; cp "$3" "$2/data/restored" ;;
esac
EOF
chmod +x "$R/target/release/moqui"
mkdir -p "$R/bin"
cat >"$R/bin/cargo" <<'EOF'
#!/bin/sh
# cargo stub: records its arguments, builds the stub moqui for the profile, and fails when CARGO_STUB_FAIL is set.
echo "cargo: $*" >>"$(dirname "$0")/../cargo.log"
echo "wrapper: ${RUSTC_WRAPPER-unset}" >>"$(dirname "$0")/../wrapper.log"
[ -z "${CARGO_STUB_FAIL:-}" ] || exit 1
[ "$1" != clean ] || exit 0
p=debug
while [ $# -gt 0 ]; do
    case $1 in --release) p=release ;; --profile) p=$2; shift ;; esac
    shift
done
[ "$p" != dev ] || p=debug
mkdir -p "$(dirname "$0")/../target/$p"
cp "$(dirname "$0")/../target/release/moqui.keep" "$(dirname "$0")/../target/$p/moqui"
EOF
chmod +x "$R/bin/cargo"
cp "$R/target/release/moqui" "$R/target/release/moqui.keep"
export PATH="$R/bin:$PATH"
echo '<moqui-conf/>' >"$R/runtime/conf/MoquiDevConf.xml"
echo '<moqui-conf/>' >"$R/runtime/conf/MoquiProductionConf.xml"
echo '<moqui-conf/>' >"$R/runtime/components/base/framework/conf/MoquiDefaultConf.xml"
echo '<entity-facade-xml type="seed">' >"$R/runtime/components/base/framework/data/A.xml"
echo '<entity-facade-xml type="seed-initial,demo">' >"$R/runtime/components/base/framework/data/B.xml"
echo 'log' >"$R/runtime/log/moqui.log"
echo 'index' >"$R/runtime/opensearch/data/nodes/x"
echo 'gc' >"$R/runtime/opensearch/logs/gc.log"
echo 'id' >"$R/runtime/db/instance_id"

M=$R/moq
dry() {
    MOQ_DRY_RUN=1 "$@"
}
CALLS=$R/target/release/calls.log

# ========== matching and help ==========

check "no args lists tasks" 1 'run-production' -- "$M"
check "help" 0 'component-zip' -- "$M" help
check "exact" 0 'exec .*/target/release/moqui$' -- dry "$M" run
check "segment prefix r-p" 0 'target/release/moqui' -- dry "$M" r-p
check "camelCase runProduction" 0 'MOQUI_CONF=.*MoquiProductionConf.xml' -- dry "$M" runProduction
check "upper case RUN" 0 'target/release/moqui' -- dry "$M" RUN
check "ambiguous l" 2 'ambiguous: load load-production load-save' -- "$M" l
check "unknown task" 1 'no task zzz' -- "$M" zzz
check "suggestion" 1 'did you mean: .*opensearch-start' -- "$M" open-stat
check "alias moqui" 0 'target/release/moqui$' -- dry "$M" moqui
check "alias with args loadSeed" 0 'load types=seed' -- dry "$M" loadSeed
check "gradle abbreviation gPA" 0 './moq git-pull' -- "$M" gPA --help
check "gradle abbreviation zC" 0 './moq component-zip' -- "$M" zC --help
check "ambiguous alias gC" 2 'ambiguous: .*get-component' -- "$M" gC
check "task --help" 0 'Options:' -- "$M" run --help
check "help partial" 0 './moq component-create' -- "$M" help c-cr

# A trust dry run and removal without an existing issuer touch no store or runtime.
check "trust supports system removal" 0 'untrust' -- "$M" help trust-dev-cert
# a real --untrust with no root removes development roots by name from this user's stores, so only its dry run here
check "missing shared root is untrusted by name" 0 'every dev CA root named' -- \
    env MOQ_DRY_RUN=1 MOQUI_SHARED_RUNTIME="$FIX/no-signer" "$M" trust-dev-cert --untrust
check "missing shared root stays absent" 1 '' -- test -e "$FIX/no-signer"
check "a relative shared path cannot install trust" 1 'absolute directory' -- \
    env MOQUI_SHARED_RUNTIME=relative "$M" trust-dev-cert
check "trust dry run" 0 'would update dev CA trust' -- \
    env MOQ_DRY_RUN=1 MOQUI_SHARED_RUNTIME="$FIX/no-signer" "$M" trust-dev-cert --system

# ========== run and load ==========

check "run options" 0 'export moqui_conf__webapp_list__webapp__WEBROOT__http_port=8081' -- dry "$M" run -p 8081 -D moqui_conf__instance__purpose=test lint --screens
check "run -D" 0 'export moqui_conf__entity_facade__datasource__TRANSACTIONAL__database_conf_name=postgres' -- dry "$M" run -D moqui_conf__entity_facade__datasource__TRANSACTIONAL__database_conf_name=postgres
check "run -Dname=value" 0 'export a_b=c' -- dry "$M" run -Da_b=c
check "run passthrough" 0 'moqui lint --screens$' -- dry "$M" run -p 8081 lint --screens
check "run -- passthrough" 0 'moqui -x$' -- dry "$M" run -- -x
check "run unknown option" 1 'unknown option -x' -- dry "$M" run -x
check "run dotted property" 1 'underscore form.*a_b' -- dry "$M" run -D a.b=1
check "run bad port" 1 'not a number' -- dry "$M" run -p abc
check "conf in runtime" 0 "MOQUI_CONF=$R/runtime/conf/MoquiProductionConf.xml" -- dry "$M" run -c conf/MoquiProductionConf.xml
check "conf from cwd" 0 "MOQUI_CONF=$R/runtime/conf/MoquiDevConf.xml" -- \
    sh -c "cd '$R/runtime/conf' && MOQ_DRY_RUN=1 '$M' run --conf=MoquiDevConf.xml"
mkdir -p "$FIX/override-runtime/conf"
cp "$R/runtime/conf/MoquiDevConf.xml" "$FIX/override-runtime/conf/Override.xml"
check "explicit runtime override resolves its conf" 0 "MOQUI_CONF=$FIX/override-runtime/conf/Override.xml" -- \
    env MOQUI_RUNTIME="$FIX/override-runtime" MOQ_DRY_RUN=1 "$M" run -c conf/Override.xml
check "conf missing" 1 'conf file nope.xml not found' -- dry "$M" run -c nope.xml
check "run-production --debug" 0 'target/debug/moqui$' -- dry "$M" run-production --debug
check "build default" 0 'cargo build --profile release --no-default-features --features maximal\\?,devtools -p moqui$' -- dry "$M" build
check "build --debug" 0 'cargo build --profile dev --no-default-features --features maximal\\?,devtools -p moqui$' -- dry "$M" build --debug
check "build --lite" 0 'cargo build --profile lite --no-default-features --features lite -p moqui$' -- dry "$M" build --lite
check "run --lite" 0 'target/lite/moqui$' -- dry "$M" run --lite
check "build --release" 0 'cargo build --profile release --no-default-features --features maximal\\?,devtools -p moqui$' -- dry "$M" build --release
check "build --enterprise" 0 'cargo build --profile enterprise --no-default-features --features enterprise -p moqui$' -- \
    dry "$M" build --enterprise
check "build two packages" 1 'ask for two packages' -- dry "$M" build --release --enterprise
check "build passes cargo args" 0 'moqui --features x$' -- dry "$M" build --debug -- --features x
check "run --enterprise" 0 'target/enterprise/moqui$' -- dry "$M" run --enterprise
check "build --embed" 0 'features maximal\\?,devtools\\?,embed-production -p moqui$' -- dry "$M" build --embed production
check "build --embed=all with the programs" 0 'features lite\\?,embed-all\\?,embed-opensearch\\?,embed-chromium -p moqui$' -- \
    dry env MOQUI_OPENSEARCH_ARCHIVE=/x MOQUI_CHROMIUM_ARCHIVE=/y "$M" build --lite --embed=all --embed-opensearch --embed-chromium
check "build --embed framework adds no feature" 0 'features enterprise -p moqui$' -- dry "$M" build --enterprise --embed framework
check "build --embed needs a known level" 1 'takes framework, production or all' -- dry "$M" build --embed most
check "build two embed levels" 1 'ask for two levels' -- dry "$M" build --embed all --embed production
check "run --embed" 0 'embed-production -p moqui$' -- dry "$M" run --embed production
if [ "$(uname -s)" = Darwin ]; then
    check "build --embed-opensearch explains the missing macOS archive" 1 'OpenSearch publishes no macOS tarball' -- \
        env CARGO_NET_OFFLINE=false MOQUI_EMBED_CACHE="$FIX/embed-cache" MOQ_DRY_RUN=1 "$M" build --embed-opensearch
else
    check "build --embed-opensearch gets the archive first" 0 'opensearch-3\.[0-9.]*-.*\.tar\.gz' -- \
        env CARGO_NET_OFFLINE=false MOQUI_EMBED_CACHE="$FIX/embed-cache" MOQ_DRY_RUN=1 "$M" build --embed-opensearch
fi
check "offline build names the missing OpenSearch archive" 1 'offline build needs the OpenSearch archive' -- \
    env MOQUI_EMBED_CACHE="$FIX/embed-cache" MOQ_DRY_RUN=1 "$M" build --offline --embed-opensearch
check "complete build --embed" 0 '^production$' -- "$M" __complete 3 build --embed p
rm -f "$R/cargo.log"
check "run --debug builds first" 0 '' -- "$M" run --debug
check "cargo saw the debug build" 0 'cargo: build --profile dev --no-default-features --features maximal,devtools -p moqui$' -- cat "$R/cargo.log"
check "debug binary exists now" 0 '' -- test -x "$R/target/debug/moqui"
rm -f "$R/cargo.log"
check "run builds first with the binary there" 0 '' -- "$M" run
check "cargo saw the default build" 0 'cargo: build --profile release --no-default-features --features maximal,devtools -p moqui$' -- cat "$R/cargo.log"
rm -f "$R/cargo.log"
check "run --nobuild with the binary there does not build" 0 '' -- "$M" run --nobuild
check "no cargo call" 1 '' -- test -f "$R/cargo.log"
check "--nobuild says which binary it uses" 0 'using target/release/moqui' -- "$M" run --nobuild
# Bash 3.2 compares whole seconds for -nt. Give the mock binaries distinct mtimes.
touch -t 202001010000 "$R/target/release/moqui"
touch -t 202001010001 "$R/target/debug/moqui"
check "--nobuild with no package takes the newer debug binary" 0 'using target/debug/moqui' -- "$M" run --nobuild
check "--release takes the release binary whatever is newer" 0 'using target/release/moqui' -- "$M" run --nobuild --release
mkdir -p "$R/crates/stale-check/src"
touch "$R/crates/stale-check/src/lib.rs"
check "--nobuild warns of a source newer than the binary" 0 'older than crates/stale-check/src/lib.rs' -- "$M" run --nobuild
rm -rf "$R/crates/stale-check"
touch "$R/target/release/moqui"
check "run --build is gone" 1 'unknown option --build' -- "$M" run --build
rm -f "$R/target/release/moqui" "$R/target/debug/moqui" "$R/cargo.log"
check "--nobuild still builds a missing binary" 0 'is not built, building it' -- "$M" run --nobuild
check "cargo saw the missing build" 0 'cargo: build --profile release --no-default-features --features maximal,devtools -p moqui$' -- cat "$R/cargo.log"
rm -f "$R/target/release/moqui" "$R/cargo.log"
check "dry run shows the build" 0 'cargo build --profile release --no-default-features --features maximal\\?,devtools -p moqui' -- dry "$M" load
check "dry run does not call cargo" 1 '' -- test -f "$R/cargo.log"
check "failed build" 1 'cargo build failed' -- env CARGO_STUB_FAIL=1 "$M" load
"$R/bin/cargo" build --profile release -p moqui
rm -f "$R/cargo.log"

# The vault's browser module, built on demand by scripts/vault-wasm.sh: a stub that records its call in cargo.log, so
# its order against cargo shows, and fails when VAULT_STUB_FAIL is set. Without the script (the fixture above) nothing.
cat >"$R/scripts/vault-wasm.sh" <<'EOF2'
#!/bin/sh
echo "vault-wasm: $*" >>"$(dirname "$0")/../cargo.log"
[ -z "${VAULT_STUB_FAIL:-}" ] || { echo "vault-wasm.sh: no target" >&2; exit 1; }
EOF2
check "build makes the vault module before cargo" 0 '' -- "$M" build --debug
check "the vault module first, then cargo" 0 '^vault-wasm: --quiet cargo: build --profile dev ' -- \
    sh -c "paste -sd' ' '$R/cargo.log'"
check "dry build shows the vault module" 0 'vault-wasm\.sh --quiet' -- dry "$M" build --embed production
check "build stops when the vault module does not build" 1 "could not build the vault's browser module" -- \
    env VAULT_STUB_FAIL=1 "$M" build --debug
rm -f "$R/cargo.log"
check "run --nobuild warns when the vault module does not build" 0 "could not build the vault's browser module" -- \
    env VAULT_STUB_FAIL=1 "$M" run --nobuild
check "run --nobuild makes the vault module" 0 '^vault-wasm: --quiet$' -- cat "$R/cargo.log"
check "test-framework makes the vault module once" 0 '^1$' -- \
    sh -c "MOQ_DRY_RUN=1 '$M' test-framework 2>&1 | grep -c 'vault-wasm\.sh'"
check "archive stops when the vault module does not build" 1 "could not build the vault's browser module" -- \
    sh -c "cd '$FIX' && VAULT_STUB_FAIL=1 '$M' archive --binary --nobuild '$FIX/vault-archive.gz'"
check "no archive without the vault module" 1 '' -- test -e "$FIX/vault-archive.gz"
rm -f "$R/scripts/vault-wasm.sh" "$R/cargo.log"

# sccache as the rustc wrapper, opt-in with MOQ_SCCACHE=1: a stub sccache on the PATH, a cargo home of its own, and
# no wrapper from the caller.
mkdir -p "$FIX/sccache-bin" "$FIX/cargo-home"
printf '#!/bin/sh\nexec "$@"\n' >"$FIX/sccache-bin/sccache"
chmod +x "$FIX/sccache-bin/sccache"
nowrap() {
    env -u RUSTC_WRAPPER -u CARGO_BUILD_RUSTC_WRAPPER -u MOQ_SCCACHE CARGO_HOME="$FIX/cargo-home" "$@"
}
withsc() {
    nowrap PATH="$FIX/sccache-bin:$PATH" "$@"
}
lacks "sccache is off by default" 'RUSTC_WRAPPER|sccache' -- withsc MOQ_DRY_RUN=1 "$M" build
lacks "MOQ_SCCACHE=0 is off" 'RUSTC_WRAPPER|sccache' -- withsc MOQ_SCCACHE=0 MOQ_DRY_RUN=1 "$M" build
rm -f "$R/wrapper.log"
check "build without MOQ_SCCACHE" 0 '' -- withsc "$M" build --debug
check "cargo saw no wrapper" 0 '^wrapper: unset$' -- cat "$R/wrapper.log"
check "MOQ_SCCACHE=1 makes sccache the wrapper" 0 "export RUSTC_WRAPPER=$FIX/sccache-bin/sccache" -- \
    withsc MOQ_SCCACHE=1 MOQ_DRY_RUN=1 "$M" build
rm -f "$R/wrapper.log"
check "build with MOQ_SCCACHE=1" 0 '' -- withsc MOQ_SCCACHE=1 "$M" build --debug
check "cargo saw sccache" 0 "^wrapper: $FIX/sccache-bin/sccache$" -- cat "$R/wrapper.log"
lacks "RUSTC_WRAPPER kept" 'export RUSTC_WRAPPER' -- withsc MOQ_SCCACHE=1 RUSTC_WRAPPER=other MOQ_DRY_RUN=1 "$M" build
lacks "empty RUSTC_WRAPPER kept" 'export RUSTC_WRAPPER' -- withsc MOQ_SCCACHE=1 RUSTC_WRAPPER= MOQ_DRY_RUN=1 "$M" build
lacks "CARGO_BUILD_RUSTC_WRAPPER kept" 'export RUSTC_WRAPPER' -- \
    withsc MOQ_SCCACHE=1 CARGO_BUILD_RUSTC_WRAPPER=other MOQ_DRY_RUN=1 "$M" build
mkdir -p "$FIX/cargo-home-wrapped"
printf '[build]\nrustc-wrapper = "other"\n' >"$FIX/cargo-home-wrapped/config.toml"
lacks "rustc-wrapper in the cargo config kept" 'export RUSTC_WRAPPER' -- \
    withsc MOQ_SCCACHE=1 CARGO_HOME="$FIX/cargo-home-wrapped" MOQ_DRY_RUN=1 "$M" build
if have sccache; then
    skipped="$skipped sccache-missing"
else
    rm -f "$R/wrapper.log"
    check "MOQ_SCCACHE=1, no sccache: the build goes on with a warning" 0 'cargo binstall sccache' -- \
        nowrap MOQ_SCCACHE=1 "$M" build --debug
    check "MOQ_SCCACHE=1, no sccache: no wrapper" 0 '^wrapper: unset$' -- cat "$R/wrapper.log"
    lacks "no sccache, not asked for: no warning" 'sccache' -- nowrap "$M" build --debug
    lacks "no sccache, a wrapper set: no warning" 'sccache' -- nowrap MOQ_SCCACHE=1 RUSTC_WRAPPER=other "$M" build --debug
fi
rm -f "$R/cargo.log" "$R/wrapper.log"
# An add-on component crate builds through the generated crate, a workspace of its own, which takes the repository's
# crate patches (cargo reads them only from the root of the workspace it builds; plans/Security-Review.md SEC-CLI-126).
mkdir -p "$R/runtime/components/addon/crate-x" "$R/patches/crate-p"
printf '<component name="crate-x"/>\n' >"$R/runtime/components/addon/crate-x/component.xml"
printf '[package]\nname = "crate-x"\n' >"$R/runtime/components/addon/crate-x/Cargo.toml"
printf '[workspace]\n\n[patch.crates-io]\n# a patched crate\ncrate-p = { path = "patches/crate-p" }\n\n[profile.release]\nlto = true\n' \
    >"$R/Cargo.toml"
: >"$R/Cargo.lock"
check "build links an add-on component crate" 0 'Linking component crate crate-x' -- "$M" build --debug
check "the generated crate takes the repository's patches" 0 "^crate-p = \\{ path = \"$R/patches/crate-p\" \\}\$" -- \
    cat "$R/target/moqui-app/Cargo.toml"
lacks "the generated crate takes no other table of the repository's" '^lto = true' -- cat "$R/target/moqui-app/Cargo.toml"
rm -rf "$R/runtime/components/addon/crate-x" "$R/patches" "$R/Cargo.toml" "$R/Cargo.lock" "$R/target/moqui-app" "$R/cargo.log"
check "release flag is gone" 1 'unknown option -r' -- "$M" run -r
check "load default all" 0 'moqui load types=all$' -- dry "$M" load
check "load types" 0 'load types=seed\\?,demo components=x$' -- dry "$M" load seed demo components=x
check "load types twice" 1 'not both' -- dry "$M" load seed types=demo
echo '<entity-facade-xml type="seed">' >"$FIX/data.xml"
check "load location from other cwd" 0 "location=$FIX/data.xml" -- sh -c "cd '$FIX' && MOQ_DRY_RUN=1 '$M' load -l data.xml"
check "load location default types" 0 'types=all location=' -- sh -c "cd '$FIX' && MOQ_DRY_RUN=1 '$M' load -l data.xml"
check "load location missing" 1 'data file nope.xml not found' -- dry "$M" load -l nope.xml
# autodoc: the caller's directory is the base, as moq runs the binary from the root.
check "autodoc default out" 0 "moqui autodoc --out $FIX/autodoc$" -- sh -c "cd '$FIX' && MOQ_DRY_RUN=1 '$M' autodoc"
check "autodoc relative out" 0 "autodoc --out $FIX/docs$" -- sh -c "cd '$FIX' && MOQ_DRY_RUN=1 '$M' autodoc --out docs"
check "autodoc out=" 0 "autodoc --out $FIX/docs$" -- sh -c "cd '$FIX' && MOQ_DRY_RUN=1 '$M' autodoc --out=docs"
check "autodoc absolute out" 0 'autodoc --out /x/docs$' -- dry "$M" autodoc --out /x/docs
check "autodoc paths and pass-through" 0 \
    "autodoc --templates $FIX/t --project $FIX/p --kinds entity --clean --out $FIX/autodoc$" \
    -- sh -c "cd '$FIX' && MOQ_DRY_RUN=1 '$M' autodoc --templates t --project=p --kinds entity --clean"
check "autodoc out needs a value" 1 '--out needs a directory' -- dry "$M" autodoc --out
check "load-production default" 0 'moqui load$' -- dry "$M" load-production --debug
check "load-production conf" 0 'MOQUI_CONF=.*MoquiProductionConf.xml' -- dry "$M" load-production --debug seed

# A real (stub) run: env and arguments reach the binary.
rm -f "$CALLS"
"$M" run -c conf/MoquiProductionConf.xml types --out x >/dev/null 2>&1
check "stub saw run" 0 "stub: types --out x conf=$R/runtime/conf/MoquiProductionConf.xml" -- cat "$CALLS"

# ========== completion ==========

check "complete task prefix" 0 'reload-save' -- "$M" __complete 1 r
check "complete task segments" 0 '^run-production$' -- "$M" __complete 1 r-p
lacks "complete task segments only" '^run$' -- "$M" __complete 1 r-p
check "complete conf" 0 '^conf/MoquiProductionConf.xml$' -- "$M" __complete 3 run -c ''
check "complete conf filtered" 0 '^conf/MoquiDevConf.xml$' -- "$M" __complete 3 run -c conf/MoquiD
check "complete conf falls back to files" 0 '^@files' -- "$M" __complete 3 run -c x/y
check "complete location" 0 '^@files' -- "$M" __complete 3 load -l ''
check "complete load types" 0 '^seed-initial$' -- "$M" __complete 2 load se
check "complete load demo" 0 '^demo$' -- "$M" __complete 2 load d
check "complete run binary commands" 0 '^lint$' -- "$M" __complete 2 run ''
check "complete run options" 0 '^--debug$' -- "$M" __complete 2 run --d
check "complete -D property" 0 '^moqui_conf__entity_facade__datasource__TRANSACTIONAL__database_conf_name=$' -- "$M" __complete 3 run -D moqui_conf__ent
check "complete alias task" 0 '^seed$' -- "$M" __complete 2 lP se

# ========== agent harness setups ==========

mkdir -p "$R/.agents/skills/sk1"
printf -- '---\nname: sk1\ndescription: x\n---\n' >"$R/.agents/skills/sk1/SKILL.md"
check "complete harness task" 0 '^claude-code$' -- "$M" __complete 1 claude-c
check "task list has the harnesses on one line" 0 'agent harnesses +claude-code, claude-desktop' -- "$M" help
check "claude-code links a skill" 0 'claude/skills/sk1 -> ../../.agents/skills/sk1' -- "$M" claude-code
check "claude-code made the link" 0 '^../../.agents/skills/sk1$' -- readlink "$R/.claude/skills/sk1"
ln -s ../../.agents/skills/gone "$R/.claude/skills/gone"
check "claude-code removes a link to a gone skill" 0 'removed .claude/skills/gone' -- "$M" claude-code
check "complete help" 0 '^git-status$' -- "$M" __complete 2 help git-s
check "complete autodoc kinds" 0 '^entity$' -- "$M" __complete 3 autodoc --kinds ''
check "complete autodoc options" 0 '^--components$' -- "$M" __complete 2 autodoc --c
check "complete autodoc out" 0 '^@dirs$' -- "$M" __complete 3 autodoc --out ''

# The bash wrapper, driven as readline would.
out=$(cd "$R" && bash -c '
    eval "$(./moq completion bash)"
    COMP_WORDS=(./moq r-p); COMP_CWORD=1; _moq_complete; printf "%s\n" "${COMPREPLY[@]}"
    COMP_WORDS=(./moq load se); COMP_CWORD=2; _moq_complete; printf "%s\n" "${COMPREPLY[@]}"
    COMP_WORDS=(./moq run -c ""); COMP_CWORD=3; _moq_complete; printf "%s\n" "${COMPREPLY[@]}"
    complete -p moq' 2>&1)
for want in '^run-production$' '^seed-initial$' '^conf/MoquiDevConf.xml$' 'complete -F _moq_complete moq'; do
    if printf '%s\n' "$out" | grep -Eq -- "$want"; then ok; else bad "bash completion wrapper: /$want/" "$out"; fi
done
check "zsh script" 0 'compdef _moq_complete moq' -- "$M" completion zsh
check "completion install dry run" 0 "append $FIX/home/.bashrc" -- env HOME="$FIX/home" MOQ_DRY_RUN=1 "$M" completion bash --install
mkdir -p "$FIX/home"
env HOME="$FIX/home" "$M" completion bash --install >/dev/null
env HOME="$FIX/home" "$M" completion bash --install >/dev/null
check "completion install once" 0 '^1$' -- grep -c 'completion bash' "$FIX/home/.bashrc"

# ========== database and files ==========

mkdir -p "$R/runtime/db/embedded/data"
check "save-db" 0 'Saved runtime/save/t1' -- "$M" save-db t1
sleep 30 &
live=$!
echo "$live" >"$R/runtime/db/embedded/data/postmaster.pid"
check "clean-db refuses while running" 1 'is running \(pid' -- "$M" clean-db
check "reload-save refuses while running" 1 'is running \(pid' -- "$M" reload-save t1
kill "$live" 2>/dev/null
wait "$live" 2>/dev/null
check "save-db dump" 0 'dump of' -- cat "$R/runtime/save/t1/moqui.dump"
check "save-db opensearch tar" 0 '' -- test -f "$R/runtime/save/t1/opensearch-data.tar.gz"
check "save-db bad name" 1 'may use only' -- "$M" save-db ../x
check "load-save builds once, with the package of its load" 0 '^1 .*target/debug/moqui save-db' -- \
    sh -c "out=\$(MOQ_DRY_RUN=1 '$M' load-save --debug -n t2 seed 2>&1); printf '%s ' \"\$(printf '%s\n' \"\$out\" | grep -c 'cargo build')\"; printf '%s\n' \"\$out\" | grep ' save-db '"
check "complete save names" 0 '^t1$' -- "$M" __complete 2 reload-save ''
check "clean-db" 0 'Deleting runtime/db/embedded' -- "$M" clean-db
check "clean-db removed cluster" 1 '' -- test -e "$R/runtime/db/embedded"
check "clean-db removed opensearch data" 1 '' -- test -e "$R/runtime/opensearch/data"
check "clean-db removed opensearch logs" 1 '' -- test -e "$R/runtime/opensearch/logs/gc.log"
check "clean-db kept install" 0 '' -- test -d "$R/runtime/db/install/18.6.0"
check "clean-db kept runtime/install" 0 '' -- test -d "$R/runtime/install/postgres/18.6.0"
check "clean-db kept instance_id" 0 '' -- test -f "$R/runtime/db/instance_id"
check "reload-save" 0 'Restored t1' -- "$M" reload-save t1
check "reload-save restored db" 0 'dump of' -- cat "$R/runtime/db/embedded/data/restored"
check "reload-save restored opensearch" 0 'index' -- cat "$R/runtime/opensearch/data/nodes/x"
check "reload-save missing" 1 'no save named nope' -- "$M" reload-save nope
# ========== tidy-cargo ==========
# Two generations of one unit, an older unit with the same features and a different profile, an
# unreadable fingerprint, an incremental crate with no deps file, and build-script runs whose
# fingerprints carry no profile. Stamps are the mtimes tidy compares.

tidy_json() {
    # $1 file, $2 profile number. features and path match; only the profile distinguishes the unit.
    printf '%s\n' '{"rustc":1,"features":"[\"http\"]","declared_features":"[\"http\"]","target":2,"profile":'"$2"',"path":3,"deps":[[1,"libc",false,4]],"local":[],"rustflags":[],"config":5,"compile_kind":0}' >"$1"
}

tidy_unit() {
    # $1 hash, $2 profile, $3 stamp, $4 crate name
    local hash=$1 profile=$2 stamp=$3 crate=$4
    local fp=$R/target/debug/.fingerprint/$crate-$hash
    mkdir -p "$fp" "$R/target/debug/deps" "$R/target/debug/build/$crate-$hash" "$R/target/debug/incremental/$crate-id$hash"
    tidy_json "$fp/lib-$crate.json" "$profile"
    echo stamp >"$fp/invoked.timestamp"
    echo rlib >"$R/target/debug/deps/lib$crate-$hash.rlib"
    echo built >"$R/target/debug/build/$crate-$hash/out"
    echo session >"$R/target/debug/incremental/$crate-id$hash/session"
    touch -d "$stamp" "$fp/invoked.timestamp" "$fp/lib-$crate.json" \
        "$R/target/debug/deps/lib$crate-$hash.rlib" \
        "$R/target/debug/build/$crate-$hash" \
        "$R/target/debug/incremental/$crate-id$hash"
}

# $1 hash, $2 profile, $3 stamp, $4 dep fingerprint (decimal). A library cargo kept.
tidy_bs_lib() {
    local hash=$1 profile=$2 stamp=$3 dep3=$4
    local fp=$R/target/debug/.fingerprint/bs-$hash
    mkdir -p "$fp" "$R/target/debug/deps"
    printf '%s\n' '{"rustc":1,"features":"[\"http\"]","declared_features":"[\"http\"]","target":2,"profile":'"$profile"',"path":3,"deps":[[1,"build_script_build",false,'"$dep3"']],"local":[],"rustflags":[],"config":5,"compile_kind":0}' >"$fp/lib-bs.json"
    echo stamp >"$fp/invoked.timestamp"
    echo rlib >"$R/target/debug/deps/libbs-$hash.rlib"
    touch -d "$stamp" "$fp/invoked.timestamp" "$fp/lib-bs.json" "$R/target/debug/deps/libbs-$hash.rlib"
}

# $1 hash, $2 stamp, $3 16 hex digits. Cargo's run-build-script fingerprint: profile and features are 0.
# The hex is the little-endian fingerprint file a library's build_script dep names.
tidy_bs_run() {
    local hash=$1 stamp=$2 hex=$3
    local fp=$R/target/debug/.fingerprint/bs-$hash
    mkdir -p "$fp" "$R/target/debug/build/bs-$hash"
    printf '%s\n' '{"rustc":1,"features":"","declared_features":"","target":0,"profile":0,"path":0,"deps":[],"local":[],"rustflags":[],"config":0,"compile_kind":0}' >"$fp/run-build-script-build-script-build.json"
    printf '%s' "$hex" >"$fp/run-build-script-build-script-build"
    echo out >"$R/target/debug/build/bs-$hash/output"
    touch -d "$stamp" "$fp/run-build-script-build-script-build.json" "$R/target/debug/build/bs-$hash"
}

OLD=aaaaaaaaaaaaaaaa
NEW=bbbbbbbbbbbbbbbb
OTHER=cccccccccccccccc
BAD=dddddddddddddddd
# Two live libraries, different profiles, each naming its own build-script run. The unreferenced
# run is newer than both, which is the one newest-wins would have kept.
BS1=1111111111111111
BS2=2222222222222222
RUN1=eeeeeeeeeeeeeeee
RUN1B=4444444444444444
RUN2=ffffffffffffffff
RUNX=3333333333333333
tidy_unit "$OLD" 9 '2020-01-01 00:00:00' demo
tidy_unit "$NEW" 9 '2020-06-01 00:00:00' demo
tidy_unit "$OTHER" 8 '2020-03-01 00:00:00' demo
mkdir -p "$R/target/debug/.fingerprint/weird-$BAD" "$R/target/debug/deps"
echo 'not json' >"$R/target/debug/.fingerprint/weird-$BAD/lib-weird.json"
echo bad >"$R/target/debug/deps/libweird-$BAD.rlib"
echo notes >"$R/target/debug/deps/notes.txt"
echo debug-bin >"$R/target/debug/moqui"
mkdir -p "$R/target/moqui-app/src" "$R/target/debug/incremental/orphan-older" "$R/target/debug/incremental/orphan-newer"
echo app >"$R/target/moqui-app/src/main.rs"
echo old >"$R/target/debug/incremental/orphan-older/x"
echo new >"$R/target/debug/incremental/orphan-newer/x"
touch -d '2020-07-01 00:00:00' "$R/target/debug/incremental/orphan-older"
touch -d '2020-08-01 00:00:00' "$R/target/debug/incremental/orphan-newer"
tidy_bs_lib "$BS1" 9 '2020-06-01 00:00:00' 1
tidy_bs_lib "$BS2" 8 '2020-03-01 00:00:00' 2
tidy_bs_run "$RUN1" '2020-01-01 00:00:00' 0100000000000000
# Same fingerprint value as RUN1, different metadata hash. Both outputs belong to that library.
tidy_bs_run "$RUN1B" '2020-02-01 00:00:00' 0100000000000000
tidy_bs_run "$RUN2" '2020-06-01 00:00:00' 0200000000000000
tidy_bs_run "$RUNX" '2020-09-01 00:00:00' 0300000000000000

check "help tidy-cargo" 0 'keeps the current build' -- "$M" help tidy-cargo
check "tidy-cargo args" 1 'takes no arguments' -- "$M" tidy-cargo extra

if have flock; then
    touch "$R/target/debug/.cargo-lock"
    flock "$R/target/debug/.cargo-lock" sleep 60 &
    lockpid=$!
    sleep 1
    check "tidy-cargo refuses a running build" 1 'a cargo build is using' -- "$M" tidy-cargo
    kill "$lockpid" 2>/dev/null
    wait "$lockpid" 2>/dev/null
    rm -f "$R/target/debug/.cargo-lock"
    check "tidy-cargo deleted nothing while locked" 0 '' -- test -f "$R/target/debug/deps/libdemo-$OLD.rlib"
fi

check "tidy-cargo dry run" 0 'would remove .* of old artifacts under target/' -- dry "$M" tidy-cargo
check "tidy-cargo dry run kept the old rlib" 0 '' -- test -f "$R/target/debug/deps/libdemo-$OLD.rlib"
check "tidy-cargo" 0 'removed .* of old artifacts under target/' -- "$M" tidy-cargo
check "tidy-cargo warned on a bad fingerprint" 0 'not cargo fingerprint json' -- "$M" tidy-cargo
check "tidy-cargo removed the old rlib" 1 '' -- test -e "$R/target/debug/deps/libdemo-$OLD.rlib"
check "tidy-cargo removed the old fingerprint" 1 '' -- test -e "$R/target/debug/.fingerprint/demo-$OLD"
check "tidy-cargo removed the old build dir" 1 '' -- test -e "$R/target/debug/build/demo-$OLD"
check "tidy-cargo removed the old incremental dir" 1 '' -- test -e "$R/target/debug/incremental/demo-id$OLD"
check "tidy-cargo kept the new rlib" 0 '' -- test -f "$R/target/debug/deps/libdemo-$NEW.rlib"
check "tidy-cargo kept the new fingerprint" 0 '' -- test -d "$R/target/debug/.fingerprint/demo-$NEW"
check "tidy-cargo kept the new incremental dir" 0 '' -- test -d "$R/target/debug/incremental/demo-id$NEW"
check "tidy-cargo kept the other profile" 0 '' -- test -f "$R/target/debug/deps/libdemo-$OTHER.rlib"
check "tidy-cargo kept the bad fingerprint" 0 '' -- test -f "$R/target/debug/deps/libweird-$BAD.rlib"
check "tidy-cargo kept a deps file with no hash" 0 '' -- test -f "$R/target/debug/deps/notes.txt"
check "tidy-cargo kept the newest orphan incremental dir" 0 '' -- test -d "$R/target/debug/incremental/orphan-newer"
check "tidy-cargo removed the older orphan incremental dir" 1 '' -- test -e "$R/target/debug/incremental/orphan-older"
check "tidy-cargo kept the debug binary" 0 'debug-bin' -- cat "$R/target/debug/moqui"
check "tidy-cargo kept the default binary" 0 'stub:' -- head -n 5 "$R/target/release/moqui"
check "tidy-cargo kept moqui-app" 0 'app' -- cat "$R/target/moqui-app/src/main.rs"
check "tidy-cargo kept an older build-script run" 0 '' -- test -d "$R/target/debug/build/bs-$RUN1"
check "tidy-cargo kept a run with the same fingerprint" 0 '' -- test -d "$R/target/debug/build/bs-$RUN1B"
check "tidy-cargo kept the other build-script run" 0 '' -- test -d "$R/target/debug/build/bs-$RUN2"
check "tidy-cargo kept the older run fingerprint" 0 '' -- test -d "$R/target/debug/.fingerprint/bs-$RUN1"
check "tidy-cargo removed the unreferenced build-script run" 1 '' -- test -e "$R/target/debug/build/bs-$RUNX"
check "tidy-cargo removed the unreferenced run fingerprint" 1 '' -- test -e "$R/target/debug/.fingerprint/bs-$RUNX"
check "tidy-cargo kept both build-script libraries" 0 '' -- test -f "$R/target/debug/deps/libbs-$BS1.rlib" -a -f "$R/target/debug/deps/libbs-$BS2.rlib"
check "tidy-cargo again" 0 'no old cargo artifacts' -- "$M" tidy-cargo

# Cargo 1.100's build directory layout: each unit in build/<package>/<hash>/, its fingerprint in
# fingerprint/ and its files in out/. Once debug has such a unit, the earlier layout there goes.

# $1 hash, $2 profile, $3 stamp, $4 crate name
tidy_new_unit() {
    local hash=$1 profile=$2 stamp=$3 crate=$4
    local unit=$R/target/debug/build/$crate/$hash inc=$R/target/debug/incremental/$crate-idn$hash
    mkdir -p "$unit/fingerprint" "$unit/out" "$inc"
    tidy_json "$unit/fingerprint/lib-$crate.json" "$profile"
    echo stamp >"$unit/fingerprint/invoked.timestamp"
    echo rlib >"$unit/out/lib$crate-$hash.rlib"
    echo session >"$inc/session"
    touch -d "$stamp" "$unit/fingerprint/invoked.timestamp" "$unit/fingerprint/lib-$crate.json" "$unit/out/lib$crate-$hash.rlib" "$inc"
}

# $1 hash, $2 stamp, $3 dep fingerprint (decimal)
tidy_new_bs_lib() {
    local hash=$1 stamp=$2 dep3=$3
    local unit=$R/target/debug/build/nbs/$hash
    mkdir -p "$unit/fingerprint" "$unit/out"
    printf '%s\n' '{"rustc":1,"features":"[\"http\"]","declared_features":"[\"http\"]","target":2,"profile":9,"path":3,"deps":[[1,"build_script_build",false,'"$dep3"']],"local":[],"rustflags":[],"config":5,"compile_kind":0}' >"$unit/fingerprint/lib-nbs.json"
    echo stamp >"$unit/fingerprint/invoked.timestamp"
    echo rlib >"$unit/out/libnbs-$hash.rlib"
    touch -d "$stamp" "$unit/fingerprint/invoked.timestamp" "$unit/fingerprint/lib-nbs.json"
}

# $1 hash, $2 stamp, $3 16 hex digits
tidy_new_bs_run() {
    local hash=$1 stamp=$2 hex=$3
    local unit=$R/target/debug/build/nbs/$hash
    mkdir -p "$unit/fingerprint" "$unit/run" "$unit/out"
    printf '%s\n' '{"rustc":1,"features":"","declared_features":"","target":0,"profile":0,"path":0,"deps":[],"local":[],"rustflags":[],"config":0,"compile_kind":0}' >"$unit/fingerprint/run-build-script-build-script-build.json"
    printf '%s' "$hex" >"$unit/fingerprint/run-build-script-build-script-build"
    echo out >"$unit/run/output"
    touch -d "$stamp" "$unit/fingerprint/run-build-script-build-script-build.json"
}

NOLD=5555555555555555
NNEW=6666666666666666
NBS=7777777777777777
NRUN=8888888888888888
NRUNX=9999999999999999
tidy_new_unit "$NOLD" 9 '2021-01-01 00:00:00' newdemo
tidy_new_unit "$NNEW" 9 '2021-06-01 00:00:00' newdemo
tidy_new_bs_lib "$NBS" '2021-06-01 00:00:00' 5
tidy_new_bs_run "$NRUN" '2021-01-01 00:00:00' 0500000000000000
tidy_new_bs_run "$NRUNX" '2021-09-01 00:00:00' 0600000000000000

if have flock; then
    touch "$R/target/debug/.cargo-build-lock"
    flock "$R/target/debug/.cargo-build-lock" sleep 60 &
    lockpid=$!
    sleep 1
    check "tidy-cargo refuses a build holding the build directory" 1 'a cargo build is using' -- "$M" tidy-cargo
    kill "$lockpid" 2>/dev/null
    wait "$lockpid" 2>/dev/null
    rm -f "$R/target/debug/.cargo-build-lock"
fi

check "tidy-cargo with the build directory layout" 0 'removed .* of old artifacts under target/' -- "$M" tidy-cargo
check "tidy-cargo removed the older unit directory" 1 '' -- test -e "$R/target/debug/build/newdemo/$NOLD"
check "tidy-cargo kept the newer unit directory" 0 '' -- test -f "$R/target/debug/build/newdemo/$NNEW/out/libnewdemo-$NNEW.rlib"
check "tidy-cargo removed the older unit's incremental dir" 1 '' -- test -e "$R/target/debug/incremental/newdemo-idn$NOLD"
check "tidy-cargo kept the newer unit's incremental dir" 0 '' -- test -d "$R/target/debug/incremental/newdemo-idn$NNEW"
check "tidy-cargo kept the build-script run a unit names" 0 '' -- test -f "$R/target/debug/build/nbs/$NRUN/run/output"
check "tidy-cargo removed the unreferenced unit run" 1 '' -- test -e "$R/target/debug/build/nbs/$NRUNX"
check "tidy-cargo removed the earlier layout's rlib" 1 '' -- test -e "$R/target/debug/deps/libdemo-$NEW.rlib"
check "tidy-cargo removed the earlier layout's fingerprint" 1 '' -- test -e "$R/target/debug/.fingerprint/demo-$NEW"
check "tidy-cargo removed the earlier layout's build-script run" 1 '' -- test -e "$R/target/debug/build/bs-$RUN2"
check "tidy-cargo kept a deps file with no hash in the earlier layout" 0 '' -- test -f "$R/target/debug/deps/notes.txt"
check "tidy-cargo with the build directory layout again" 0 'no old cargo artifacts' -- "$M" tidy-cargo

rm -f "$R/cargo.log"
mkdir -p "$R/runtime"/{types,tmp,drive-export,git-export,cert,worktrees}
touch "$R/runtime"/{MoquiConf.xml,.env}
check "exact clean" 0 'tidy-cargo:' -- "$M" clean
check "clean-all dry run" 0 'runtime/types' -- "$M" clean-all --dry-run
check "clean-all needs confirmation" 1 'terminal or --yes' -- "$M" clean-all </dev/null
check "refusal kept data" 0 '' -- test -f "$R/runtime/log/moqui.log"
mkdir -p "$R/runtime/db/embedded/data"
echo $$ >"$R/runtime/db/embedded/data/postmaster.pid"
check "running instance refuses clean-dev" 1 'stop the server' -- "$M" clean-dev
check "running instance refuses clean-all" 1 'stop the server' -- "$M" clean-all --yes
rm "$R/runtime/db/embedded/data/postmaster.pid"
check "clean-dev" 0 'Deleting runtime/tmp' -- "$M" clean-dev
check "clean-dev kept the generated types" 0 '' -- test -d "$R/runtime/types"
check "clean-dev kept build" 0 '' -- test -x "$R/target/release/moqui"
check "clean-dev kept cargo untouched" 1 '' -- test -f "$R/cargo.log"
# Components with browser libraries: a build/js-build.json each. A stub node runs the build script with bash, so these
# checks need neither Node nor the network.
JSBIN=$FIX/js-bin
mkdir -p "$JSBIN"
printf '#!/usr/bin/env bash\ncase "$1" in -e) exit 0 ;; --version) echo v24.0.0; exit 0 ;; esac\nexec bash "$@"\n' >"$JSBIN/node"
printf '#!/usr/bin/env bash\necho 11.0.0\n' >"$JSBIN/npm"
chmod +x "$JSBIN/node" "$JSBIN/npm"
js_component() { # name script-body
    local root="$R/runtime/components/addon/$1"
    mkdir -p "$root/build" "$root/screen/Js/libs"
    printf '{"output": "screen/Js/libs", "projects": [], "script": "build.sh"}\n' >"$root/build/js-build.json"
    printf '%s\n' "$2" >"$root/build/build.sh"
    HELLO_SHA=$(printf 'hello\n' | shasum -a 256 | cut -d' ' -f1)
    printf '{"libraries": [{"id": "hello", "files": [{"path": "hello.js", "sha256": "%s"}]}]}\n' "$HELLO_SHA" \
        >"$root/screen/Js/libs/MANIFEST.json"
}
js_component js-a 'case "$1" in --build) printf "hello\n" >"$JS_OUTPUT/hello.js"; echo "a built" ;; *) echo "a verified" ;; esac'
check "js components in load order" 0 '^webroot js-a $' -- \
    sh -c "perl '$R/scripts/js-state.pl' components '$R/runtime/components' | cut -f1 | tr '\\n' ' '"
check "build-js builds a component" 0 'a built' -- env PATH="$JSBIN:$PATH" "$M" build-js
check "build-js wrote the component's output" 0 '' -- test -f "$R/runtime/components/addon/js-a/screen/Js/libs/hello.js"
check "build-js stamps the component apart" 0 '' -- test -f "$R/target/js/js-a/stamp.json"
lacks "build-js is current after a build" 'built' -- env PATH="$JSBIN:$PATH" "$M" build-js
check "build-js names a component" 0 'a built' -- env PATH="$JSBIN:$PATH" "$M" build-js --force js-a
check "build-js refuses an unknown component" 1 'no component nope' -- env PATH="$JSBIN:$PATH" "$M" build-js nope
check "verify-js runs each component" 0 'a verified' -- env PATH="$JSBIN:$PATH" "$M" verify-js js-a
js_component js-b 'exit 3'
check "one component failing still builds the others" 1 'could not build the JavaScript libraries of js-b' -- \
    env PATH="$JSBIN:$PATH" "$M" build-js --force
check "the other component was built" 0 'a built' -- env PATH="$JSBIN:$PATH" "$M" build-js --force js-a
rm -rf "$R/runtime/components/addon/js-b"
# A component whose build ends by a signal (Ctrl-C) ends the task: js-0 comes before js-a.
js_component js-0 'kill -INT $$'
check "a build ended by SIGINT stops build-js" 130 '' -- env PATH="$JSBIN:$PATH" "$M" build-js --force
lacks "the next component was not built" 'a built' -- env PATH="$JSBIN:$PATH" "$M" build-js --force js-0
rm -rf "$R/runtime/components/addon/js-0"
# An invalid js-build.json is named and left out; the others still build and the task fails.
for output in ../escape screen/ screen/Js/libs/ screen/./libs; do
    js_component js-b 'echo "b built"'
    printf '{"output": "%s", "projects": [], "script": "build.sh"}\n' "$output" >"$R/runtime/components/addon/js-b/build/js-build.json"
    check "the output $output is refused" 1 'js-b/build/js-build.json: output must be a directory under screen/' -- \
        perl "$R/scripts/js-state.pl" configs "$R/runtime/components"
    lacks "the output $output leaves the component out" 'js-b' -- \
        perl "$R/scripts/js-state.pl" components "$R/runtime/components"
done
check "an invalid component fails build-js" 1 'Invalid JavaScript build: .*js-b' -- env PATH="$JSBIN:$PATH" "$M" build-js --force
check "and the others still build" 1 'a built' -- env PATH="$JSBIN:$PATH" "$M" build-js --force
rm -rf "$R/runtime/components/addon/js-b"
# The stamp is written when target/js is gone (cargo clean) and the outputs match their manifest.
rm -rf "$R/target/js"
check "build-js with target/js gone" 0 '' -- env PATH="$JSBIN:$PATH" "$M" build-js
check "the stamp was written again" 0 '' -- test -f "$R/target/js/js-a/stamp.json"
# A locked task puts back a tree a killed publish left aside, and removes its staged tree and temporary installs.
js_out=$R/runtime/components/addon/js-a/screen/Js
mv "$js_out/libs" "$js_out/.libs-previous"
mkdir -p "$js_out/.libs-stage-x" "$R/target/js/js-a/build-x"
check "build-js recovers an interrupted publish" 0 '' -- env PATH="$JSBIN:$PATH" "$M" build-js js-a
check "the output tree is back" 0 '' -- test -f "$js_out/libs/hello.js"
check "the staged tree is gone" 1 '' -- test -e "$js_out/.libs-stage-x"
check "the temporary install is gone" 1 '' -- test -e "$R/target/js/js-a/build-x"
# The journal of a component that is not checked out is named and left; a day-old add-js review is dropped.
mkdir -p "$R/target/js/gone"
echo '{}' >"$R/target/js/gone/rollback.json"
: >"$R/target/js/js-a/review-old.json"
touch -t 202001010000 "$R/target/js/js-a/review-old.json"
: >"$R/target/js/js-a/review-new.json"
check "a journal of a component not checked out is left in place" 0 'gone, which is not checked out here' -- \
    env PATH="$JSBIN:$PATH" "$M" build-js js-a
check "the journal is kept" 0 '' -- test -f "$R/target/js/gone/rollback.json"
check "an old review is dropped" 1 '' -- test -e "$R/target/js/js-a/review-old.json"
check "a new review is kept" 0 '' -- test -f "$R/target/js/js-a/review-new.json"
rm -rf "$R/target/js/gone" "$R/target/js/js-a/review-new.json"
mkdir -p "$webroot/build/fonts" "$webroot/screen/webroot/libs/demo"
: >"$webroot/build/fonts/selected.txt"
touch "$webroot/screen/webroot/libs/demo/demo.js"
check "clean-js" 0 '' -- "$M" clean-js
check "clean-js removed built libraries" 1 '' -- test -e "$webroot/screen/webroot/libs/demo"
check "clean-js removed a component's output" 1 '' -- test -e "$R/runtime/components/addon/js-a/screen/Js/libs/hello.js"
check "clean-js kept a component's manifest" 0 '' -- test -f "$R/runtime/components/addon/js-a/screen/Js/libs/MANIFEST.json"
check "clean-js kept the manifest" 0 '' -- test -f "$webroot/screen/webroot/libs/MANIFEST.json"
check "clean-js kept the font selection" 0 '' -- test -f "$webroot/build/fonts/selected.txt"
mkdir -p "$webroot/screen/webroot/libs/demo"
touch "$webroot/screen/webroot/libs/demo/demo.js"
check "clean-all confirmed" 0 'Keeping downloads' -- "$M" clean-all --yes
check "clean-all removed built libraries" 1 '' -- test -e "$webroot/screen/webroot/libs/demo"
check "clean-all removed the generated types" 1 '' -- test -e "$R/runtime/types"
rm -rf "$R/runtime/components/addon/js-a"
check "clean-all kept the font selection" 0 '' -- test -f "$webroot/build/fonts/selected.txt"
check "clean-all ran cargo clean" 0 'cargo: clean --target-dir' -- cat "$R/cargo.log"
check "clean-all removed log" 1 '' -- test -e "$R/runtime/log/moqui.log"
for kept in save/t1/moqui.dump drive-export git-export MoquiConf.xml .env worktrees cert; do
    check "clean-all kept $kept" 0 '' -- test -e "$R/runtime/$kept"
done
check "load-save dry run" 0 'save-db .*runtime/save/demo/moqui.dump.part' -- dry "$M" load-save -n demo seed
check "opensearch-start without install" 1 'no OpenSearch' -- "$M" opensearch-start
check "opensearch-stop without pid" 0 'not running' -- "$M" opensearch-stop
if [ "$(uname -s)" = Darwin ]; then
    check "opensearch-download refused on macOS" 1 'no macOS tarball' -- "$M" opensearch-download
else
    check "opensearch-download needs force" 1 'add --force' -- "$M" opensearch-download
fi
check "help chromium-download" 0 'AppArmor profile' -- "$M" help chromium-download
check "help test-all" 0 'whole regression suite' -- "$M" help test-all
check "test-all dry run offline webroot" 0 'moqui test-client webroot --offline$' -- dry "$M" test-all
check "test-all dry run offline dev-aide" 0 'moqui test-client runtime/components/dev/dev-aide --offline$' -- dry "$M" test-all
check "test-all dry run turns the demo component on" 0 'moq test --nobuild -D demo_disabled=false demo$' -- dry "$M" test-all
check "test-all dry run runs cargo test" 0 'cargo test --workspace$' -- dry "$M" test-all
lacks "test-all dry run has no node suite" 'node --test' -- dry "$M" test-all
lacks "test-all dry run runs no acceptance test" 'gate3|client_browser|tui_screens|test-slow|--slow|--acceptance' -- dry "$M" test-all
check "test-all has no --slow" 1 'unknown option --slow' -- "$M" test-all --slow
check "test-all passes the package on" 0 'moq test-client --debug --nobuild -- webroot --offline' -- dry "$M" test-all --debug
check "test-all unknown option" 1 'unknown option --fast' -- "$M" test-all --fast
check "help test-apps" 0 'runtime/components/addon' -- "$M" help test-apps
check "help test-framework" 0 'components/base and components/dev' -- "$M" help test-framework
check "test-apps with no app component" 0 'No application component' -- dry "$M" test-apps
lacks "test-apps runs no cargo" 'cargo|moq-test.sh' -- dry "$M" test-apps
lacks "test-framework runs no app suite" '\(apps\)' -- dry "$M" test-framework
# the client crates: moqui-client in the workspace, the others each in a workspace of its own
check "help test-clients" 0 'crates/moqui-client-tauri' -- "$M" help test-clients
check "test-clients runs the client's tests" 0 'cargo test -p moqui-client$' -- dry "$M" test-clients
check "test-clients runs the apps' tests" 0 'cargo test --manifest-path crates/moqui-mobile/Cargo.toml$' -- dry "$M" test-clients
check "test-clients builds into target/" 0 "export CARGO_TARGET_DIR=$R/target\$" -- dry "$M" test-clients tauri
check "test-clients starts a crate's lock from the repository's" 0 \
    "cp $R/Cargo.lock $R/crates/moqui-client-tauri/Cargo.lock\$" -- dry "$M" test-clients tauri
check "test-clients writes the desktop's app crate first" 0 "write $R/target/moqui-app/Cargo.toml" -- \
    dry "$M" test-clients desktop
lacks "test-clients of one crate runs no other" 'moqui-mobile|moqui-desktop|-p moqui-client' -- dry "$M" test-clients tauri
check "test-clients takes a crate's name" 0 'cargo test --manifest-path crates/moqui-desktop/Cargo.toml$' -- \
    dry "$M" test-clients moqui-desktop
check "test-clients passes cargo test's arguments on" 0 \
    'cargo test --manifest-path crates/moqui-client-tauri/Cargo.toml -- --list$' -- dry "$M" test-clients tauri -- -- --list
check "test-clients unknown crate" 1 'no client crate server' -- "$M" test-clients server
check "test-clients unknown option" 1 'unknown option --release' -- "$M" test-clients --release
lacks "test-framework runs no crate outside the workspace" 'manifest-path|moqui-desktop' -- dry "$M" test-framework
# the committed app locks (SEC-CLI-104): kept as they are, and brought to the repository's versions by update-app-locks
mkdir -p "$R/crates/moqui-mobile"
printf '[[package]]\nname = "serde"\nversion = "1.0.1"\nsource = "registry+https://github.com/rust-lang/crates.io-index"\n' \
    >"$R/crates/moqui-mobile/Cargo.lock"
printf '[[package]]\nname = "serde"\nversion = "1.0.9"\nsource = "registry+https://github.com/rust-lang/crates.io-index"\n' \
    >"$R/Cargo.lock"
lacks "test-clients keeps a committed app lock" "cp $R/Cargo.lock $R/crates/moqui-mobile/Cargo.lock" -- dry "$M" test-clients mobile
check "update-app-locks brings the repository's version in" 0 \
    'cargo update --manifest-path .*/crates/moqui-mobile/Cargo.toml -p serde@1.0.1 --precise 1.0.9$' -- dry "$M" update-app-locks
check "deny-apps checks each app for its targets" 0 \
    'cargo deny --manifest-path .*/crates/moqui-mobile/Cargo.toml --target aarch64-linux-android check advisories sources$' -- \
    dry "$M" deny-apps
rm -rf "$R/crates/moqui-mobile" "$R/Cargo.lock"
check "test-client port selects HTTPS" 0 'moqui test-client --server https://localhost:9443$' -- dry "$M" test-client -p 9443
check "test-load port selects HTTPS" 0 'moqui test-load --server https://localhost:9443$' -- dry "$M" test-load -p 9443
check "test-client rejects an off destination" 1 'numeric HTTPS port' -- dry "$M" test-client -p off
check "test-client is still test-client" 0 'moqui test-client$' -- dry "$M" test-client
# an app component and a framework component with specs, a served client suite and an offline one
mkdir -p "$R/runtime/components/addon/app-x/test/client" "$R/runtime/components/base/fw-x/test/client" \
    "$R/runtime/components/addon/app-y/test/client"
printf '<component name="app-x"/>\n' >"$R/runtime/components/addon/app-x/component.xml"
printf '<component name="app-y"/>\n' >"$R/runtime/components/addon/app-y/component.xml"
printf '<component name="fw-x"/>\n' >"$R/runtime/components/base/fw-x/component.xml"
: >"$R/runtime/components/addon/app-x/test/Rule.spec.js"
: >"$R/runtime/components/addon/app-x/test/client/Walk.spec.js"
printf 'suite(["Walk"], { server: { components: ["app-x"] } })\n' >"$R/runtime/components/addon/app-x/test/client/suite.js"
: >"$R/runtime/components/addon/app-y/test/client/Page.spec.js"
printf '// No server: offline\nsuite(["Page"])\n' >"$R/runtime/components/addon/app-y/test/client/suite.js"
: >"$R/runtime/components/base/fw-x/test/Fw.spec.js"
check "test-apps runs the app specs" 0 'moq test --nobuild -- app-x --workers 4$' -- dry "$M" test-apps
check "test-apps serves the app client specs" 0 'moq test-client --nobuild -- --serve app-x$' -- dry "$M" test-apps
check "test-framework runs the framework specs" 0 'moq test -- fw-x --workers 4$' -- dry "$M" test-framework
lacks "test-framework leaves the app specs out" 'app-x' -- dry "$M" test-framework
# security proofs: their own tree, in the regression suites, never among the server specs
mkdir -p "$R/runtime/components/base/fw-x/test/proofs" "$R/runtime/components/addon/app-y/test/proofs"
: >"$R/runtime/components/base/fw-x/test/proofs/V3Web.spec.js"
printf 'suite(["V3Web"], { server: { components: ["fw-x"] } })\n' >"$R/runtime/components/base/fw-x/test/proofs/suite.js"
: >"$R/runtime/components/addon/app-y/test/proofs/V8Access.spec.js"
printf 'suite(["V8Access"], { server: { components: ["app-y"] } })\n' >"$R/runtime/components/addon/app-y/test/proofs/suite.js"
check "test-framework serves the framework proofs" 0 'moq test-client --nobuild -- --serve --proofs fw-x$' -- dry "$M" test-framework
check "test-apps serves the app proofs" 0 'moq test-client --nobuild -- --serve --proofs app-y$' -- dry "$M" test-apps
lacks "test-apps runs no server spec of a component with only proofs" 'moq test --nobuild -- .*app-y' -- dry "$M" test-apps
lacks "accept runs no proof" '--proofs' -- dry "$M" accept
check "test-all runs both" 0 'moq test --nobuild -- app-x --workers 4$' -- dry "$M" test-all
check "MOQ_TEST_WORKERS sets the workers" 0 'moq test --nobuild -- app-x --workers 2$' -- env MOQ_TEST_WORKERS=2 MOQ_DRY_RUN=1 "$M" test-apps
# acceptance: its own tree, run only by accept and accept-all
mkdir -p "$R/runtime/components/addon/app-x/test/acceptance/client"
: >"$R/runtime/components/addon/app-x/test/acceptance/Sweep.spec.js"
: >"$R/runtime/components/addon/app-x/test/acceptance/client/Scenario.spec.js"
printf 'suite(["Scenario"], { server: { components: ["app-x"] } })\n' >"$R/runtime/components/addon/app-x/test/acceptance/client/suite.js"
lacks "test-apps runs no acceptance spec" '--acceptance' -- dry "$M" test-apps
mkdir -p "$R/runtime/components/addon/app-y/test/acceptance"
: >"$R/runtime/components/addon/app-y/test/acceptance/Only.spec.js"
check "test-apps leaves out test/acceptance" 0 'moq test --nobuild -- app-x --workers 4$' -- dry "$M" test-apps
rm -r "$R/runtime/components/addon/app-y/test/acceptance"
check "accept runs the acceptance specs" 0 'moq test -- app-x --acceptance$' -- dry "$M" accept
check "accept serves the acceptance client specs" 0 'moq test-client --nobuild -- --serve --acceptance app-x$' -- dry "$M" accept app-x
check "accept --spec picks the tree of the spec" 0 'moq test-client --nobuild -- --serve --acceptance app-x --spec Scenario$' -- \
    dry "$M" accept --spec Scenario
lacks "accept --spec leaves out the other tree" 'moq test --' -- dry "$M" accept --spec Scenario
check "accept of a component with none" 0 'No acceptance specs' -- dry "$M" accept fw-x
mkdir -p "$R/runtime/components/addon/app-y/acceptance"
check "accept refuses a top-level acceptance/" 1 'move acceptance/ there .*app-y' -- dry "$M" accept
check "accept of another component ignores it" 0 'moq test -- app-x --acceptance$' -- dry "$M" accept app-x
rmdir "$R/runtime/components/addon/app-y/acceptance"
check "accept-all runs the Rust acceptance tests" 0 'cargo test-slow$' -- dry "$M" accept-all
check "accept-all runs the opt-in host tests" 0 'cargo test -p moqui-host --test gate3 --test client_browser --test tui_screens$' -- \
    dry "$M" accept-all
check "accept-all runs gate3_browser" 0 'cargo test -p moqui-host --test gate3_browser$' -- dry "$M" accept-all
check "accept-all writes a dated junit directory" 0 '--junit .*/junit/[0-9]{8}-[0-9]{6}$' -- dry "$M" accept-all --junit junit
check "accept-all takes no component" 1 'unknown argument' -- "$M" accept-all app-x
rm -rf "$R/runtime/components/addon/app-x" "$R/runtime/components/addon/app-y" "$R/runtime/components/base/fw-x"
check "test still resolves to test" 0 'moqui test$' -- dry "$M" test
check "chromium-download unknown option" 1 'unknown option --full' -- "$M" chromium-download --full
check "chromium-download bad revision" 1 'snapshot number' -- "$M" chromium-download 12x
if [ "$(uname -s)/$(uname -m)" = Linux/x86_64 ] && have curl && have unzip; then
    check "chromium-download dry run" 0 'curl .*Linux_x64/1697588/chrome-linux.zip' -- dry "$M" chromium-download 1697588
    check "chromium-download dry run unpacks" 0 'mv .*chrome-linux .*runtime/chromium' -- dry "$M" chromium-download 1697588
fi

# ========== git ==========

if have git; then
    export GIT_AUTHOR_NAME=test GIT_AUTHOR_EMAIL=test@example.com GIT_COMMITTER_NAME=test GIT_COMMITTER_EMAIL=test@example.com
    export GIT_CONFIG_NOSYSTEM=1 HOME=$FIX/home
    git config --global init.defaultBranch master
    git config --global protocol.file.allow always

    # Component "remotes": comp-a depends on comp-b.
    mkrepo() {
        local name=$1 body=$2
        mkdir -p "$FIX/src/$name"
        printf '%s\n' "$body" >"$FIX/src/$name/component.xml"
        (cd "$FIX/src/$name" && git init -q && git add . && git commit -qm init) || exit 1
        git clone -q --bare "$FIX/src/$name" "$FIX/repos/$name.git"
    }
    mkdir -p "$FIX/repos"
    mkrepo comp-a '<component xmlns:xsi="x"
        name="comp-a" version="1.0">
    <depends-on name="comp-b"/>
</component>'
    mkrepo comp-b '<component name="comp-b" version="2.0"/>'
    mkrepo comp-c '<component name="comp-c"/>'
    # A fork that keeps its upstream component name: repository and directory mr-comp-d, component.xml name comp-d.
    mkrepo mr-comp-d '<component name="comp-d" version="1.0"/>'
    mkrepo comp-e '<component name="comp-e">
    <depends-on name="comp-d"/>
</component>'

    cat >"$R/myaddons.xml" <<EOF
<addons default-repository="local" default="comp-c">
    <!-- <component name="comp-a" group="wrong" branch="nope"/> -->
    <repository name="local">
        <location type="git" url="file://$FIX/repos/\${name}.git"/>
        <location type="current" url="file://$FIX/zips/\${name}-\${branch}.zip"/>
        <location type="release" url="file://$FIX/zips/\${name}-\${version}.zip"/>
    </repository>
    <!-- The detected default repositories; the -ssh one has its own directory so the two are told apart. -->
    <repository name="github">
        <location type="git" url="file://$FIX/repos/\${name}.git"/>
    </repository>
    <repository name="github-ssh">
        <location type="git" url="file://$FIX/repos-ssh/\${name}.git"/>
    </repository>
    <component name="comp-a" group="g" branch="master"/>
    <component name="comp-b" group="g" branch="master" repository="local"/>
    <component name="comp-c" group="g" branch="master" version="1.5"/>
    <component name="comp-skip" group="g" branch="master" skip-get="true"/>
    <component name="mr-comp-d" component-name="comp-d" group="g" branch="master"/>
    <component name="comp-e" group="g" branch="master"/>
    <component-set name="outer" sets="inner"/>
    <component-set name="inner" components="comp-c"/>
</addons>
EOF
    (cd "$R" && git init -q && git add moq && git commit -qm root)

    check "complete component names" 0 '^comp-a$' -- "$M" __complete 2 component-get comp
    check "complete set names" 0 '^outer$' -- "$M" __complete 2 component-get ''
    check "component-get with depends" 0 'Checked: comp-a, comp-b' -- "$M" component-get comp-a
    check "component-get cloned dependency" 0 '' -- test -f "$R/runtime/components/addon/comp-b/component.xml"
    check "component-get present" 0 'comp-a is already in' -- "$M" component-get comp-a
    check "component-get skip-get" 0 'Skipping comp-skip' -- "$M" component-get comp-skip
    check "component-get unknown" 1 'not a component or component-set' -- "$M" component-get nope
    check "component-get defaults" 0 'Getting comp-c' -- "$M" component-get
    rm -rf "$R/runtime/components/addon/comp-c"
    check "component-get nested set" 0 'Getting comp-c' -- "$M" component-get outer
    rm -rf "$R/runtime/components/addon/comp-b"
    check "component-depends" 0 'comp-a depends on comp-b' -- "$M" component-depends
    check "component-get bad type" 1 'not git, current' -- "$M" component-get -t svn comp-a
    check "a depends-on names a fork by its component name" 0 'Getting mr-comp-d' -- "$M" component-get comp-e
    check "the fork goes in its own directory" 0 '' -- test -f "$R/runtime/components/addon/mr-comp-d/component.xml"
    check "component-depends finds the fork by its component name" 0 'Checked: .*comp-e, mr-comp-d' -- \
        "$M" component-depends
    lacks "component-depends gets no fork that is there" 'Getting' -- "$M" component-depends
    check "component-get by component name" 0 'mr-comp-d is already in' -- "$M" component-get comp-d
    rm -rf "$R/runtime/components/addon/mr-comp-d" "$R/runtime/components/addon/comp-e"
    check "component-get --dev" 0 'Checked: comp-a' -- "$M" component-get --dev comp-a
    check "component-get --dev clones into components/dev" 0 '' -- test -f "$R/runtime/components/dev/comp-a/component.xml"
    check "git includes components/dev repos" 0 '== runtime/components/dev/comp-a' -- "$M" git rev-parse HEAD
    rm -rf "$R/runtime/components/dev"
    check "complete --dev on create" 0 '^--dev$' -- "$M" __complete 2 component-create --de
    check "component-get --test" 0 'Checked: comp-a' -- "$M" component-get --test comp-a
    check "component-get --test clones into components/test" 0 '' -- test -f "$R/runtime/components/test/comp-a/component.xml"
    check "git includes components/test repos" 0 '== runtime/components/test/comp-a' -- "$M" git rev-parse HEAD
    rm -rf "$R/runtime/components/test"
    check "complete --test on create" 0 '^--test$' -- "$M" __complete 2 component-create --te

    # Repository: -r, then the component's repository attribute, then default-repository, then how the root
    # repo was cloned (https -> github, git@ -> github-ssh, no origin -> github).
    check "complete -r option" 0 '^--repo$' -- "$M" __complete 2 component-get --r
    check "complete -r repositories" 0 '^github-ssh$' -- "$M" __complete 3 component-get -r git
    cp "$R/myaddons.xml" "$FIX/myaddons.xml.bak"
    rm -rf "$R/runtime/components/addon/comp-c" "$R/runtime/components/addon/comp-b"
    sed 's/ default-repository="local"//' "$R/myaddons.xml" >"$R/myaddons.xml.moq-tmp" &&
        cat "$R/myaddons.xml.moq-tmp" >"$R/myaddons.xml"
    rm -f "$R/myaddons.xml.moq-tmp"
    check "default repository with no origin" 0 "file://$FIX/repos/comp-c.git" -- dry "$M" component-get comp-c
    (cd "$R" && git remote add origin https://github.com/moqui/mr-moqui.git)
    check "default repository for an https clone" 0 "file://$FIX/repos/comp-c.git" -- dry "$M" component-get comp-c
    (cd "$R" && git remote set-url origin git@github.com:moqui/mr-moqui.git)
    check "default repository for a git@ clone" 0 "file://$FIX/repos-ssh/comp-c.git" -- dry "$M" component-get comp-c
    check "repository attribute beats the clone url" 0 "file://$FIX/repos/comp-b.git" -- \
        dry "$M" component-get comp-b
    cp "$FIX/myaddons.xml.bak" "$R/myaddons.xml"
    check "default-repository beats the clone url" 0 "file://$FIX/repos/comp-c.git" -- dry "$M" component-get comp-c
    check "-r beats default-repository" 0 "file://$FIX/repos-ssh/comp-c.git" -- \
        dry "$M" component-get -r github-ssh comp-c
    check "-r beats the repository attribute" 0 "file://$FIX/repos-ssh/comp-b.git" -- \
        dry "$M" component-get --repo=github-ssh comp-b
    check "-r unknown repository" 1 'repository nope has no git location' -- dry "$M" component-get -r nope comp-c
    check "-r needs a repository" 1 'needs a repository' -- "$M" component-get -r
    (cd "$R" && git remote remove origin)
    cp "$FIX/myaddons.xml.bak" "$R/myaddons.xml"
    # Back to the fixture defaults, with the components the later checks expect in place again.
    "$M" component-get comp-b comp-c >/dev/null

    # A server's addons.xml through a myaddons.xml repository entry (a group's served addons.xml): its components and
    # repositories are found after myaddons.xml, never its root's default or default-repository; one that cannot be
    # read, or is not https, is skipped with a warning.
    if have curl; then
        mkrepo comp-f '<component name="comp-f" version="1.0"/>'
        mkdir -p "$FIX/served"
        cat >"$FIX/served/addons.xml" <<EOF
<addons default="comp-a" default-repository="nope">
    <repository name="served.example">
        <location type="git" url="file://$FIX/repos/\${name}.git"/>
    </repository>
    <component name="comp-f" group="acme" version="1.0" repository="served.example"/>
</addons>
EOF
        cp "$R/myaddons.xml" "$FIX/myaddons.xml.bak"
        served_entry() {
            awk -v line="    <repository name=\"$1\"><location type=\"addons\" url=\"$2\"/></repository>" \
                '/<\/addons>/ { print line } { print }' "$R/myaddons.xml" >"$R/myaddons.xml.moq-tmp" &&
                cat "$R/myaddons.xml.moq-tmp" >"$R/myaddons.xml"
            rm -f "$R/myaddons.xml.moq-tmp"
        }
        check "component-get finds nothing served before the entry" 1 'not a component' -- "$M" component-get comp-f
        served_entry team "file://$FIX/served/addons.xml"
        check "component-get from a served addons.xml" 0 "Getting comp-f .*file://$FIX/repos/comp-f.git" -- \
            "$M" component-get comp-f
        check "the served component is cloned" 0 '' -- test -f "$R/runtime/components/addon/comp-f/component.xml"
        rm -rf "$R/runtime/components/addon/comp-c"
        check "a served root's default-repository is not used" 0 "file://$FIX/repos/comp-c.git" -- \
            dry "$M" component-get -t git comp-c
        rm -rf "$R/runtime/components/addon/comp-f"
        # an entry named as the server's repository falls through to the served file's locations
        cp "$FIX/myaddons.xml.bak" "$R/myaddons.xml"
        served_entry served.example "file://$FIX/served/addons.xml"
        check "an addons-only entry falls through to the served repository" 0 "file://$FIX/repos/comp-f.git" -- \
            dry "$M" component-get comp-f
        served_entry gone "file://$FIX/served/missing.xml"
        served_entry plain "http://example.com/addons.xml"
        check "an unreadable served addons.xml is skipped" 0 'could not read an addons.xml from file://.*missing.xml' -- \
            dry "$M" component-get comp-f
        check "a served addons.xml over plain http is skipped" 0 'example.com/addons.xml is not https' -- \
            dry "$M" component-get comp-f
        cp "$FIX/myaddons.xml.bak" "$R/myaddons.xml"
        "$M" component-get comp-c >/dev/null
        # addons.xml is searched before a served file, so a served file adds components and repositories and never
        # replaces addons.xml's; and a served component's name is a name, never a path (SEC-OUT-87)
        cat >"$R/addons.xml" <<EOF
<addons>
    <repository name="official">
        <location type="git" url="file://$FIX/repos/\${name}.git"/>
    </repository>
    <component name="comp-g" group="g" branch="master" repository="official"/>
</addons>
EOF
        cat >"$FIX/served/addons.xml" <<EOF
<addons>
    <repository name="official">
        <location type="git" url="file://$FIX/served-repos/\${name}.git"/>
    </repository>
    <repository name="served.example">
        <location type="git" url="file://$FIX/served-repos/\${name}.git"/>
    </repository>
    <component name="comp-g" group="g" branch="master" repository="served.example"/>
    <component name="../escape" group="g" branch="master" repository="served.example"/>
</addons>
EOF
        served_entry team "file://$FIX/served/addons.xml"
        check "a served addons.xml does not replace a component of addons.xml" 0 "file://$FIX/repos/comp-g.git" -- \
            dry "$M" component-get comp-g
        check "a served component's name is never a path" 1 'is not a component name' -- \
            dry "$M" component-get ../escape
        rm -f "$R/addons.xml"
        cp "$FIX/myaddons.xml.bak" "$R/myaddons.xml"
    fi

    check "git in each repo" 0 '== runtime/components/addon/comp-a \(master\)' -- "$M" git rev-parse --abbrev-ref HEAD
    check "git failure exit" 1 'git failed in:' -- "$M" git rev-parse --verify nope
    echo change >>"$R/runtime/components/addon/comp-a/component.xml"
    check "git-status shows change" 0 ' M component.xml' -- "$M" git-status
    lacks "git-status -s hides clean repos" 'comp-b' -- "$M" git-status -s
    echo '<!-- new -->' >>"$FIX/src/comp-b/component.xml"
    (cd "$FIX/src/comp-b" && git commit -qam "second commit" && git push -q "$FIX/repos/comp-b.git" master)
    # The plain check captures a pipe, so git never pages. A pseudo-terminal would start GIT_PAGER.
    # Export it: a prefix assignment on a function is not given to child processes in bash 3.2.
    tty_status=0
    GIT_PAGER='printf PAGED\n; exec cat'
    export GIT_PAGER
    tty_out=$(run_on_tty "$(printf '%q' "$M") git-pull" 2>&1) || tty_status=$?
    unset GIT_PAGER
    if [ "$tty_status" -eq 127 ]; then
        skipped="$skipped git-pull-pager"
        check "git-pull lists new commits" 1 'second commit' -- "$M" git-pull
    else
        if printf '%s\n' "$tty_out" | grep -Eq 'second commit'; then
            ok
        else
            bad "git-pull lists new commits on a tty" "$tty_out"
        fi
        if printf '%s\n' "$tty_out" | grep -Eq 'PAGED'; then
            bad "git-pull does not page" "$tty_out"
        else
            ok
        fi
    fi
    check "git-pull reports the repo without a remote" 1 'git pull failed in: \.$' -- "$M" git-pull
    check "complete git refs" 0 '^master$' -- "$M" __complete 3 git checkout ma
    check "complete git-checkout-all alias refs" 0 '^master$' -- "$M" __complete 2 gitCheckoutAll ma

    # component-create from a copy of the component template (runtime/components/dev/template).
    mkdir -p "$R/runtime/components/dev"
    cp -R "$SRC/runtime/components/dev/template" "$R/runtime/components/dev/"
    files_of() { (cd "$1" && find . -type f ! -path './.git/*' | LC_ALL=C sort); }
    check "complete --from templates" 0 '^template$' -- "$M" __complete 3 component-create --from t
    check "create dry run" 0 'cp -R .*/template/\. .*/components/addon/dry-shop' -- dry "$M" component-create dry-shop
    check "create dry run writes nothing" 1 '' -- test -e "$R/runtime/components/addon/dry-shop"
    check "component-create" 0 'Created runtime/components/addon/my-shop from template' -- \
        "$M" component-create my-shop --myaddons me --git-remote https://example.com/my-shop.git
    S=$R/runtime/components/addon/my-shop
    check "create names every file" 0 '' -- test "$(files_of "$S")" = "$(files_of "$R/runtime/components/dev/template" \
        | sed -e 's/@component@/my-shop/g' -e 's/@Class@/MyShop/g' -e 's/@package@/myshop/g' | LC_ALL=C sort)"
    check "create directories" 0 '' -- test -f "$S/service/myshop/MyShopServices.xml" -a -f "$S/screen/MyShop/FindItem.xml"
    check "create leaves no placeholder" 1 '' -- grep -rE --exclude-dir=.git '@(component|Title|Class|package|ID)@' "$S"
    check "create takes out disabled" 1 '' -- grep -q 'disabled=' "$S/component.xml"
    check "create name" 0 'name="my-shop" version' -- cat "$S/component.xml"
    check "create title" 0 'menu-title="My Shop"' -- cat "$S/MoquiConf.xml"
    check "create class and package" 0 'entity-name="MyShopItem" package="myshop"' -- cat "$S/entity/MyShopEntities.xml"
    check "create id" 0 'artifactGroupId="MY_SHOP_APP"' -- cat "$S/data/MyShopSecurityData.xml"
    check "create leaves the template as it is" 0 'name="@component@"' -- \
        cat "$R/runtime/components/dev/template/component.xml"
    check "create new git repo" 0 'example.com/my-shop.git' -- git -C "$S" remote -v
    check "create myaddons line" 0 'name="my-shop" group="me"' -- cat "$R/myaddons.xml"
    check "create myaddons still closed" 0 '</addons>' -- tail -n 1 "$R/myaddons.xml"
    check "create exists" 1 'already exists' -- "$M" component-create my-shop
    check "create name starts with a letter" 1 'starts with a letter' -- "$M" component-create 9lives --no-git
    check "create name has no dot" 1 'starts with a letter' -- "$M" component-create my.shop --no-git
    check "create unknown template" 1 'no template nope' -- "$M" component-create other --from nope --no-git
    check "create --rest" 0 'REST API only' -- "$M" component-create my_api --rest --no-git
    check "create --rest has no screens" 1 '' -- test -e "$R/runtime/components/addon/my_api/screen" -o \
        -e "$R/runtime/components/addon/my_api/MoquiConf.xml" -o -e "$R/runtime/components/addon/my_api/test/MyApiScreens.spec.js"
    check "create --rest has no screen sweep" 1 '' -- test -e "$R/runtime/components/addon/my_api/test/acceptance"
    check "create has the screen sweep in acceptance" 0 '' -- test -f "$S/test/acceptance/MyShopScreenSweep.spec.js"
    check "create --rest keeps the REST API" 0 '' -- test -f "$R/runtime/components/addon/my_api/service/my_api.rest.xml"
    check "create --screens --dev" 0 'Screens only' -- "$M" component-create shop --screens --dev --no-git
    check "create --screens has no REST API" 1 '' -- test -e "$R/runtime/components/dev/shop/service/shop.rest.xml" -o \
        -e "$R/runtime/components/dev/shop/data/ShopApiSecurityData.xml"
    check "create --screens keeps the screens" 0 '' -- test -f "$R/runtime/components/dev/shop/screen/Shop.xml"
    # A template reached through a link: its contents are copied, so filling them in leaves the template as it is.
    ln -s "$R/runtime/components/dev/template" "$R/runtime/components/addon/linked-template"
    check "create from a linked template" 0 'Created' -- "$M" component-create via-link --from linked-template --no-git
    check "a linked template is copied, not linked" 1 '' -- test -L "$R/runtime/components/addon/via-link"
    check "a linked template stays a template" 0 'name="@component@"' -- \
        cat "$R/runtime/components/dev/template/component.xml"
    rm -rf "$R/runtime/components/addon/linked-template" "$R/runtime/components/addon/via-link" "$R/runtime/components/dev/shop"
else
    skipped="$skipped git"
fi

if have git && have zip && have unzip && have curl; then
    mkdir -p "$FIX/zips"
    (cd "$FIX/src" && cp -R comp-c comp-c-master && rm -rf comp-c-master/.git && zip -qr "$FIX/zips/comp-c-master.zip" comp-c-master)
    rm -rf "$R/runtime/components/addon/comp-c"
    check "component-get current (zip)" 0 'Downloaded comp-c' -- "$M" component-get -t current comp-c
    check "zip unpacked one level up" 0 '' -- test -f "$R/runtime/components/addon/comp-c/component.xml"
    # SEC-OUT-14: a version's archive is recorded the first time and checked after; a changed one stops the get
    (cd "$FIX/src" && cp -R comp-c comp-c-1.5 && rm -rf comp-c-1.5/.git && zip -qr "$FIX/zips/comp-c-1.5.zip" comp-c-1.5)
    rm -rf "$R/runtime/components/addon/comp-c"
    check "component-get release records its SHA-256" 0 'Recorded the SHA-256 of addon:' -- \
        env MOQUI_EMBED_CACHE="$FIX/sums-cache/embed" "$M" component-get -t release comp-c
    rm -rf "$R/runtime/components/addon/comp-c"
    check "component-get release checks it again" 0 'Checked addon:.* against the SHA-256' -- \
        env MOQUI_EMBED_CACHE="$FIX/sums-cache/embed" "$M" component-get -t release comp-c
    rm -rf "$R/runtime/components/addon/comp-c"
    (cd "$FIX/src" && printf 'changed\n' >comp-c-1.5/changed.txt && zip -qr "$FIX/zips/comp-c-1.5.zip" comp-c-1.5)
    check "component-get release refuses a changed archive" 1 'does not match the SHA-256 recorded' -- \
        env MOQUI_EMBED_CACHE="$FIX/sums-cache/embed" "$M" component-get -t release comp-c
    check "the refused archive is not unpacked" 1 '' -- test -d "$R/runtime/components/addon/comp-c"
    rm -rf "$R/runtime/components/addon/comp-c"
    "$M" component-get -t current comp-c >/dev/null 2>&1
    mkdir -p "$R/runtime/components/addon/comp-a/test"
    : >"$R/runtime/components/addon/comp-a/test/Rule.spec.js"
    check "component-zip" 0 'Created runtime/components/addon/comp-a-1.0.zip' -- "$M" component-zip comp-a
    lacks "component-zip leaves out .git" '\.git/' -- unzip -l "$R/runtime/components/addon/comp-a-1.0.zip"
    lacks "component-zip leaves out test/" 'comp-a/test/' -- unzip -l "$R/runtime/components/addon/comp-a-1.0.zip"
    check "component-zip --with-tests" 0 'Created' -- "$M" component-zip --with-tests comp-a
    check "component-zip --with-tests keeps test/" 0 'comp-a/test/Rule.spec.js' -- \
        unzip -l "$R/runtime/components/addon/comp-a-1.0.zip"
    rm -rf "$R/runtime/components/addon/comp-a/test"
    check "complete component-zip" 0 '^comp-a$' -- "$M" __complete 2 component-zip comp-a
else
    skipped="$skipped zip"
fi

# ========== archive ==========

if have zip && have unzip && have tar && have gzip; then
    O=$FIX/out
    mkdir -p "$O"
    mkdir -p "$R/runtime/opensearch/logs" "$R/runtime/opensearch/data/nodes" "$R/runtime/tmp"
    echo 'gc' >"$R/runtime/opensearch/logs/gc.log"
    echo 'index' >"$R/runtime/opensearch/data/nodes/x"
    echo 'scratch' >"$R/runtime/tmp/x"
    mkdir -p "$R/runtime/cert/dev-ca/signer"
    : >"$R/runtime/cert/dev-ca/signer/issuer.key"
    : >"$R/runtime/cert/dev-ca/ca.crt"
    echo 'secret=1' >"$R/runtime/.env"
    echo '<moqui-conf dev-override=""/>' >"$R/runtime/MoquiConf.xml"
    # tests and the dev set stay out of a production archive; a deeper directory named test stays in
    mkdir -p "$R/runtime/components/base/fw-z/test/acceptance" "$R/runtime/components/base/fw-z/service/z/test" \
        "$R/runtime/components/test/tc" "$R/runtime/components/dev/dv"
    : >"$R/runtime/components/base/fw-z/test/acceptance/Walk.spec.js"
    : >"$R/runtime/components/base/fw-z/service/z/test/Keep.xml"
    mkdir -p "$R/runtime/components/base/fw-z/plan" "$R/runtime/components/base/fw-z/plans" \
        "$R/runtime/components/base/fw-z/screen/plan"
    : >"$R/runtime/components/base/fw-z/plan/design.md"
    : >"$R/runtime/components/base/fw-z/plans/Next.md"
    : >"$R/runtime/components/base/fw-z/screen/plan/Keep.xml"
    : >"$R/runtime/components/test/tc/component.xml"
    : >"$R/runtime/components/dev/dv/component.xml"
    # the names archive-excludes.txt lists, which the runtime built into the binary leaves out too
    mkdir -p "$R/runtime/components/base/fw-z/target/debug" "$R/runtime/components/base/fw-z/script/__pycache__" \
        "$R/runtime/components/base/fw-z/src/target"
    : >"$R/runtime/components/base/fw-z/target/debug/out"
    : >"$R/runtime/components/base/fw-z/script/__pycache__/x.pyc"
    : >"$R/runtime/components/base/fw-z/src/target/Keep.rs"
    : >"$R/runtime/components/base/fw-z/screen/Edit.xml~"
    check "archive default name" 0 "Created $O/mr-moqui-[0-9]{8}\.zip .*expands to mr-moqui-[0-9]{8}/" -- \
        sh -c "cd '$O' && '$M' archive"
    # the name moq gave it, so a run across midnight checks the day moq saw
    today=$(cd "$O" && ls -d mr-moqui-*.zip 2>/dev/null | head -n 1)
    today=${today%.zip}
    zip_list=$(unzip -l "$O/$today.zip")
    if tar --help 2>&1 | grep -q BusyBox; then
        check "archive --tgz refuses BusyBox tar" 1 'needs GNU or BSD tar' -- "$M" archive --tgz "$O/bb"
        kinds=zip
    else
        check "archive --tgz" 0 "Created $O/mr-moqui-[0-9]{8}\.tgz" -- sh -c "cd '$O' && '$M' archive --tgz"
        tgz_today=$(cd "$O" && ls -d mr-moqui-*.tgz 2>/dev/null | head -n 1)
        tgz_today=${tgz_today%.tgz}
        tgz_list=$(tar -tzf "$O/$tgz_today.tgz" | sed 's/^/ /')
        kinds='zip tgz'
    fi
    for kind in $kinds; do
        if [ "$kind" = zip ]; then list=$zip_list top=$today; else list=$tgz_list top=$tgz_today; fi
        for want in "$top/moqui" "$top/MoquiConf.xml" "$top/README.txt" "$top/conf/MoquiDevConf.xml" \
            "$top/components/base/" "$top/cert/dev-ca/ca.crt" "$top/db/instance_id" "$top/.env" "$top/opensearch/data/nodes/x"; do
            check "$kind has $want" 0 '' -- grep -qF " $want" <<<"$list"
        done
        check "$kind keeps a deeper test directory" 0 '' -- grep -qF " $top/components/base/fw-z/service/z/test/Keep.xml" <<<"$list"
        check "$kind keeps a deeper plan directory" 0 '' -- grep -qF " $top/components/base/fw-z/screen/plan/Keep.xml" <<<"$list"
        check "$kind keeps a deeper target directory" 0 '' -- grep -qF " $top/components/base/fw-z/src/target/Keep.rs" <<<"$list"
        for unwanted in "$top/runtime" "$top/log" "$top/tmp" "$top/save" "$top/install" db/install \
            cert/dev-ca/signer opensearch/logs/. '\.git/' '\.moq-archive' fw-z/test/ fw-z/plan/ fw-z/plans/ components/test/ components/dev/ \
            fw-z/target/ __pycache__ 'Edit\.xml~'; do
            lacks "$kind leaves out $unwanted" "$unwanted" -- printf '%s\n' "$list"
        done
        rm -rf "$O/x"
        mkdir -p "$O/x"
        if [ "$kind" = zip ]; then
            (cd "$O/x" && unzip -q "$O/$top.zip")
        else
            tar -xzf "$O/$top.tgz" -C "$O/x"
        fi
        check "$kind binary is executable" 0 '' -- test -x "$O/x/$top/moqui"
        check "$kind conf is the production conf, not the runtime's" 0 '' -- \
            cmp "$O/x/$top/MoquiConf.xml" "$R/runtime/conf/MoquiProductionConf.xml"
        check "$kind readme" 0 'keep this archive private' -- cat "$O/x/$top/README.txt"
        check "$kind readme says the tests are left out" 0 'each component.s test/, plan/ and plans/' -- \
            cat "$O/x/$top/README.txt"
    done
    rm -f "$R/runtime/MoquiConf.xml"
    check "archive exists" 1 'there already; -f replaces it' -- sh -c "cd '$O' && '$M' archive"
    check "archive -f" 0 'Created' -- sh -c "cd '$O' && '$M' archive -f"
    check "archive name gets .zip" 0 "Created $O/one.zip" -- "$M" archive "$O/one"
    check "archive name top dir" 0 ' one/moqui' -- unzip -l "$O/one.zip"
    if [ "$kinds" != zip ]; then
        check "archive --tgz name gets .tgz" 0 "Created $O/two.tgz" -- "$M" archive --tgz "$O/two"
        check "archive .tar.gz name is a tgz" 0 ' three/moqui' -- \
            sh -c "'$M' archive '$O/three.tar.gz' && tar -tzf '$O/three.tar.gz' | sed 's/^/ /'"
    fi
    check "archive --tgz with a .zip name" 1 'not four.zip' -- "$M" archive --tgz "$O/four.zip"
    check "archive into a directory" 0 "Created $O/x/mr-moqui-[0-9]{8}\.zip" -- "$M" archive "$O/x/"
    check "archive --with-save" 0 ' saved/save/t1/moqui.dump' -- sh -c "'$M' archive --with-save '$O/saved.zip' && unzip -l '$O/saved.zip'"
    check "archive --conf" 0 'Created' -- "$M" archive -c conf/MoquiDevConf.xml "$O/dev.zip"
    check "archive --with-tests keeps the tests" 0 ' tested/components/base/fw-z/test/acceptance/Walk.spec.js' -- \
        sh -c "'$M' archive --with-tests '$O/tested.zip' && unzip -l '$O/tested.zip'"
    check "archive --with-tests keeps the test set" 0 ' tested/components/test/tc/component.xml' -- unzip -l "$O/tested.zip"
    check "archive --with-tests keeps plan/" 0 ' tested/components/base/fw-z/plan/design.md' -- unzip -l "$O/tested.zip"
    check "archive --with-tests keeps plans/" 0 ' tested/components/base/fw-z/plans/Next.md' -- unzip -l "$O/tested.zip"
    lacks "archive --with-tests still leaves out the dev set" 'components/dev/' -- unzip -l "$O/tested.zip"
    check "archive --with-dev keeps the dev set" 0 ' withdev/components/dev/dv/component.xml' -- \
        sh -c "'$M' archive --with-dev '$O/withdev.zip' && unzip -l '$O/withdev.zip'"
    lacks "archive --with-dev still leaves out the tests" 'fw-z/test/' -- unzip -l "$O/withdev.zip"
    check "archive --no-data" 0 'Created' -- "$M" archive --no-data "$O/clean.zip"
    list=$(unzip -l "$O/clean.zip")
    for unwanted in clean/db clean/.env opensearch/data; do
        lacks "archive --no-data leaves out $unwanted" "$unwanted" -- printf '%s\n' "$list"
    done
    check "archive refuses inside runtime" 1 'is inside runtime' -- "$M" archive "$R/runtime/x.zip"
    ln -s "$R" "$FIX/root-link"
    check "archive refuses inside runtime through a symlink" 1 'is inside runtime' -- "$M" archive "$FIX/root-link/runtime/x.zip"
    check "archive refuses inside runtime by a // path" 1 'is inside runtime' -- "$M" archive "$R//runtime/x.zip"
    rm -f "$FIX/root-link"
    check "archive bad name" 1 'may use only' -- "$M" archive "$O/a b.zip"
    check "archive missing directory" 1 'no directory' -- "$M" archive "$O/nope/a.zip"
    check "archive dry run" 0 'zip -qry .* dry/conf' -- dry "$M" archive "$O/dry"
    check "archive dry run writes nothing" 1 '' -- test -e "$O/dry.zip"
    check "archive dry run leaves no stage" 1 '' -- sh -c "ls -A '$O' | grep -q moq-archive"
    mkdir -p "$R/runtime/db/embedded/data"
    sleep 30 &
    live=$!
    echo "$live" >"$R/runtime/db/embedded/data/postmaster.pid"
    check "archive refuses while running" 1 'is running \(pid' -- "$M" archive "$O/live.zip"
    check "archive --no-data while running" 0 'Created' -- "$M" archive --no-data "$O/live.zip"
    kill "$live" 2>/dev/null
    wait "$live" 2>/dev/null
    rm -f "$R/runtime/db/embedded/data/postmaster.pid"
    rm -f "$R/cargo.log"
    check "archive --binary" 0 "Created $O/bin/moqui-[0-9]{8}\.gz .*production runtime built in" -- \
        sh -c "mkdir -p '$O/bin' && '$M' archive --binary '$O/bin/'"
    check "archive --binary builds with the production runtime" 0 'embed-production -p moqui$' -- cat "$R/cargo.log"
    bin_gz=$(ls "$O/bin/"moqui-*.gz)
    check "archive --binary gzips the binary" 0 '' -- sh -c "gzip -dc '$bin_gz' | cmp - '$R/target/release/moqui'"
    check "archive --binary writes its checksums" 0 '  runtime$' -- cat "${bin_gz%.gz}.sha256"
    check "archive --binary checksum of the .gz" 0 "  $(basename "$bin_gz")\$" -- cat "${bin_gz%.gz}.sha256"
    check "archive --binary exists" 1 'there already; -f replaces them' -- "$M" archive --binary "$bin_gz"
    check "archive --binary --with-tests builds all" 0 'embed-all' -- \
        sh -c "'$M' archive --binary --with-tests -f '$O/bin/all.gz' && cat '$R/cargo.log'"
    check "archive --binary refuses --tgz" 1 'writes the binary alone' -- "$M" archive --binary --tgz
    check "alias add-runtime" 0 './moq archive' -- "$M" add-runtime --help
    check "complete archive" 0 '^--tgz$' -- "$M" __complete 2 archive --t
else
    skipped="$skipped archive"
fi

# The deploy files (SEC-OUT-13): the unit leaves only the runtime and its components writable and keeps the binary and
# the conf read-only in it; the image gives the moqui user neither the binary, the conf nor the runtime directory.
unit=$SRC/systemd/moqui.service
check "unit ProtectSystem strict" 0 '' -- grep -qx 'ProtectSystem=strict' "$unit"
check "unit writes the runtime and its components" 0 '' -- grep -qx 'ReadWritePaths=/opt/moqui /opt/moqui/components' "$unit"
check "unit keeps the binary and the conf read-only" 0 '' -- \
    grep -qx 'ReadOnlyPaths=/opt/moqui/moqui -/opt/moqui/MoquiConf.xml -/opt/moqui/conf' "$unit"
check "image runtime directory root's and sticky" 0 '' -- \
    sh -c "grep -q 'chown root:moqui /opt/moqui ' '$SRC/podman/Containerfile' && grep -q 'chmod 1775 /opt/moqui\$' '$SRC/podman/Containerfile'"
lacks "image gives moqui no binary or runtime directory" 'chown (-R )?moqui:moqui /opt/moqui( |$)|chown .*moqui:moqui .*/opt/moqui/moqui' -- \
    cat "$SRC/podman/Containerfile"
lacks "entrypoint leaves the runtime directory's owner" 'chown .*"\$runtime"' -- cat "$SRC/podman/entrypoint.sh"

printf '%d passed, %d failed%s (bash %s)\n' "$pass" "$fail" "${skipped:+, skipped:$skipped}" "$BASH_VERSION"
[ "$fail" -eq 0 ]
