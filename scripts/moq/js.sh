# shellcheck shell=bash
# moq's build-js, verify-js, clean-js, update-js, outdated-js, audit-js and fonts-download. Each component with a
# build/js-build.json has JavaScript libraries built from its npm locks (docs/Build-Test.md "Browser libraries"); the
# webroot component is the framework's. Sourced by moq, which holds the task table; bash 3.2, as moq is.

# The webroot's, which the font tasks, the vault module and the source archive use.
JS_WEBROOT=$ROOT/runtime/$BASE_SET/webroot
JS_SOURCE=$JS_WEBROOT/build
JS_LIBS=$JS_WEBROOT/screen/webroot/libs
# Every component's temporary trees and stamp are under target/js/<component>; the lock is target/js.lock.
JS_CACHE=${CARGO_TARGET_DIR:-$ROOT/target}/js

js_node() {
    need_tool node npm
    # 22.13 is the oldest Node a locked package declares (pdfjs-dist); distributions commonly carry 22.
    node -e 'const [a, b] = process.versions.node.split(".").map(Number); process.exit(a > 22 || (a === 22 && b >= 13) ? 0 : 1)' ||
        die 'Install Node 22.13 or later (22 or 24 LTS) with its bundled npm to build the JavaScript libraries'
    JS_NODE_VERSION=$(node --version)
    JS_NPM_VERSION=$(npm --version)
    export JS_NODE_VERSION JS_NPM_VERSION
}

# The components with JavaScript libraries, one line each (name, root, output, script, projects; tab separated), in the
# order components.xml lists their sets.
js_components() {
    perl "$ROOT/scripts/js-state.pl" components "$RUNTIME_DIR/components"
}

# Name each component whose build/js-build.json is invalid (js_components leaves it out); fails when there is one.
js_configs() {
    perl "$ROOT/scripts/js-state.pl" configs "$RUNTIME_DIR/components"
}

# Select one component's line: JS_C_NAME, JS_C_SOURCE (its build/), JS_C_LIBS (the served output), JS_C_SCRIPT,
# JS_C_PROJECTS (comma separated) and JS_C_CACHE.
js_use() {
    local root output script
    IFS=$'\t' read -r JS_C_NAME root output script JS_C_PROJECTS <<<"$1"
    JS_C_SOURCE=$root/build
    JS_C_LIBS=$root/$output
    JS_C_SCRIPT=$JS_C_SOURCE/$script
    if [ "$script" = :standard ]; then JS_C_SCRIPT=$JS_SOURCE/component-libs.mjs; fi
    JS_C_CACHE=$JS_CACHE/$JS_C_NAME
}

js_use_webroot() {
    local line
    line=$(js_components | awk -F'\t' '$1 == "webroot"') || return 1
    [ -n "$line" ] || die "the webroot component has no build/js-build.json"
    js_use "$line"
}

