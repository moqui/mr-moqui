#!/usr/bin/env bash
# Tests for ./moq in a throwaway copy of the repo layout: a stub moqui binary, a small runtime, and local git and zip
# "remotes" for the component tasks. Nothing outside the temp directory is touched.
#   scripts/moq-test.sh
# Runs under bash 3.2 too:
#   docker run --rm -v "$PWD":/w -w /w bash:3.2 bash scripts/moq-test.sh
# Parts that need git, zip, unzip, or curl are skipped when those are missing.
set -o pipefail

SRC=$(cd "$(dirname "$0")/.." && pwd)
FIX=$(mktemp -d "${TMPDIR:-/tmp}/moq-test.XXXXXX")
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

# ========== fixture ==========

R=$FIX/root
mkdir -p "$R/target/release" "$R/runtime/base-component/framework/data" "$R/runtime/base-component/framework/conf" \
    "$R/runtime/conf" "$R/runtime/component" \
    "$R/runtime/db/install/18.6.0" "$R/runtime/log" "$R/runtime/tmp" "$R/runtime/opensearch/data/nodes" "$R/runtime/opensearch/logs"
cp "$SRC/moq" "$R/moq"
chmod +x "$R/moq"
cat >"$R/target/release/moqui" <<'EOF'
#!/bin/sh
# moqui stub: records its arguments and env, and fakes save-db / load-save.
echo "stub: $* conf=${MOQUI_CONF:-}" >>"$(dirname "$0")/calls.log"
case $1 in
    save-db) echo "dump of $2" >"$3" ;;
    load-save) [ -f "$3" ] || exit 3; mkdir -p "$2/data"; cp "$3" "$2/data/restored" ;;
esac
EOF
chmod +x "$R/target/release/moqui"
mkdir -p "$R/bin"
cat >"$R/bin/cargo" <<'EOF'
#!/bin/sh
# cargo stub: records its arguments, builds the stub moqui for the profile, and fails when CARGO_STUB_FAIL is set.
echo "cargo: $*" >>"$(dirname "$0")/../cargo.log"
[ -z "${CARGO_STUB_FAIL:-}" ] || exit 1
[ "$1" != clean ] || exit 0
case " $* " in *" --release "*) p=release ;; *) p=debug ;; esac
mkdir -p "$(dirname "$0")/../target/$p"
cp "$(dirname "$0")/../target/release/moqui.keep" "$(dirname "$0")/../target/$p/moqui"
EOF
chmod +x "$R/bin/cargo"
cp "$R/target/release/moqui" "$R/target/release/moqui.keep"
export PATH="$R/bin:$PATH"
echo '<moqui-conf/>' >"$R/runtime/conf/MoquiDevConf.xml"
echo '<moqui-conf/>' >"$R/runtime/conf/MoquiProductionConf.xml"
echo '<moqui-conf><default-property name="webapp_http_port" value=""/><default-property name="entity_ds_db_conf" value=""/></moqui-conf>' \
    >"$R/runtime/base-component/framework/conf/MoquiDefaultConf.xml"
echo '<entity-facade-xml type="seed">' >"$R/runtime/base-component/framework/data/A.xml"
echo '<entity-facade-xml type="seed-initial,demo">' >"$R/runtime/base-component/framework/data/B.xml"
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

# ========== run and load ==========

check "run options" 0 'export webapp_http_port=8081' -- dry "$M" run -p 8081 -D entity_ds_db_conf=postgres lint --screens
check "run -D" 0 'export entity_ds_db_conf=postgres' -- dry "$M" run -D entity_ds_db_conf=postgres
check "run -Dname=value" 0 'export a_b=c' -- dry "$M" run -Da_b=c
check "run passthrough" 0 'moqui lint --screens$' -- dry "$M" run -p 8081 lint --screens
check "run -- passthrough" 0 'moqui -x$' -- dry "$M" run -- -x
check "run unknown option" 1 'unknown option -x' -- dry "$M" run -x
check "run dotted property" 1 'underscore form.*a_b' -- dry "$M" run -D a.b=1
check "run bad port" 1 'not a number' -- dry "$M" run -p abc
check "conf in runtime" 0 "MOQUI_CONF=$R/runtime/conf/MoquiProductionConf.xml" -- dry "$M" run -c conf/MoquiProductionConf.xml
check "conf from cwd" 0 "MOQUI_CONF=$R/runtime/conf/MoquiDevConf.xml" -- \
    sh -c "cd '$R/runtime/conf' && MOQ_DRY_RUN=1 '$M' run --conf=MoquiDevConf.xml"
