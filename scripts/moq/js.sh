# shellcheck shell=bash
JS_WEBROOT=$ROOT/runtime/$BASE_SET/webroot
JS_SOURCE=$JS_WEBROOT/build
JS_LIBS=$JS_WEBROOT/screen/webroot/libs
JS_CACHE=${CARGO_TARGET_DIR:-$ROOT/target}/js

js_node() {
    need_tool node npm
    node -e 'process.exit(Number(process.versions.node.split(".")[0]) >= 24 ? 0 : 1)' ||
        die 'Install Node 24 LTS with its bundled npm to build the JavaScript libraries'
    JS_NODE_VERSION=$(node --version)
    JS_NPM_VERSION=$(npm --version)
    export JS_NODE_VERSION JS_NPM_VERSION
}

js_state() {
    perl "$ROOT/scripts/js-state.pl" "$1" "$JS_SOURCE" "$JS_LIBS" "$JS_CACHE"
}

# Lock mutating operations across processes. flock releases the descriptor on exit or interruption.
js_locked() {
    local task=$1
    shift
    dry_run && { show_cmd "$MOQ" "$task" "$@"; return 0; }
    if [ "${MOQ_JS_LOCKED:-}" != "$JS_CACHE" ]; then
        mkdir -p "$(dirname "$JS_CACHE")" || return 1
        MOQ_JS_LOCKED="$JS_CACHE" perl -MFcntl=:flock -e '
            open my $fh, ">>", shift or die $!;
            flock($fh, LOCK_EX) or die $!;
            system @ARGV; exit($? == -1 || ($? & 127) ? 1 : $? >> 8);
        ' "$JS_CACHE.lock" "$MOQ" "$task" "$@"
        return $?
    fi
    local backup=${JS_LIBS%/*}/.libs-previous
    if [ -d "$backup" ]; then
        if [ -d "$JS_LIBS" ]; then rm -rf -- "$backup"; else mv -- "$backup" "$JS_LIBS"; fi || return 1
    fi
    "js_${task//-/_}" "$@"
}

task_build_js() {
    if [ $# -eq 0 ] && [ -f "$JS_LIBS/MANIFEST.json" ] && js_state check; then return 0; fi
    js_locked build-js "$@"
}

js_build_js() {
    local force= arg
    for arg in "$@"; do
        case $arg in
            --force) force=1 ;;
            --offline) local npm_config_offline=true; export npm_config_offline ;;
            *) die "unknown build-js option: $arg" ;;
        esac
    done
    dry_run && { show_cmd "$MOQ" build-js "$@"; return 0; }
    if [ -z "$force" ] && js_state check; then return 0; fi
    js_rebuild --build && js_state stamp
}

js_rebuild() {
    js_node
    mkdir -p "$JS_CACHE" || return 1
    JS_CACHE="$JS_CACHE" node "$JS_SOURCE/libs/refresh.mjs" "$@"
}

task_verify_js() {
    [ $# -eq 0 ] || die "verify-js takes no arguments"
    js_rebuild --verify
}

task_clean_js() { js_locked clean-js "$@"; }

js_clean_js() {
    [ $# -eq 0 ] || die "clean-js takes no arguments"
    local entry
    for entry in "$JS_LIBS"/*; do
        case ${entry##*/} in MANIFEST.json|NOTICES.md|licenses) continue ;; esac
        run_cmd rm -rf -- "$entry" || return 1
    done
    run_cmd rm -rf -- "${JS_CACHE:?}"
}

task_update_js() { js_locked update-js "$@"; }

js_update_js() {
    js_node
    mkdir -p "$JS_CACHE" || return 1
    JS_CACHE="$JS_CACHE" node "$JS_SOURCE/libs/manage.mjs" update "$@" && js_state stamp
}

task_outdated_js() {
    js_node
    node "$JS_SOURCE/libs/manage.mjs" outdated "$@"
}

task_audit_js() {
    js_node
    node "$JS_SOURCE/libs/manage.mjs" audit "$@"
}

help_build_js() { echo 'Install locked packages and build the manifest-verified browser libraries. --force rebuilds. --offline uses cached packages.'; }
help_verify_js() { echo 'Independently rebuild both locked projects and check their outputs against the reviewed manifest.'; }
help_clean_js() { echo 'Remove generated browser libraries and target/js.'; }
help_update_js() { echo 'Update one exact library version or --all, then regenerate the manifest and notices. --security permits a recent security fix.'; }
help_outdated_js() { echo 'List newer library versions and versions past the two-week cooldown.'; }
help_audit_js() { echo 'Audit both projects and inspect the copies embedded in distributed assets.'; }