# The lines of the named components, or of all of them with no names; an unknown name stops the task.
js_selected() {
    local all name found line
    all=$(js_components) || return 1
    if [ $# -eq 0 ]; then
        [ -z "$all" ] || printf '%s\n' "$all"
        return 0
    fi
    for name in "$@"; do
        found=
        while IFS= read -r line; do
            [ "${line%%$'\t'*}" = "$name" ] || continue
            printf '%s\n' "$line"
            found=1
        done <<<"$all"
        [ -n "$found" ] || die "no component $name has a build/js-build.json"
    done
}

js_state() {
    JS_SHARED_BUILD="$JS_SOURCE" perl "$ROOT/scripts/js-state.pl" "$1" "$JS_C_SOURCE" "$JS_C_LIBS" "$JS_C_CACHE"
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
    js_recover || return 1
    "js_${task//-/_}" "$@"
}

# Under the lock: put back an output tree a killed publish left renamed, and remove what an interrupted run left, its
# staged tree beside the output and its temporary installs. None is in use while the lock is held.
js_recover() {
    local line backup stale lines
    local dir name
    for stale in "$JS_CACHE"/*/rollback.json; do
        [ -f "$stale" ] || continue
        dir=${stale%/*}
        name=${dir##*/}
        # The journal of a component this checkout no longer has: nothing here to put back, and the other components
        # are not held up by it. A journal whose files were edited since still stops the task (add-library.mjs).
        if ! ls "$RUNTIME_DIR"/components/*/"$name"/component.xml >/dev/null 2>&1; then
            echo "moq: $stale is an interrupted library change of $name, which is not checked out here; left in place" >&2
            continue
        fi
        JS_COMPONENTS="$RUNTIME_DIR/components" JS_CACHE="$JS_CACHE" node "$JS_SOURCE/add-library.mjs" recover "$name" || return 1
    done
    for stale in "$JS_CACHE"/*/prepare-*; do
        [ ! -d "$stale" ] || rm -rf -- "$stale" || return 1
    done
    # An add-js review is applied soon after it is prepared, and each holds whole bundles: a day old, it is dropped.
    [ ! -d "$JS_CACHE" ] || find "$JS_CACHE" -mindepth 2 -maxdepth 2 -name 'review-*.json' -mtime +0 -exec rm -f {} + || return 1
    lines=$(js_components) || return 1
    while IFS= read -r line; do
        [ -n "$line" ] || continue
        js_use "$line"
        backup=${JS_C_LIBS%/*}/.${JS_C_LIBS##*/}-previous
        if [ -d "$backup" ]; then
            if [ -d "$JS_C_LIBS" ]; then rm -rf -- "$backup"; else mv -- "$backup" "$JS_C_LIBS"; fi || return 1
        fi
        for stale in "${JS_C_LIBS%/*}/.${JS_C_LIBS##*/}"-stage-* "$JS_C_CACHE"/build-* "$JS_C_CACHE"/update-*; do
            [ ! -e "$stale" ] || rm -rf -- "$stale" || return 1
        done
    done <<<"$lines"
}

# Each component's output is current: its files match its manifest and its stamp its inputs.
js_all_current() {
    local line lines
    js_configs 2>/dev/null || return 1
    lines=$(js_selected "$@") || return 1
    while IFS= read -r line; do
        [ -n "$line" ] || continue
        js_use "$line"
        [ -f "$JS_C_LIBS/MANIFEST.json" ] && js_state check 2>/dev/null || return 1
    done <<<"$lines"
}