check "conf missing" 1 'conf file nope.xml not found' -- dry "$M" run -c nope.xml
check "run-production --debug" 0 'target/debug/moqui$' -- dry "$M" run-production --debug
check "build default" 0 'cargo build --release -p moqui$' -- dry "$M" build
check "build --debug" 0 'cargo build -p moqui$' -- dry "$M" build --debug
check "build passes cargo args" 0 'cargo build -p moqui --features x$' -- dry "$M" build --debug -- --features x
rm -f "$R/cargo.log"
check "run --debug builds a missing debug binary" 0 'is not built, building it' -- "$M" run --debug
check "cargo saw the debug build" 0 'cargo: build -p moqui$' -- cat "$R/cargo.log"
check "debug binary exists now" 0 '' -- test -x "$R/target/debug/moqui"
rm -f "$R/cargo.log"
check "run with the binary there does not build" 0 '' -- "$M" run
check "no cargo call" 1 '' -- test -f "$R/cargo.log"
check "run --build builds anyway" 0 '' -- "$M" run --build
check "cargo saw --build" 0 'cargo: build --release -p moqui$' -- cat "$R/cargo.log"
rm -f "$R/target/release/moqui" "$R/cargo.log"
check "missing release binary is built" 0 'is not built, building it' -- "$M" run
check "cargo saw the release build" 0 'cargo: build --release -p moqui$' -- cat "$R/cargo.log"
rm -f "$R/target/release/moqui" "$R/cargo.log"
check "dry run shows the build" 0 'cargo build --release -p moqui' -- dry "$M" load
check "dry run does not call cargo" 1 '' -- test -f "$R/cargo.log"
check "failed build" 1 'cargo build failed' -- env CARGO_STUB_FAIL=1 "$M" load
"$R/bin/cargo" build --release -p moqui
rm -f "$R/cargo.log"
check "release flag is gone" 1 'unknown option -r' -- "$M" run -r
check "load default all" 0 'moqui load types=all$' -- dry "$M" load
check "load types" 0 'load types=seed\\?,demo components=x$' -- dry "$M" load seed demo components=x
check "load types twice" 1 'not both' -- dry "$M" load seed types=demo
echo '<entity-facade-xml type="seed">' >"$FIX/data.xml"
check "load location from other cwd" 0 "location=$FIX/data.xml" -- sh -c "cd '$FIX' && MOQ_DRY_RUN=1 '$M' load -l data.xml"
check "load location default types" 0 'types=all location=' -- sh -c "cd '$FIX' && MOQ_DRY_RUN=1 '$M' load -l data.xml"
check "load location missing" 1 'data file nope.xml not found' -- dry "$M" load -l nope.xml
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
check "complete -D property" 0 '^entity_ds_db_conf=$' -- "$M" __complete 3 run -D ent
check "complete alias task" 0 '^seed$' -- "$M" __complete 2 lP se
check "complete help" 0 '^git-status$' -- "$M" __complete 2 help git-s

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
check "complete save names" 0 '^t1$' -- "$M" __complete 2 reload-save ''
check "clean-db" 0 'Deleting runtime/db/embedded' -- "$M" clean-db
check "clean-db removed cluster" 1 '' -- test -e "$R/runtime/db/embedded"
check "clean-db removed opensearch data" 1 '' -- test -e "$R/runtime/opensearch/data"
check "clean-db removed opensearch logs" 1 '' -- test -e "$R/runtime/opensearch/logs/gc.log"
check "clean-db kept install" 0 '' -- test -d "$R/runtime/db/install/18.6.0"
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
check "tidy-cargo kept the release binary" 0 'stub:' -- head -n 5 "$R/target/release/moqui"
check "tidy-cargo kept moqui-app" 0 'app' -- cat "$R/target/moqui-app/src/main.rs"
check "tidy-cargo kept an older build-script run" 0 '' -- test -d "$R/target/debug/build/bs-$RUN1"
check "tidy-cargo kept a run with the same fingerprint" 0 '' -- test -d "$R/target/debug/build/bs-$RUN1B"
check "tidy-cargo kept the other build-script run" 0 '' -- test -d "$R/target/debug/build/bs-$RUN2"
check "tidy-cargo kept the older run fingerprint" 0 '' -- test -d "$R/target/debug/.fingerprint/bs-$RUN1"
check "tidy-cargo removed the unreferenced build-script run" 1 '' -- test -e "$R/target/debug/build/bs-$RUNX"
check "tidy-cargo removed the unreferenced run fingerprint" 1 '' -- test -e "$R/target/debug/.fingerprint/bs-$RUNX"
check "tidy-cargo kept both build-script libraries" 0 '' -- test -f "$R/target/debug/deps/libbs-$BS1.rlib" -a -f "$R/target/debug/deps/libbs-$BS2.rlib"
check "tidy-cargo again" 0 'no old cargo artifacts' -- "$M" tidy-cargo

