# shellcheck shell=bash
# moq's the utilities, task matching and the shared help (usage, the option texts). Sourced by moq, which holds the task table; bash 3.2, as moq is.

# ========== utilities ==========

die() {
    printf 'moq: %s\n' "$*" >&2
    exit 1
}

warn() {
    printf 'moq: %s\n' "$*" >&2
}

info() {
    printf '%s\n' "$*"
}

heading() {
    printf '%s%s%s\n' "$BOLD" "$*" "$PLAIN"
}

dry_run() {
    [ "${MOQ_DRY_RUN:-}" = 1 ]
}

# The command as a shell would read it, on stderr.
show_cmd() {
    local out='+' arg
    for arg in "$@"; do
        out="$out $(printf '%q' "$arg")"
    done
    printf '%s\n' "$out" >&2
}

run_cmd() {
    if dry_run; then
        show_cmd "$@"
        return 0
    fi
    "$@"
}

exec_cmd() {
    if dry_run; then
        show_cmd exec "$@"
        exit 0
    fi
    exec "$@"
}

# moqui.exe does not translate an MSYS path in the environment or in `name=/d/...` arguments.
exec_moqui() {
    if moq_on_windows; then
        if dry_run; then
            show_cmd exec "$@"
            exit 0
        fi
        apply_win_env
        local arg
        local -a args
        args=()
        for arg in "$@"; do
            args+=("$(win_arg "$arg")")
        done
        exec "${args[@]}"
    else
        exec_cmd "$@"
    fi
}

run_moqui() {
    if moq_on_windows; then
        if dry_run; then
            show_cmd "$@"
            return 0
        fi
        local arg
        local -a args
        args=()
        for arg in "$@"; do
            args+=("$(win_arg "$arg")")
        done
        (
            apply_win_env
            exec "${args[@]}"
        )
    else
        run_cmd "$@"
    fi
}

export_var() {
    if dry_run; then
        show_cmd export "$1=$2"
    fi
    export "$1=$2"
}