task_build_js() {
    local journal
    for journal in "$JS_CACHE"/*/rollback.json; do [ ! -f "$journal" ] || { js_locked build-js "$@"; return $?; }; done
    if [ $# -eq 0 ] && js_all_current; then return 0; fi
    js_locked build-js "$@"
}

js_build_js() {
    local force= arg line lines status=0 built
    local -a names
    names=()
    for arg in "$@"; do
        case $arg in
            --force) force=1 ;;
            --offline) local npm_config_offline=true; export npm_config_offline ;;
            -*) die "unknown build-js option: $arg" ;;
            *) names+=("$arg") ;;
        esac
    done
    dry_run && { show_cmd "$MOQ" build-js "$@"; return 0; }
    # One component's failure, or its invalid js-build.json, is reported and the others still build (the
    # failure-tolerance rule). A build ended by a signal (Ctrl-C) ends the task.
    js_configs || status=1
    lines=$(js_selected ${names[@]+"${names[@]}"}) || return 1
    while IFS= read -r line; do
        [ -n "$line" ] || continue
        js_use "$line"
        if [ -z "$force" ] && js_state check 2>/dev/null; then continue; fi
        js_rebuild --build
        built=$?
        [ "$built" -le 128 ] || return "$built"
        if [ "$built" -ne 0 ] || ! js_state stamp; then
            echo "moq: could not build the JavaScript libraries of $JS_C_NAME" >&2
            status=1
        fi
    done <<<"$lines"
    return "$status"
}

js_rebuild() {
    js_node
    mkdir -p "$JS_C_CACHE" || return 1
    local JS_PACKS
    JS_PACKS=$(js_state selected) || return 1
    export JS_PACKS
    local projects=
    [ -z "$JS_C_PROJECTS" ] || projects=" (build/${JS_C_PROJECTS//,/, build/})"
    info "npm JavaScript libraries of $JS_C_NAME$projects" >&2
    JS_BUILD_DIR="$JS_C_SOURCE" JS_CACHE="$JS_C_CACHE" JS_OUTPUT="$JS_C_LIBS" node "$JS_C_SCRIPT" "$@"
}

# Locked as well: its temporary install under target/js is what a locked task removes as stale.
task_verify_js() { js_locked verify-js "$@"; }

js_verify_js() {
    local line lines status=0 verified
    js_configs || status=1
    # The standard component library builder's own tests (targets, license and cooldown policy, review, recovery), with
    # the checks of the builds: verify-js is the check for a change to the JS build inputs, and test-all needs no Node.
    js_node
    if [ -f "$ROOT/scripts/test-component-libs.mjs" ]; then
        info "Component library builder tests" >&2
        node --test "$ROOT/scripts/test-component-libs.mjs" || status=1
    fi
    lines=$(js_selected "$@") || return 1
    while IFS= read -r line; do
        [ -n "$line" ] || continue
        js_use "$line"
        js_rebuild --verify
        verified=$?
        [ "$verified" -le 128 ] || return "$verified"
        [ "$verified" -eq 0 ] || { echo "moq: the JavaScript libraries of $JS_C_NAME do not match their manifest" >&2; status=1; }
    done <<<"$lines"
    return "$status"
}

# The built output of every component: each entry of its output directory but the committed manifest, notices and
# license texts. clean-js and clean-all remove these.
js_built_paths() {
    local line lines entry
    lines=$(js_components) || return 1
    while IFS= read -r line; do
        [ -n "$line" ] || continue
        js_use "$line"
        if [ -f "$JS_C_LIBS/MANIFEST.json" ]; then js_state server-files || return 1; fi
        for entry in "$JS_C_LIBS"/*; do
            case ${entry##*/} in MANIFEST.json|NOTICES.md|licenses) continue ;; esac
            [ ! -e "$entry" ] || printf '%s\n' "$entry"
        done
    done <<<"$lines"
}

task_clean_js() { js_locked clean-js "$@"; }