rm -f "$R/cargo.log"
check "clean-all" 0 'Deleting runtime/tmp' -- "$M" clean-all
check "clean-all removed log" 1 '' -- test -e "$R/runtime/log/moqui.log"
check "clean-all did not run cargo clean" 1 '' -- test -f "$R/cargo.log"
check "clean-all runs tidy-cargo" 0 '^tidy-cargo:' -- "$M" clean-all
check "clean-all kept saves" 0 '' -- test -f "$R/runtime/save/t1/moqui.dump"
check "load-save dry run" 0 'save-db .*runtime/save/demo/moqui.dump.part' -- dry "$M" load-save -n demo seed
check "opensearch-start without install" 1 'no OpenSearch' -- "$M" opensearch-start
check "opensearch-stop without pid" 0 'not running' -- "$M" opensearch-stop
check "opensearch-download needs force" 1 'add --force' -- "$M" opensearch-download

# ========== git ==========

if have git; then
    export GIT_AUTHOR_NAME=test GIT_AUTHOR_EMAIL=test@example.com GIT_COMMITTER_NAME=test GIT_COMMITTER_EMAIL=test@example.com
    export GIT_CONFIG_NOSYSTEM=1 HOME=$FIX/home
    git config --global init.defaultBranch master
    git config --global protocol.file.allow always

    # Component "remotes": comp-a depends on comp-b, and a start template.
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
    mkdir -p "$FIX/src/start/screen" "$FIX/src/start/data" "$FIX/src/start/services" "$FIX/src/start/src"
    echo '<screen name="start"/>' >"$FIX/src/start/screen/start.xml"
    echo 'StartApp start' >"$FIX/src/start/data/AppSeedData.xml"
    echo 'api' >"$FIX/src/start/data/ApiSeedData.xml"
    echo 'rest' >"$FIX/src/start/services/start.rest.xml"
    echo '<moqui-conf/>' >"$FIX/src/start/MoquiConf.xml"
    mkrepo start '<component name="start" version="1.0"/>'

    cat >"$R/myaddons.xml" <<EOF
<addons default-repository="local" default="comp-c">
    <!-- <component name="comp-a" group="wrong" branch="nope"/> -->
    <repository name="local">
        <location type="git" url="file://$FIX/repos/\${name}.git"/>
        <location type="current" url="file://$FIX/zips/\${name}-\${branch}.zip"/>
    </repository>
    <component name="comp-a" group="g" branch="master"/>
    <component name="comp-b" group="g" branch="master"/>
    <component name="comp-c" group="g" branch="master"/>
    <component name="comp-skip" group="g" branch="master" skip-get="true"/>
    <component name="start" group="g" branch="master"/>
    <component-set name="outer" sets="inner"/>
    <component-set name="inner" components="comp-c"/>