# Paths as the user sees them: relative to the repo root when inside it.
rel() {
    case $1 in
        "$ROOT") printf '.\n' ;;
        "$ROOT"/*) printf '%s\n' "${1#"$ROOT"/}" ;;
        *) printf '%s\n' "$1" ;;
    esac
}

# Delete a path under the runtime directory or the component directory, and refuse anything else.
rm_path() {
    case $1 in
        "$RUNTIME_DIR"/?*) ;;
        *) die "refusing to delete $1, which is not under $(rel "$RUNTIME_DIR")" ;;
    esac
    [ -e "$1" ] || [ -h "$1" ] || return 0
    info "Deleting $(rel "$1")"
    run_cmd rm -rf -- "$1"
}

need_tool() {
    local tool
    for tool in "$@"; do
        command -v "$tool" >/dev/null 2>&1 || die "this task needs '$tool', which is not on the PATH"
    done
}

# aws-lc-sys looks up nasm.exe on PATH. On Windows, download a pinned NASM once
# (scripts/nasm.sh) and prepend it. A dry run prints the curl and does not download.
# It may no longer be needed, now that AWS-LC is built once with prebuilt-nasm on: docs/Build-Test.md (Windows)
# says how to check. MOQ_NASM_CHECKED=1 skips it.
# `moq run` does not call need_cargo, so starting the server does not fetch it.
ensure_nasm() {
    [ -z "${MOQ_NASM_CHECKED:-}" ] || return 0
    MOQ_NASM_CHECKED=1
    moq_on_windows || return 0
    local script=$ROOT/scripts/nasm.sh dir
    [ -f "$script" ] || die "missing $(rel "$script")"
    dir=$("$script" ensure | tr -d '\r') || die "could not get NASM"
    [ -n "$dir" ] || return 0
    export PATH="$dir:$PATH"
}

# cargo, with sccache as its rustc wrapper when MOQ_SCCACHE=1 asks for it (opt-in), so the worktrees of this repo
# share compiled dependencies instead of each building them into its own target/ (docs/Build-Test.md, Compile cache).
# A crate compiled incrementally goes straight to rustc, so incremental builds work as before. A wrapper already
# chosen (RUSTC_WRAPPER or CARGO_BUILD_RUSTC_WRAPPER set, even empty, or rustc-wrapper in a cargo config) is kept.
# Asked for without sccache on the PATH, the build goes on without it, with a warning once per moq run.
need_cargo() {
    need_tool cargo
    ensure_nasm
    [ -z "${MOQ_SCCACHE_CHECKED:-}" ] || return 0
    MOQ_SCCACHE_CHECKED=1
    case ${MOQ_SCCACHE:-} in 1 | yes | on | true) ;; *) return 0 ;; esac
    [ -z "${RUSTC_WRAPPER+set}" ] && [ -z "${CARGO_BUILD_RUSTC_WRAPPER+set}" ] || return 0
    local dir=$ROOT conf
    for conf in "${CARGO_HOME:-$HOME/.cargo}"/config "${CARGO_HOME:-$HOME/.cargo}"/config.toml; do
        cargo_conf_has_wrapper "$conf" && return 0
    done
    while :; do
        for conf in "$dir"/.cargo/config "$dir"/.cargo/config.toml; do
            cargo_conf_has_wrapper "$conf" && return 0
        done
        [ "$dir" != / ] && [ -n "$dir" ] || break
        dir=$(dirname "$dir")
    done
    local sccache
    if sccache=$(command -v sccache 2>/dev/null) && [ -n "$sccache" ]; then
        export_var RUSTC_WRAPPER "$sccache"
        return 0
    fi
    # A moq that this one runs (test-all runs moq test) says it once only.
    [ -z "${MOQ_SCCACHE_WARNED:-}" ] || return 0
    export MOQ_SCCACHE_WARNED=1
    cat >&2 <<'EOF'
moq: MOQ_SCCACHE=1 asks for sccache, which is not on the PATH; building without it. Any one of these installs it:
       cargo binstall sccache
       cargo install sccache --locked
       brew install sccache, dnf install sccache, or apt install sccache
       a release binary from https://github.com/mozilla/sccache/releases
EOF
}

cargo_conf_has_wrapper() {
    [ -f "$1" ] && grep -Eq '^[[:space:]]*(build\.)?rustc-wrapper[[:space:]]*=' "$1"
}

# kill -0 fails for another user's live process, which ps -p sees (busybox ps has no -p, but kill -0 covers our own).
pid_alive() {
    kill -0 "$1" 2>/dev/null || ps -p "$1" >/dev/null 2>&1
}

# A pid from a pid file when that process is alive.
live_pid() {
    local pid
    [ -f "$1" ] || return 1
    pid=$(head -n 1 "$1" 2>/dev/null | tr -cd '0-9')
    [ -n "$pid" ] || return 1
    pid_alive "$pid" || return 1
    printf '%s\n' "$pid"
}

server_pid() {
    live_pid "$CLUSTER_DIR/data/postmaster.pid"
}

require_server_stopped() {
    local pid
    if pid=$(server_pid); then
        die "$1: the embedded Postgres in $(rel "$CLUSTER_DIR") is running (pid $pid); stop the server first"
    fi
}

valid_name() {
    case $1 in
        '' | .* | *[!A-Za-z0-9._-]*) return 1 ;;
    esac
    return 0
}

# Lines of $1 that start with $2, for completion.
filter_prefix() {
    local cur=$1 line
    shift
    for line in "$@"; do
        case $line in "$cur"*) printf '%s\n' "$line" ;; esac
    done
}

# ========== task names and matching ==========

task_names() {
    printf '%s\n' "$TASKS" | cut -d'|' -f1
}

task_field() {
    printf '%s\n' "$TASKS" | awk -F'|' -v name="$1" -v field="$2" '$1 == name { print $field }'
}

# runProduction -> run-production, gPA -> g-p-a, RUN -> run, clean_db -> clean-db.
normalize_name() {
    case $1 in
        *[[:lower:]]*)
            printf '%s\n' "$1" | sed -e 's/\([^-]\)\([[:upper:]]\)/\1-\2/g' -e 's/\([^-]\)\([[:upper:]]\)/\1-\2/g' |
                tr '[:upper:]_' '[:lower:]-'
            ;;
        *) printf '%s\n' "$1" | tr '[:upper:]_' '[:lower:]-' ;;
    esac
}

# Each dash-separated part of $1 is a prefix of the same part of $2, and $1 may have fewer parts.
segment_match() {
    local i
    local -a want have
    IFS=- read -r -a want <<<"$1"
    IFS=- read -r -a have <<<"$2"
    [ "${#want[@]}" -le "${#have[@]}" ] || return 1
    for ((i = 0; i < ${#want[@]}; i++)); do
        case ${have[i]} in "${want[i]}"*) ;; *) return 1 ;; esac
    done
    return 0
}

# Sets RESOLVED_TASK and RESOLVED_ARGS (an alias's arguments). Returns 1 for no match and 2 for several, with the
# candidates in RESOLVED_CANDIDATES.
resolve_task() {
    local want name line
    RESOLVED_TASK=
    RESOLVED_ARGS=
    RESOLVED_CANDIDATES=
    want=$(normalize_name "$1")
    [ -n "$want" ] || return 1
    for name in $(task_names); do
        if [ "$name" = "$want" ]; then
            RESOLVED_TASK=$name
            return 0
        fi
    done
    while IFS= read -r line; do
        if [ "${line%%|*}" = "$want" ]; then
            line=${line#*|}
            RESOLVED_TASK=${line%% *}
            case $line in *' '*) RESOLVED_ARGS=${line#* } ;; esac
            return 0
        fi
    done <<<"$ALIASES"
    for name in $(task_names); do
        if segment_match "$want" "$name"; then
            RESOLVED_CANDIDATES="$RESOLVED_CANDIDATES $name"
        fi
    done
    RESOLVED_CANDIDATES=${RESOLVED_CANDIDATES# }
    case $RESOLVED_CANDIDATES in
        '') ;;
        *' '*) return 2 ;;
        *)
            RESOLVED_TASK=$RESOLVED_CANDIDATES
            return 0
            ;;
    esac
    # Then old names abbreviated the Gradle way (gPA is gitPullAll), when every match leads to the same task.
    local target='' targets='' same=1
    while IFS= read -r line; do
        if segment_match "$want" "${line%%|*}"; then
            RESOLVED_CANDIDATES="$RESOLVED_CANDIDATES ${line%%|*}"
            [ -z "$targets" ] || [ "$targets" = "${line#*|}" ] || same=
            targets=${line#*|}
        fi
    done <<<"$ALIASES"
    RESOLVED_CANDIDATES=${RESOLVED_CANDIDATES# }
    [ -n "$targets" ] || return 1
    [ -n "$same" ] || return 2
    target=$targets
    RESOLVED_TASK=${target%% *}
    case $target in *' '*) RESOLVED_ARGS=${target#* } ;; esac
    return 0
}

# Task names for an unknown input: those containing it, else those containing a part of it (two letters or more).
suggest_tasks() {
    local want name part found=''
    local -a parts
    want=$(normalize_name "$1")
    for name in $(task_names); do
        case $name in *"$want"*) found="$found $name" ;; esac
    done
    if [ -z "$found" ]; then
        IFS=- read -r -a parts <<<"$want"
        for name in $(task_names); do
            for part in "${parts[@]}"; do
                [ ${#part} -ge 2 ] || continue
                case $name in
                    *"$part"*)
                        found="$found $name"
                        break
                        ;;
                esac
            done
        done
    fi
    printf '%s\n' "${found# }"
}

# ========== help ==========

usage() {
    local group last='' name args desc
    cat <<EOF
${BOLD}moq${PLAIN}: MR Moqui tasks (the Java Moqui Gradle tasks, with real arguments)

Usage: ./moq <task> [args...]      ./moq help <task>      ./moq <task> --help

A task name matches by a prefix of each dash-separated part, ignoring case,
so r-p and runProduction both mean run-production, and c-g is component-get.
EOF
    while IFS='|' read -r name group args desc; do
        [ "$group" = Harness ] && continue
        if [ "$group" != "$last" ]; then
            printf '\n%s%s%s\n' "$BOLD" "$group" "$PLAIN"
            last=$group
        fi
        printf '  %-20s %-30s %s\n' "$name" "$args" "$desc"
    done <<<"$TASKS"
    printf '  %-20s %s\n' "agent harnesses" "$(cut -d'|' -f1 <<<"$HARNESSES" | paste -sd, - | sed 's/,/, /g') (./moq help <name>)"
    cat <<EOF

Tab completion: ./moq completion --install, or add this to ~/.bashrc or ~/.zshrc:
  eval "\$($(printf '%q' "$MOQ") completion bash)"     (zsh: completion zsh)
EOF
}

help_task() {
    local name=$1 fn
    fn="help_${name//-/_}"
    printf '%s./moq %s %s%s\n    %s\n\n' "$BOLD" "$name" "$(task_field "$name" 3)" "$PLAIN" "$(task_field "$name" 4)"
    if declare -f "$fn" >/dev/null; then
        "$fn"
    fi
}

PACKAGE_OPTIONS_HELP='      --lite           The Lite package, target/lite/moqui (the web server on embedded Postgres,
                       PDF and SMTP)
      --debug          The Debug package, target/debug/moqui (unoptimized, every feature, dev tools)
      --release        The Maximal package, target/release/moqui (every feature, Postgres embedded, the
                       dev tools); the default, the same as none of these
      --enterprise     The Enterprise package, target/enterprise/moqui (every feature, databases external).
                       docs/Build-Test.md lists the features of each'

EMBED_OPTIONS_HELP='      --embed LEVEL    The runtime built into the binary (docs/Build-Test.md, Embedded runtime and programs):
                       framework (the default: framework, tools, a webroot stub and the confs),
                       production (every base and add-on component less its tests and plans, the file
                       set of ./moq archive), or all (with tests, plans and the dev and test sets)
      --embed-opensearch  Build in OpenSearch (Linux), unpacked into install/ on its first start
      --embed-chromium    Build in Chromium for PDF, unpacked into install/ when no other comes first'

RUN_OPTIONS_HELP="$PACKAGE_OPTIONS_HELP
$EMBED_OPTIONS_HELP"'
  -c, --conf FILE      Runtime conf merged over MoquiDefaultConf.xml (sets MOQUI_CONF); relative to
                       the runtime directory, like conf/MoquiProductionConf.xml, or a path
  -p, --port N         HTTP port (also --http-port N, or off)
      --https-port N   HTTPS port to listen on (its https-listen-port), or off
  -D name=value        Set a conf attribute by its environment name (moqui_conf__ssh_server__port;
                       moqui conf env lists them) or a conf property, as an environment variable;
                       repeatable. Use the underscore name, not a dotted one
      --nobuild        Skip the build (./moq build) that runs first. A binary that is not there is still built
      --               End of moq options'




LOAD_HELP='Words without "=" are data types, joined into types=a,b. name=value words go to moqui load as
they are: components=a,b, timeout=<seconds>, raw=true, disableEntityEca=true,
disableAuditLog=true, disableFkCreate=true, disableDataFeed=true.

Options (as for run): --lite, --debug, --release, --enterprise, -c FILE, -D name=value, --nobuild, and
  -l, --location FILE  Load only this data file (location=<absolute path>)'




























COMPONENT_TYPE_HELP='  -t, --type TYPE      git, current (branch archive), release, or binary. Default: git when the
                       root has .git, else current. release and binary fall back to current when
                       the component has no version'
COMPONENT_REPO_HELP='  -r, --repo REPO      Repository from myaddons.xml or addons.xml, overriding the component'"'"'s
                       repository attribute. Default: github when this repo was cloned by https,
                       github-ssh when by git@ (ssh)'







help_help() {
    echo 'With a task name (partial names work), shows that task'"'"'s help. Otherwise the task list.'
}