js_clean_js() {
    [ $# -eq 0 ] || die "clean-js takes no arguments"
    local entry paths
    paths=$(js_built_paths) || return 1
    while IFS= read -r entry; do
        [ -z "$entry" ] || run_cmd rm -rf -- "$entry" || return 1
    done <<<"$paths"
    # The font selection is the checkout's choice, not output: the next build-js rebuilds the same packs.
    run_cmd rm -rf -- "${JS_CACHE:?}"
}

# The environment manage.mjs reads for the selected component.
js_manage() {
    JS_BUILD_DIR="$JS_C_SOURCE" JS_OUTPUT="$JS_C_LIBS" JS_SCRIPT="$JS_C_SCRIPT" JS_PROJECT_NAMES="$JS_C_PROJECTS" \
        JS_CACHE="$JS_C_CACHE" node "$JS_SOURCE/libs/manage.mjs" "$@"
}

task_update_js() { js_locked update-js "$@"; }

js_update_js() {
    local component=webroot
    if [ "${1:-}" = --component ]; then
        [ $# -ge 2 ] || die "--component needs a component name"
        component=$2
        shift 2
    fi
    js_node
    local line
    line=$(js_selected "$component") || return 1
    js_use "$line"
    mkdir -p "$JS_C_CACHE" || return 1
    local JS_PACKS
    JS_PACKS=$(js_state selected) || return 1
    export JS_PACKS
    js_validator
    js_manage update "$@" && js_state stamp
}

# outdated-js and audit-js for the named components, or all; each one's result is reported and the others still run.
js_each_manage() {
    local task=$1 line lines status=0 managed
    shift
    js_node
    js_configs || status=1
    lines=$(js_selected "$@") || return 1
    while IFS= read -r line; do
        [ -n "$line" ] || continue
        js_use "$line"
        info "== $JS_C_NAME" >&2
        js_manage "$task"
        managed=$?
        [ "$managed" -le 128 ] || return "$managed"
        [ "$managed" -eq 0 ] || status=1
    done <<<"$lines"
    return "$status"
}

task_outdated_js() { js_each_manage outdated "$@"; }

task_audit_js() { js_each_manage audit "$@"; }

help_build_js() { echo 'Install locked packages and build the manifest-verified JavaScript libraries of each component with a build/js-build.json, or of the named ones. --force rebuilds. --offline uses cached packages.'; }
help_verify_js() { echo 'Independently rebuild the locked projects of each component, or of the named ones, and check their outputs against the reviewed manifests.'; }
help_clean_js() { echo 'Remove the generated client and server libraries of every component and target/js. The font pack selection is kept.'; }
help_update_js() { echo 'Update one exact library version, or --all to the newest ^pin release, then regenerate the manifest and notices. --component name for a component other than webroot (first). --security permits a recent security fix.'; }
help_outdated_js() { echo 'List pins with newer releases (compatible with ^pin, latest and newest, each past the two-week cooldown or not), for each component or the named ones.'; }
help_audit_js() { echo 'Audit the locks of each component, or of the named ones, and the copies embedded in distributed assets.'; }


task_fonts_download() {
    js_use_webroot || return 1
    if [ "$#" -eq 1 ] && [ "$1" = --list ]; then
        js_state packs || return 1
        echo "Installed: $(js_state selected)"
        return
    fi
    js_locked fonts-download "$@"
}

js_fonts_download() {
    js_use_webroot || return 1
    local current selected pack action=add
    current=$(js_state selected) || return 1
    selected=" $current "
    if [ "${1:-}" = --remove ]; then action=remove; shift; fi
    [ $# -gt 0 ] || die "name a font pack or use --list"
    for pack in "$@"; do
        js_state packs | grep -Fxq -- "$pack" || die "unknown font pack $pack"
        if [ "$action" = remove ]; then selected=${selected// "$pack" / }; else selected="$selected$pack "; fi
    done
    selected=$(MOQUI_FONT_PACKS="$selected" js_state selected) || return 1
    MOQUI_FONT_PACKS="$selected" js_build_js webroot || return 1
    printf '%s\n' "$selected" > "$JS_SOURCE/fonts/selected.txt.new" &&
        mv "$JS_SOURCE/fonts/selected.txt.new" "$JS_SOURCE/fonts/selected.txt"
}

js_report_fonts() {
    js_use_webroot || return 1
    local packs
    packs=$(js_state selected) || return 1
    info "Optional font packs: ${packs:-none}" >&2
}

help_fonts_download() { echo 'List optional fonts with --list, add named packs, or --remove named packs. MOQUI_FONT_PACKS overrides the checkout selection.'; }

# Only candidate creation requires an already built QuickJS host (moqui check-js-module, for a server target); a clean
# build verifies reviewed bytes. Left unset when no binary is built, so the task that needs it names ./moq build.
js_validator() {
    if [ -z "${JS_VALIDATOR:-}" ]; then
        local candidate
        for candidate in "$(binary_path debug)" "$(binary_path release)"; do
            if [ -x "$candidate" ]; then JS_VALIDATOR=$candidate; break; fi
        done
    fi
    [ -z "${JS_VALIDATOR:-}" ] || export JS_VALIDATOR
}
task_add_js() {
    case ${1:-} in
        list|search|versions) js_add_js "$@" ;;
        prepare|apply) js_locked add-js "$@" ;;
        *) die 'add-js prepare <component> <id> <npm-entry> <version> <client|server|both> or add-js apply <component> <review-id>' ;;
    esac
}
js_add_js() {
    js_node
    js_validator
    JS_COMPONENTS="$RUNTIME_DIR/components" JS_CACHE="$JS_CACHE" node "$JS_SOURCE/add-library.mjs" "$@"
}
help_add_js() { echo 'Search, prepare and apply a reviewed component library. add-js prepare COMPONENT ID ENTRY VERSION client|server|both; add-js apply COMPONENT REVIEW-ID.'; }