</addons>
EOF
    (cd "$R" && git init -q && git add moq && git commit -qm root)

    check "complete component names" 0 '^comp-a$' -- "$M" __complete 2 component-get comp
    check "complete set names" 0 '^outer$' -- "$M" __complete 2 component-set ''
    check "component-get with depends" 0 'Checked: comp-a, comp-b' -- "$M" component-get comp-a
    check "component-get cloned dependency" 0 '' -- test -f "$R/runtime/component/comp-b/component.xml"
    check "component-get present" 0 'comp-a is already in' -- "$M" component-get comp-a
    check "component-get skip-get" 0 'Skipping comp-skip' -- "$M" component-get comp-skip
    check "component-get unknown" 1 'not in myaddons.xml or addons.xml' -- "$M" component-get nope
    check "component-get defaults" 0 'Getting comp-c' -- "$M" component-get
    rm -rf "$R/runtime/component/comp-c"
    check "component-set nested" 0 'Getting comp-c' -- "$M" component-set outer
    rm -rf "$R/runtime/component/comp-b"
    check "component-depends" 0 'comp-a depends on comp-b' -- "$M" component-depends
    check "component-get bad type" 1 'not git, current' -- "$M" component-get -t svn comp-a

    check "git in each repo" 0 '== runtime/component/comp-a \(master\)' -- "$M" git rev-parse --abbrev-ref HEAD
    check "git failure exit" 1 'git failed in:' -- "$M" git rev-parse --verify nope
    echo change >>"$R/runtime/component/comp-a/component.xml"
    check "git-status shows change" 0 ' M component.xml' -- "$M" git-status
    lacks "git-status -s hides clean repos" 'comp-b' -- "$M" git-status -s
    echo '<!-- new -->' >>"$FIX/src/comp-b/component.xml"
    (cd "$FIX/src/comp-b" && git commit -qam "second commit" && git push -q "$FIX/repos/comp-b.git" master)
    check "git-pull lists new commits" 1 'second commit' -- "$M" git-pull
    check "git-pull reports the repo without a remote" 1 'git pull failed in: \.$' -- "$M" git-pull
    check "complete git refs" 0 '^master$' -- "$M" __complete 3 git checkout ma
    check "complete git-checkout-all alias refs" 0 '^master$' -- "$M" __complete 2 gitCheckoutAll ma

    check "component-create" 0 'Created runtime/component/my-app from start' -- \
        "$M" component-create my-app --rest --myaddons me --git-remote https://example.com/my-app.git
    check "create renamed file" 0 '' -- test -f "$R/runtime/component/my-app/services/my-app.rest.xml"
    check "create replaced contents" 0 'name="my-app"' -- cat "$R/runtime/component/my-app/component.xml"
    check "create rest drops screens" 1 '' -- test -e "$R/runtime/component/my-app/screen"
    check "create drops src" 1 '' -- test -e "$R/runtime/component/my-app/src"
    check "create new git repo" 0 'example.com/my-app.git' -- git -C "$R/runtime/component/my-app" remote -v
    check "create myaddons line" 0 'name="my-app" group="me"' -- cat "$R/myaddons.xml"
    check "create myaddons still closed" 0 '</addons>' -- tail -n 1 "$R/myaddons.xml"
    check "create exists" 1 'already exists' -- "$M" component-create my-app
    check "create --screens" 0 'Screens only' -- "$M" component-create my-ui --screens --no-git
    check "create --screens drops rest" 1 '' -- test -e "$R/runtime/component/my-ui/services/my-ui.rest.xml"
else
    skipped="$skipped git"
fi

if have git && have zip && have unzip && have curl; then
    mkdir -p "$FIX/zips"
    (cd "$FIX/src" && cp -R comp-c comp-c-master && rm -rf comp-c-master/.git && zip -qr "$FIX/zips/comp-c-master.zip" comp-c-master)
    rm -rf "$R/runtime/component/comp-c"
    check "component-get current (zip)" 0 'Downloaded comp-c' -- "$M" component-get -t current comp-c
    check "zip unpacked one level up" 0 '' -- test -f "$R/runtime/component/comp-c/component.xml"
    check "component-zip" 0 'Created runtime/component/comp-a-1.0.zip' -- "$M" component-zip comp-a
    lacks "component-zip leaves out .git" '\.git/' -- unzip -l "$R/runtime/component/comp-a-1.0.zip"
    check "complete component-zip" 0 '^comp-a$' -- "$M" __complete 2 component-zip comp-a
else
    skipped="$skipped zip"
fi

printf '%d passed, %d failed%s (bash %s)\n' "$pass" "$fail" "${skipped:+, skipped:$skipped}" "$BASH_VERSION"
[ "$fail" -eq 0 ]
