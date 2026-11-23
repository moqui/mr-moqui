# shellcheck shell=bash
# moq's save-db, reload-save, clean-db, clean-log, clean-all and tidy-cargo. Sourced by moq, which holds the task table; bash 3.2, as moq is.

# ========== database and files ==========

task_clean_db() {
    [ $# -eq 0 ] || die "clean-db takes no arguments"
    clean_db
}

clean_db() {
    require_server_stopped clean-db
    rm_path "$CLUSTER_DIR"
    rm_path "$RUNTIME_DIR/txlog"
    if [ -d "$OS_DIR/data" ] || [ -d "$OS_DIR/logs" ]; then
        opensearch_pause
        clean_opensearch_data
        opensearch_resume
    fi
}

task_clean_log() {
    [ $# -eq 0 ] || die "clean-log takes no arguments"
    clean_log
}

clean_log() {
    local f
    for f in "$RUNTIME_DIR"/log/* "$RUNTIME_DIR"/log/.[!.]*; do
        if [ -e "$f" ]; then
            rm_path "$f"
        fi
    done
}

task_clean() {
    task_tidy_cargo "$@"
}

# Collect the exact runtime paths once, before displaying or removing them.
clean_dev_paths() {
    local path
    CLEAN_PATHS=()
    for path in "$CLUSTER_DIR" "$RUNTIME_DIR/txlog" "$OS_DIR/data" "$OS_DIR"/logs/* "$OS_DIR"/logs/.[!.]* \
        "$RUNTIME_DIR"/log/* "$RUNTIME_DIR"/log/.[!.]* "$RUNTIME_DIR/tmp"; do
        if [ -e "$path" ] || [ -L "$path" ]; then CLEAN_PATHS+=("$path"); fi
    done
}

remove_dev_paths() {
    local path pause=
    if [ -d "$OS_DIR/data" ] || [ -d "$OS_DIR/logs" ]; then
        opensearch_pause || return 1
        pause=1
    fi
    local status=0
    for path in "${CLEAN_PATHS[@]}"; do rm_path "$path" || status=1; done
    if [ -n "$pause" ]; then opensearch_resume || status=1; fi
    return "$status"
}

clean_dev() {
    require_server_stopped clean-dev
    clean_dev_paths
    remove_dev_paths
}

task_clean_dev() {
    [ $# -eq 0 ] || die "clean-dev takes no arguments"
    clean_dev
}

task_clean_all() {
    local yes= path arg answer target=$ROOT/target
    for arg in "$@"; do
        case "$arg" in --yes) yes=1 ;; --dry-run) local MOQ_DRY_RUN=1 ;; *) die "unknown clean-all option: $arg" ;; esac
    done
    require_server_stopped clean-all
    clean_dev_paths
    local entry
    local -a js_paths
    local vault=$JS_WEBROOT/screen/webroot/js/vault/moqui_vault.wasm
    # runtime/types is build output (moqui types), which an editor's jsconfig reads: clean-all removes it, clean-dev keeps it
    js_paths=("$RUNTIME_DIR/types" "$vault" "$vault.json")
    [ "$JS_CACHE" = "$target/js" ] || js_paths+=("$JS_CACHE")
    local built
    built=$(js_built_paths) || return 1
    while IFS= read -r entry; do
        [ -z "$entry" ] || js_paths+=("$entry")
    done <<<"$built"
    info "Removing:"
    for path in "${CLEAN_PATHS[@]}" "${js_paths[@]}" "$target"; do
        if [ -e "$path" ] || [ -L "$path" ]; then du -sh "$path" || return 1; fi
    done
    info "Keeping downloads, certificates, local configuration, the font pack selection, saves, worktrees and named exports."
    dry_run && return 0
    if [ -z "$yes" ]; then
        [ -t 0 ] || die "clean-all needs a terminal or --yes"
        read -r -p "Remove the paths listed above? [y/N] " answer
        case "$answer" in y|Y|yes|YES) ;; *) die "clean-all cancelled" ;; esac
    fi
    need_tool cargo
    remove_dev_paths || return 1
    for entry in "${js_paths[@]}"; do run_cmd rm -rf -- "$entry" || return 1; done
    run_cmd cargo clean --target-dir "$target"
}

task_tidy_cargo() {
    [ $# -eq 0 ] || die "tidy-cargo takes no arguments"
    tidy_cargo
}

# Drop old cargo artifacts under target/ and keep the current build.
#
# Cargo's metadata hash (the 16 hex digits in deps/ and .fingerprint/, or the unit directory
# build/<package>/<hash>/ in the build directory layout of Cargo 1.100 and later) changes with the
# compiler, profile, features, and dependency graph, not with a source edit. Cargo never deletes
# the previous hash, so repeated builds leave a full copy of each unit. rustc already keeps only the newest
# incremental session inside one incremental/<crate>-<id>/ directory; the waste is the other
# directories. The fingerprint json identifies a unit. The newest hash of each unit is the one cargo
# will look up next. An older hash of that same unit is unreachable. A run-build-script fingerprint
# leaves profile, features, and path at 0, so two live runs of one package compare equal; the library
# json names the run it compiled against, and that run's build/ directory stays. Cargo 1.100 and
# later never read the earlier layout (.fingerprint/, deps/, build/<package>-<hash>/), so once a
# profile has units in the new one, every unit of the earlier layout there goes.
tidy_cargo() {
    local tmp
    tmp=$(mktemp -d "${TMPDIR:-/tmp}/moq-tidy.XXXXXX") || die "could not make a temp directory"
    (
        trap 'rm -rf "$tmp"' EXIT
        TIDY_TMP=$tmp
        tidy_cargo_run
    ) || exit $?
}

tidy_cargo_run() {
    local list=$TIDY_TMP/profiles profile
    [ -d "$ROOT/target" ] || {
        info "tidy-cargo: no target/ yet; nothing to remove"
        return 0
    }
    # A profile is target/<profile> or target/<triple>/<profile>, with .fingerprint (the earlier
    # layout) or build/ in it. Do not walk into deps/ or incremental/, and never into the generated
    # crate at target/moqui-app.
    find "$ROOT/target" -maxdepth 3 \
        \( -path "$ROOT/target/js" -o -path "$ROOT/target/moqui-app" -o -path "$ROOT/target/moqui-app/*" \) -prune -o \
        -type d \( -name .fingerprint -o -name build \) -print >"$TIDY_TMP/found" \
        || die "tidy-cargo: could not read target/"
    while IFS= read -r profile; do
        dirname "$profile"
    done <"$TIDY_TMP/found" | sort -u >"$list"
    if [ ! -s "$list" ]; then
        info "tidy-cargo: no old cargo artifacts to remove; the current build stays"
        return 0
    fi
    tidy_lock_profiles "$list"
    info "tidy-cargo: checking target/ for old artifacts"
    # One stat per file made this sit for a minute with no output. Batch the stats.
    if stat -c %Y "$ROOT/moq" >/dev/null 2>&1; then
        TIDY_STAT_GNU=1
    else
        TIDY_STAT_GNU=0
    fi
    tidy_write_manifest_awk
    : >"$TIDY_TMP/manifest"
    while IFS= read -r profile; do
        [ -n "$profile" ] || continue
        tidy_manifest_profile "$profile" >>"$TIDY_TMP/manifest" || die "tidy-cargo: could not read $(rel "$profile")"
    done <"$list"
    need_tool awk
    cat >"$TIDY_TMP/tidy.awk" <<'EOF'
function json_string(s, key,    i, n, c, esc, out, marker) {
    marker = "\"" key "\":\""
    i = index(s, marker)
    if (i == 0) return ""
    i += length(marker)
    n = length(s)
    esc = 0
    out = ""
    for (; i <= n; i++) {
        c = substr(s, i, 1)
        if (esc) {
            out = out c
            esc = 0
            continue
        }
        if (c == "\\") {
            esc = 1
            continue
        }
        if (c == "\"") return out
        out = out c
    }
    return ""
}

# A number, or a whole array or object. A JSON string is handed to json_string.
function json_value(s, key,    i, n, c, depth, out, in_str, esc, rest, marker) {
    marker = "\"" key "\":"
    i = index(s, marker)
    if (i == 0) return ""
    rest = substr(s, i + length(marker))
    if (substr(rest, 1, 1) == "\"") return json_string(s, key)
    n = length(rest)
    depth = 0
    in_str = 0
    esc = 0
    out = ""
    for (i = 1; i <= n; i++) {
        c = substr(rest, i, 1)
        if (in_str) {
            out = out c
            if (esc) {
                esc = 0
                continue
            }
            if (c == "\\") {
                esc = 1
                continue
            }
            if (c == "\"") in_str = 0
            continue
        }
        if (c == "\"") {
            in_str = 1
            out = out c
            continue
        }
        if (c == "[" || c == "{") {
            depth++
            out = out c
            continue
        }
        if (c == "]" || c == "}") {
            if (depth == 0) break
            depth--
            out = out c
            if (depth == 0) return out
            continue
        }
        if (c == "," && depth == 0) break
        out = out c
    }
    return out
}

function dep_names(deps,    s, name, list, n, i, j, tmp, out) {
    s = deps
    n = 0
    while (match(s, /,"[A-Za-z0-9_+-]+",(true|false)/)) {
        name = substr(s, RSTART + 2, RLENGTH - 2)
        sub(/",(true|false)$/, "", name)
        n++
        list[n] = name
        s = substr(s, RSTART + RLENGTH)
    }
    for (i = 2; i <= n; i++) {
        tmp = list[i]
        j = i - 1
        while (j >= 1 && list[j] > tmp) {
            list[j + 1] = list[j]
            j--
        }
        list[j + 1] = tmp
    }
    out = ""
    for (i = 1; i <= n; i++) {
        if (i > 1) out = out ","
        out = out list[i]
    }
    return out
}

function near(list, t,    n, i, a, d) {
    n = split(list, a, " ")
    for (i = 1; i <= n; i++) {
        if (a[i] == "") continue
        d = t - a[i]
        if (d < 0) d = -d
        if (d <= 10) return 1
    }
    return 0
}

# dep[3] of build_script_build / build_script_main, as digit strings. Those integers do not fit
# in an awk number, so they stay text.
function script_dep_ids(deps,    s, num, out) {
    s = deps
    out = ""
    while (match(s, /,"build_script_(build|main)",(true|false),[0-9]+/)) {
        num = substr(s, RSTART, RLENGTH)
        sub(/.*,/, "", num)
        if (out != "") out = out SUBSEP
        out = out num
        s = substr(s, RSTART + RLENGTH)
    }
    return out
}

# Decimal string times a small integer, plus a small integer.
function dec_mul_add(d, m, a,    i, carry, out, digit) {
    carry = a
    out = ""
    for (i = length(d); i >= 1; i--) {
        digit = substr(d, i, 1) * m + carry
        carry = int(digit / 10)
        out = (digit % 10) out
    }
    while (carry > 0) {
        out = (carry % 10) out
        carry = int(carry / 10)
    }
    sub(/^0+/, "", out)
    if (out == "") out = "0"
    return out
}

function hex_dec(h,    i, d, c, v) {
    d = "0"
    for (i = 1; i <= length(h); i++) {
        c = substr(h, i, 1)
        v = index("0123456789abcdef", c) - 1
        if (v < 0) return ""
        d = dec_mul_add(d, 16, v)
    }
    return d
}

# Cargo writes the fingerprint as 16 hex digits. The dep list stores that value little-endian.
function hex_le_dec(h,    i, rev) {
    h = tolower(h)
    if (h !~ /^[0-9a-f]{16}$/) return ""
    rev = ""
    for (i = 7; i >= 0; i--) rev = rev substr(h, i * 2 + 1, 2)
    return hex_dec(rev)
}

# Map the fingerprint-file value to every metadata hash that carries it. Two live runs of one
# package can store the same 16 hex digits, so keeping only the last hash drops a real output.
function note_fp_file(path, hash,    hf, hline, dec) {
    hf = path
    sub(/\.json$/, "", hf)
    if ((getline hline < hf) > 0 && hline ~ /^[0-9a-fA-F]{16}$/) {
        dec = hex_le_dec(hline)
        if (dec == "") {
            close(hf)
            return
        }
        if (dec in run_meta) run_meta[dec] = run_meta[dec] SUBSEP hash
        else run_meta[dec] = hash
    }
    close(hf)
}

# A kept unit's build_script dep names the run (and that run names the build-script binary).
function keep_build_scripts(    changed, i, n, h, m, j, rn, k) {
    do {
        changed = 0
        n = 0
        for (h in keep) {
            n++
            klist[n] = h
        }
        for (i = 1; i <= n; i++) {
            h = klist[i]
            if (!(h in script_deps)) continue
            m = split(script_deps[h], sdep, SUBSEP)
            for (j = 1; j <= m; j++) {
                if (!(sdep[j] in run_meta)) continue
                rn = split(run_meta[sdep[j]], rhash, SUBSEP)
                for (k = 1; k <= rn; k++) {
                    if (rhash[k] != "" && !(rhash[k] in keep)) {
                        keep[rhash[k]] = 1
                        changed = 1
                    }
                }
            }
        }
    } while (changed)
}

BEGIN { FS = "\t" }

$1 == "K" {
    force[$2] = 1
    next
}

# A unit of the earlier layout in a profile Cargo 1.100 and later builds: never read again.
$1 == "X" {
    print $2
    next
}

# An artifact in a unit directory of the build directory layout: its stem, for incremental/ only.
$1 == "S" {
    ns++
    s_hash[ns] = $2
    s_stem[ns] = $3
    s_mtime[ns] = $4 + 0
    next
}

$1 == "F" {
    hash = $2
    mtime = $3 + 0
    pkg = $4
    kind = $5
    path = $6
    line = ""
    if ((getline line < path) <= 0) {
        force[hash] = 1
        unparsed++
        close(path)
        next
    }
    close(path)
    if (substr(line, 1, 1) != "{" || index(line, "\"features\":") == 0) {
        force[hash] = 1
        unparsed++
        next
    }
    sd = script_dep_ids(json_value(line, "deps"))
    if (sd != "") {
        if (hash in script_deps) script_deps[hash] = script_deps[hash] SUBSEP sd
        else script_deps[hash] = sd
    }
    note_fp_file(path, hash)
    # profile, features, and path are 0 on a build-script run, so newest-wins would drop a run
    # that another kept library still compiled against.
    if (kind ~ /^run-build-script/) next
    key = pkg SUBSEP kind SUBSEP json_string(line, "features") SUBSEP json_value(line, "path") \
        SUBSEP json_value(line, "profile") SUBSEP json_value(line, "rustc") SUBSEP json_value(line, "target") \
        SUBSEP json_value(line, "compile_kind") SUBSEP json_value(line, "config") \
        SUBSEP json_value(line, "rustflags") SUBSEP dep_names(json_value(line, "deps"))
    if (!(key in best_m) || mtime > best_m[key]) {
        best_m[key] = mtime
        best_h[key] = hash
    } else if (mtime == best_m[key]) {
        best_h[key] = best_h[key] SUBSEP hash
    }
    next
}

$1 == "D" {
    nd++
    d_path[nd] = $2
    d_hash[nd] = $3
    d_stem[nd] = $4
    d_mtime[nd] = $5 + 0
    next
}
$1 == "B" {
    nb++
    b_path[nb] = $2
    b_hash[nb] = $3
    next
}
$1 == "I" {
    ni++
    i_path[ni] = $2
    i_stem[ni] = $3
    i_mtime[ni] = $4 + 0
    next
}
$1 == "P" {
    np++
    p_path[np] = $2
    p_hash[np] = $3
    next
}

END {
    for (key in best_h) {
        n = split(best_h[key], hs, SUBSEP)
        for (i = 1; i <= n; i++) keep[hs[i]] = 1
    }
    for (h in force) keep[h] = 1
    keep_build_scripts()

    for (i = 1; i <= nd; i++) {
        if (keep[d_hash[i]]) {
            stem_n[d_stem[i]]++
            stem_mt[d_stem[i]] = stem_mt[d_stem[i]] " " d_mtime[i]
        } else {
            print d_path[i]
        }
    }
    for (i = 1; i <= ns; i++) {
        if (!keep[s_hash[i]]) continue
        stem_n[s_stem[i]]++
        stem_mt[s_stem[i]] = stem_mt[s_stem[i]] " " s_mtime[i]
    }
    for (i = 1; i <= nb; i++) if (!keep[b_hash[i]]) print b_path[i]
    for (i = 1; i <= np; i++) if (!keep[p_hash[i]]) print p_path[i]

    for (i = 1; i <= ni; i++) {
        if (stem_n[i_stem[i]] > 0) continue
        if (!(i_stem[i] in inc_best) || i_mtime[i] > inc_best[i_stem[i]]) inc_best[i_stem[i]] = i_mtime[i]
    }
    for (i = 1; i <= ni; i++) {
        if (stem_n[i_stem[i]] > 0) {
            if (!near(stem_mt[i_stem[i]], i_mtime[i])) print i_path[i]
        } else if (i_mtime[i] + 0 < inc_best[i_stem[i]] + 0) {
            print i_path[i]
        }
    }
    print unparsed + 0 > (tmp "/unparsed")
}
EOF
    awk -v tmp="$TIDY_TMP" -f "$TIDY_TMP/tidy.awk" "$TIDY_TMP/manifest" >"$TIDY_TMP/delete" \
        || die "tidy-cargo: could not read cargo fingerprints"
    local n
    n=$(cat "$TIDY_TMP/unparsed" 2>/dev/null || echo 0)
    if [ "$n" -gt 0 ]; then
        if [ "$n" -eq 1 ]; then
            warn "tidy-cargo: kept 1 fingerprint file that was not cargo fingerprint json"
        else
            warn "tidy-cargo: kept $n fingerprint files that were not cargo fingerprint json"
        fi
    fi
    if [ ! -s "$TIDY_TMP/delete" ]; then
        info "tidy-cargo: no old cargo artifacts to remove; the current build stays"
        return 0
    fi
    local path kb
    while IFS= read -r path; do
        [ -n "$path" ] || continue
        tidy_check_build_path "$path"
    done <"$TIDY_TMP/delete"
    kb=$(tidy_du_kb "$TIDY_TMP/delete")
    if dry_run; then
        info "tidy-cargo: would remove $(tidy_human_kb "$kb") of old artifacts under target/; the current build of each package stays"
        return 0
    fi
    info "tidy-cargo: removing $(tidy_human_kb "$kb") of old artifacts under target/"
    while IFS= read -r path; do
        [ -n "$path" ] || continue
        rm -rf -- "$path"
        # build/<package> once its last unit directory is gone.
        case ${path#"$ROOT/target/"} in
            */build/*/*) rmdir -- "${path%/*}" 2>/dev/null || true ;;
        esac
    done <"$TIDY_TMP/delete"
    info "tidy-cargo: removed $(tidy_human_kb "$kb") of old artifacts under target/; the current build of each package stays"
}

# Hold every profile lock for the rest of this process. A build waiting on the lock starts after
# tidy finishes. bash 3.2 has no dynamic file descriptors, so the numbers are fixed.
tidy_lock_profiles() {
    command -v flock >/dev/null 2>&1 || return 0
    local list=$1 profile lock n=0 fd
    while IFS= read -r profile; do
        [ -n "$profile" ] || continue
        # .cargo-lock is the artifact directory's, .cargo-build-lock the build directory's (Cargo 1.100).
        for lock in "$profile/.cargo-lock" "$profile/.cargo-build-lock"; do
            [ -e "$lock" ] || continue
            n=$((n + 1))
            fd=$((19 + n))
            tidy_open_lock "$fd" "$lock"
            flock -n "$fd" || die "tidy-cargo: a cargo build is using $(rel "$profile"); stop it first"
        done
    done <"$list"
}

tidy_open_lock() {
    case $1 in
        20) exec 20<"$2" || die "tidy-cargo: could not open $2" ;;
        21) exec 21<"$2" || die "tidy-cargo: could not open $2" ;;
        22) exec 22<"$2" || die "tidy-cargo: could not open $2" ;;
        23) exec 23<"$2" || die "tidy-cargo: could not open $2" ;;
        24) exec 24<"$2" || die "tidy-cargo: could not open $2" ;;
        25) exec 25<"$2" || die "tidy-cargo: could not open $2" ;;
        26) exec 26<"$2" || die "tidy-cargo: could not open $2" ;;
        27) exec 27<"$2" || die "tidy-cargo: could not open $2" ;;
        28) exec 28<"$2" || die "tidy-cargo: could not open $2" ;;
        29) exec 29<"$2" || die "tidy-cargo: could not open $2" ;;
        30) exec 30<"$2" || die "tidy-cargo: could not open $2" ;;
        31) exec 31<"$2" || die "tidy-cargo: could not open $2" ;;
        32) exec 32<"$2" || die "tidy-cargo: could not open $2" ;;
        33) exec 33<"$2" || die "tidy-cargo: could not open $2" ;;
        34) exec 34<"$2" || die "tidy-cargo: could not open $2" ;;
        35) exec 35<"$2" || die "tidy-cargo: could not open $2" ;;
        *) die "tidy-cargo: too many cargo target directories to lock" ;;
    esac
}

# stdin: one path per line. stdout: mtime, a tab, the path. GNU stat, else BSD stat.
tidy_batch_stat() {
    local line
    local -a batch
    batch=()
    while IFS= read -r line || [ -n "$line" ]; do
        [ -n "$line" ] || continue
        batch+=("$line")
        if [ "${#batch[@]}" -ge 200 ]; then
            tidy_stat_batch "${batch[@]}" || return 1
            batch=()
        fi
    done
    [ "${#batch[@]}" -eq 0 ] || tidy_stat_batch "${batch[@]}"
}

tidy_stat_batch() {
    if [ "$TIDY_STAT_GNU" = 1 ]; then
        stat -c $'%Y\t%n' "$@"
    else
        stat -f $'%m\t%N' "$@"
    fi
}

tidy_write_manifest_awk() {
    cat >"$TIDY_TMP/manifest.awk" <<'EOF'
function base(path) {
    sub(/.*\//, "", path)
    return path
}

function dir_hash(name) {
    if (match(name, /-[0-9a-f]{16}$/)) return substr(name, RSTART + 1, 16)
    return ""
}

function file_hash(name) {
    if (match(name, /-[0-9a-f]{16}(\.[^.]*)?$/)) return substr(name, RSTART + 1, 16)
    return ""
}

function file_stem(name,    stem) {
    if (file_hash(name) == "") return ""
    stem = name
    sub(/-[0-9a-f]{16}(\.[^.]*)?$/, "", stem)
    if (name ~ /\.(rlib|rmeta|so|dylib|a|dll)$/ && stem ~ /^lib./) sub(/^lib/, "", stem)
    return stem
}

function incr_stem(name,    stem) {
    stem = name
    sub(/-[^-]*$/, "", stem)
    if (stem == name) return ""
    return stem
}

function up(path) {
    sub(/\/[^\/]+$/, "", path)
    return path
}

# A unit directory of the build directory layout: build/<package>/<hash>.
function is_unit(dir) {
    return base(dir) ~ /^[0-9a-f]{16}$/ && base(up(up(dir))) == "build"
}

# A path of the earlier layout goes whole (X) when the profile has units in the new layout.
function old_unit(line, path) {
    if (newlayout) printf "X\t%s\n", path
    else print line
}

BEGIN { FS = "\t" }

{
    mtime = $1 + 0
    path = $2
    name = base(path)
    parent = up(path)
    parent_name = base(parent)
    grand = up(parent)

    if (is_unit(path)) {
        new_unit[path] = name
    } else if (parent_name == "fingerprint" && is_unit(grand)) {
        if (name == "invoked.timestamp") {
            stamp[grand] = mtime
        } else if (name ~ /\.json$/) {
            json_n++
            json_path[json_n] = path
            json_dir[json_n] = grand
            json_name[json_n] = name
            json_m[json_n] = mtime
            json_new[json_n] = 1
            has_json[grand] = 1
        }
    } else if (parent_name == "out" && is_unit(grand)) {
        art_n++
        art_name[art_n] = name
        art_hash[art_n] = base(grand)
        art_m[art_n] = mtime
    } else if (parent_name == ".fingerprint") {
        unit[path] = name
    } else if (base(grand) == ".fingerprint" && name == "invoked.timestamp") {
        stamp[parent] = mtime
    } else if (base(grand) == ".fingerprint" && name ~ /\.json$/) {
        json_n++
        json_path[json_n] = path
        json_dir[json_n] = parent
        json_name[json_n] = name
        json_m[json_n] = mtime
        has_json[parent] = 1
    } else if (parent_name == "deps") {
        dep_n++
        dep_path[dep_n] = path
        dep_m[dep_n] = mtime
    } else if (parent_name == "build") {
        build_n++
        build_path[build_n] = path
        build_name[build_n] = name
    } else if (parent_name == "incremental") {
        inc_n++
        inc_path[inc_n] = path
        inc_name[inc_n] = name
        inc_m[inc_n] = mtime
    }
}

END {
    for (path in new_unit) {
        hash = new_unit[path]
        printf "P\t%s\t%s\n", path, hash
        if (!has_json[path]) printf "K\t%s\n", hash
    }
    for (path in unit) {
        hash = dir_hash(unit[path])
        if (hash == "") continue
        if (newlayout) {
            printf "X\t%s\n", path
            continue
        }
        printf "P\t%s\t%s\n", path, hash
        if (!has_json[path]) printf "K\t%s\n", hash
    }
    for (i = 1; i <= json_n; i++) {
        dir = json_dir[i]
        if (json_new[i]) {
            hash = base(dir)
            pkg = base(up(dir))
        } else {
            if (newlayout) continue
            hash = dir_hash(base(dir))
            if (hash == "") continue
            pkg = base(dir)
            sub(/-[0-9a-f]{16}$/, "", pkg)
        }
        kind = json_name[i]
        sub(/\.json$/, "", kind)
        m = (dir in stamp) ? stamp[dir] : json_m[i]
        printf "F\t%s\t%.0f\t%s\t%s\t%s\n", hash, m, pkg, kind, json_path[i]
    }
    for (i = 1; i <= art_n; i++) {
        stem = file_stem(art_name[i])
        if (stem == "") continue
        printf "S\t%s\t%s\t%.0f\n", art_hash[i], stem, art_m[i]
    }
    for (i = 1; i <= dep_n; i++) {
        name = base(dep_path[i])
        hash = file_hash(name)
        if (hash == "") continue
        old_unit(sprintf("D\t%s\t%s\t%s\t%.0f", dep_path[i], hash, file_stem(name), dep_m[i]), dep_path[i])
    }
    for (i = 1; i <= build_n; i++) {
        hash = dir_hash(build_name[i])
        if (hash == "") continue
        old_unit(sprintf("B\t%s\t%s", build_path[i], hash), build_path[i])
    }
    for (i = 1; i <= inc_n; i++) {
        stem = incr_stem(inc_name[i])
        if (stem == "") continue
        printf "I\t%s\t%s\t%.0f\n", inc_path[i], stem, inc_m[i]
    }
}
EOF
}

# One manifest line per fingerprint json, deps file, build dir, incremental dir, and fingerprint dir,
# and in the build directory layout of Cargo 1.100 and later per unit directory
# (build/<package>/<hash>), its fingerprint json, and each file in its out/.
tidy_manifest_profile() {
    local profile=$1 fp=$profile/.fingerprint newlayout=0
    if [ -d "$profile/build" ] && [ -n "$(find "$profile/build" -mindepth 3 -maxdepth 3 -type d -name fingerprint \
        -path '*/build/*/[0-9a-f]*/fingerprint' -print | head -n 1)" ]; then
        newlayout=1
    fi
    {
        if [ -d "$fp" ]; then
            find "$fp" -mindepth 1 -maxdepth 1 -type d -print
            find "$fp" -mindepth 2 -maxdepth 2 \( -name '*.json' -o -name 'invoked.timestamp' \) -print
            if [ -d "$profile/deps" ]; then
                find "$profile/deps" -maxdepth 1 -type f -print
            fi
        fi
        if [ -d "$profile/build" ]; then
            find "$profile/build" -mindepth 1 -maxdepth 2 -type d -print
            find "$profile/build" -mindepth 4 -maxdepth 4 \( -path '*/fingerprint/*' \( -name '*.json' -o -name 'invoked.timestamp' \) \
                -o -path '*/out/*' -type f \) -print
        fi
        if [ -d "$profile/incremental" ]; then
            find "$profile/incremental" -mindepth 1 -maxdepth 1 -type d -print
        fi
    } | tidy_batch_stat | awk -v newlayout="$newlayout" -f "$TIDY_TMP/manifest.awk"
}

# Refuse anything that is not one artifact file or directory under target/.
tidy_check_build_path() {
    local rest parent leaf
    case $1 in
        "$ROOT/target/moqui-app" | "$ROOT/target/moqui-app"/*)
            die "refusing to delete $(rel "$1")"
            ;;
        "$ROOT"/target/*) ;;
        *) die "refusing to delete $(rel "$1"), which is not under target/" ;;
    esac
    rest=${1#"$ROOT/target/"}
    leaf=${rest##*/}
    parent=${rest%/*}
    case $leaf in
        "" | .* | */*) die "refusing to delete $(rel "$1")" ;;
    esac
    case $parent in
        */.fingerprint | .fingerprint | */deps | deps | */build | build | */incremental | incremental) ;;
        */build/* | build/*)
            # A unit directory of the build directory layout: build/<package>/<16 hex digits>.
            case ${parent##*/build/} in
                "" | */*) die "refusing to delete $(rel "$1")" ;;
            esac
            case $leaf in
                *[!0-9a-f]*) die "refusing to delete $(rel "$1")" ;;
            esac
            [ "${#leaf}" -eq 16 ] || die "refusing to delete $(rel "$1")"
            ;;
        *) die "refusing to delete $(rel "$1")" ;;
    esac
}

tidy_du_kb() {
    local list=$1 path kb s=0
    [ -s "$list" ] || {
        echo 0
        return 0
    }
    if du --files0-from=/dev/null -sk >/dev/null 2>&1; then
        tr '\n' '\0' <"$list" >"$TIDY_TMP/delete0"
        du --files0-from="$TIDY_TMP/delete0" -sk | awk '{s += $1} END {print s + 0}'
        return 0
    fi
    while IFS= read -r path; do
        [ -n "$path" ] || continue
        kb=$(du -sk "$path" | awk '{print $1}')
        s=$((s + kb))
    done <"$list"
    echo "$s"
}

tidy_human_kb() {
    awk -v k="$1" 'BEGIN {
        if (k < 1024) printf "%dK", k + 0
        else if (k < 1048576) printf "%.1fM", k / 1024
        else printf "%.1fG", k / 1048576
    }'
}

# Sets SAVE_NAME and SKIP_OPENSEARCH, and OPT_PACKAGE for any_binary.
parse_save_args() {
    local task=$1
    shift
    SAVE_NAME=default
    SKIP_OPENSEARCH=
    OPT_PACKAGE=
    OPT_NOBUILD=
    local named=''
    while [ $# -gt 0 ]; do
        case $1 in
            --lite) set_package lite ;;
            --debug) set_package debug ;;
            --release) set_package release ;;
            --enterprise) set_package enterprise ;;
            --skip-opensearch) SKIP_OPENSEARCH=1 ;;
            --nobuild) OPT_NOBUILD=1 ;;
            -*) die "unknown option $1" ;;
            *)
                [ -z "$named" ] || die "$task takes one save name"
                SAVE_NAME=$1
                named=1
                ;;
        esac
        shift
    done
    valid_name "$SAVE_NAME" || die "save name '$SAVE_NAME' may use only letters, digits, '.', '_' and '-'"
}

task_save_db() {
    local bin dir
    parse_save_args save-db "$@"
    if [ ! -d "$CLUSTER_DIR/data" ]; then
        dry_run || die "no embedded database in $(rel "$CLUSTER_DIR"); load data first"
        warn "no embedded database in $(rel "$CLUSTER_DIR") yet"
    fi
    bin=$(any_binary) || exit 1
    dir=$SAVE_DIR/$SAVE_NAME
    run_cmd mkdir -p "$dir" || die "could not make $(rel "$dir")"
    info "Saving the embedded database to $(rel "$dir/moqui.dump")"
    run_moqui "$bin" save-db "$CLUSTER_DIR" "$dir/moqui.dump.part" || die "save-db failed"
    run_cmd mv "$dir/moqui.dump.part" "$dir/moqui.dump"
    if [ -z "$SKIP_OPENSEARCH" ] && [ -d "$OS_DIR/data" ]; then
        info "Saving OpenSearch data to $(rel "$dir/opensearch-data.tar.gz")"
        opensearch_pause
        if run_cmd tar -czf "$dir/opensearch-data.tar.gz.part" -C "$OS_DIR" data; then
            run_cmd mv "$dir/opensearch-data.tar.gz.part" "$dir/opensearch-data.tar.gz"
        else
            run_cmd rm -f "$dir/opensearch-data.tar.gz.part"
            warn "the OpenSearch data was not saved"
        fi
        opensearch_resume
    elif [ -f "$dir/opensearch-data.tar.gz" ]; then
        # An older copy would not match this database.
        run_cmd rm -f "$dir/opensearch-data.tar.gz"
    fi
    info "Saved $(rel "$dir")"
}

task_reload_save() {
    local bin dir
    parse_save_args reload-save "$@"
    dir=$SAVE_DIR/$SAVE_NAME
    [ -f "$dir/moqui.dump" ] || die "no save named $SAVE_NAME (no $(rel "$dir/moqui.dump"))"
    require_server_stopped reload-save
    bin=$(any_binary) || exit 1
    rm_path "$CLUSTER_DIR"
    rm_path "$RUNTIME_DIR/txlog"
    info "Restoring the embedded database from $(rel "$dir/moqui.dump")"
    run_moqui "$bin" load-save "$CLUSTER_DIR" "$dir/moqui.dump" || die "load-save failed"
    if [ -z "$SKIP_OPENSEARCH" ] && { [ -d "$OS_DIR/data" ] || [ -f "$dir/opensearch-data.tar.gz" ]; }; then
        opensearch_pause
        clean_opensearch_data
        if [ -f "$dir/opensearch-data.tar.gz" ] && [ -d "$OS_DIR" ]; then
            info "Restoring OpenSearch data from $(rel "$dir/opensearch-data.tar.gz")"
            run_cmd tar -xzf "$dir/opensearch-data.tar.gz" -C "$OS_DIR" || warn "the OpenSearch data was not restored"
        fi
        opensearch_resume
    fi
    info "Restored $SAVE_NAME"
}


# ========== help ==========

help_save_db() {
    cat <<'EOF'
Saves the embedded database (pg_dump through moqui save-db) to runtime/save/<name>/moqui.dump,
and runtime/opensearch/data to runtime/save/<name>/opensearch-data.tar.gz. An OpenSearch daemon
with a pid file is stopped for the copy and started again. The name defaults to "default".
This saves only the embedded database, not an external one a datasource's database-conf-name names.
  ./moq save-db
  ./moq save-db before-upgrade

Options:
      --debug, --release
                       Use the Debug or Maximal binary (default: Maximal), built first
      --nobuild        Skip that build. A binary that is not there is still built
      --skip-opensearch  Leave out the OpenSearch data
EOF
}

help_reload_save() {
    cat <<'EOF'
Restores a save-db copy: clean-db, then pg_restore of runtime/save/<name>/moqui.dump into a new
embedded database (moqui load-save), then the OpenSearch data when the save has it. The server
must be stopped. The name defaults to "default".
  ./moq reload-save
  ./moq reload-save before-upgrade

Options:
      --debug, --release
                       Use the Debug or Maximal binary (default: Maximal), built first
      --nobuild        Skip that build. A binary that is not there is still built
      --skip-opensearch  Leave runtime/opensearch/data alone
EOF
}

help_clean_db() {
    cat <<'EOF'
Deletes runtime/db/embedded (the embedded Postgres cluster), runtime/txlog, and the OpenSearch
data and logs in runtime/opensearch. An OpenSearch daemon with a pid file is stopped first and
started again after. It keeps runtime/install (the unpacked Postgres) and
runtime/db/instance_id, and refuses while the embedded Postgres is running.
EOF
}

help_clean_log() {
    echo 'Deletes everything in runtime/log.'
}

help_clean() { help_tidy_cargo; }

help_clean_dev() {
    echo 'Reset the stopped instance: database, transaction log, OpenSearch data and logs, runtime/log and tmp. Build output, runtime/types among it, stays.'
}

help_clean_all() {
    echo 'Reset the stopped instance and remove its build output. Preview with --dry-run. Use --yes in scripts.'
}

help_tidy_cargo() {
    cat <<'EOF'
Deletes old cargo artifacts under target/ and keeps the current build. Each unit cargo still
builds keeps its newest output: the binary of each package, test harnesses, and each feature
set, such as moqui-core with and without its default features. A later cargo build of unchanged
code does not recompile.

Cargo names a unit's files with a hash of the compiler, the profile, the features, and the
dependency graph. The hash changes when one of those changes, and cargo leaves the previous
files in place. Repeated builds fill target/debug/build/<package> (target/debug/deps before Cargo
1.100) and target/debug/incremental with those old copies. rustc already drops old incremental
sessions inside one of those directories; this removes the directories for hashes the current
build does not use. Cargo 1.100 and later do not read the earlier layout (.fingerprint, deps), so
it goes once a profile has been built in the new one. A build script's run fingerprint does not
record the profile or the features, so each run that a kept library names stays, along with its
output under build/.

It does not remove a package's binary (target/<profile>/moqui) or target/moqui-app. A cargo build
that is running is left alone. load-save does not run this.
EOF
}
