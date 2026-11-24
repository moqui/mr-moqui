# shellcheck shell=bash
# moq's the bash and zsh completion and its protocol. Sourced by moq, which holds the task table; bash 3.2, as moq is.

# ========== completion ==========

completion_bash() {
    cat <<'EOF'
# bash completion for moq (MR Moqui). Load with: eval "$(/path/to/moq completion bash)"
_moq_complete() {
    local cmd=${COMP_WORDS[0]} cur=${COMP_WORDS[COMP_CWORD]} out line
    case $cmd in
        "~/"*) cmd=$HOME/${cmd#"~/"} ;;
        */*) ;;
        *) cmd=$(type -P "$cmd") || return 0 ;;
    esac
    [ -f "$cmd" ] && head -n 3 "$cmd" 2>/dev/null | grep -q moq-completion-protocol-1 || return 0
    out=$("$cmd" __complete "$COMP_CWORD" "${COMP_WORDS[@]:1}" 2>/dev/null) || return 0
    COMPREPLY=()
    case $out in
        @files*)
            while IFS= read -r line; do COMPREPLY+=("$line"); done < <(compgen -f -- "$cur")
            type compopt >/dev/null 2>&1 && compopt -o filenames
            ;;
        @dirs)
            while IFS= read -r line; do COMPREPLY+=("$line"); done < <(compgen -d -- "$cur")
            type compopt >/dev/null 2>&1 && compopt -o filenames
            ;;
        *)
            while IFS= read -r line; do [ -n "$line" ] && COMPREPLY+=("$line"); done <<<"$out"
            if [ ${#COMPREPLY[@]} -eq 1 ]; then
                case ${COMPREPLY[0]} in *= | */) type compopt >/dev/null 2>&1 && compopt -o nospace ;; esac
            fi
            ;;
    esac
    return 0
}
complete -F _moq_complete moq
EOF
}

completion_zsh() {
    cat <<'EOF'
# zsh completion for moq (MR Moqui). Load with: eval "$(/path/to/moq completion zsh)"
_moq_complete() {
    local cmd=${words[1]} out
    local -a cands
    cmd=${~cmd}
    [[ $cmd == */* ]] || cmd=${commands[$cmd]}
    [[ -f $cmd ]] && head -n 3 -- "$cmd" 2>/dev/null | grep -q moq-completion-protocol-1 || return 1
    out=$("$cmd" __complete $((CURRENT - 1)) "${(@)words[2,-1]}" 2>/dev/null) || return 1
    cands=("${(@f)out}")
    case ${cands[1]} in
        @files*) _files ;;
        @dirs) _files -/ ;;
        *)
            [[ -n ${cands[1]} ]] || return 1
            if [[ ${#cands} -eq 1 && ${cands[1]} == *= ]]; then
                compadd -U -S '' -- "${cands[@]}"
            else
                compadd -U -- "${cands[@]}"
            fi
            ;;
    esac
}
if ! (( $+functions[compdef] )); then
    autoload -Uz compinit && compinit
fi
compdef _moq_complete moq
EOF
}

task_completion() {
    local shell='' install='' rc line
    while [ $# -gt 0 ]; do
        case $1 in
            bash | zsh) shell=$1 ;;
            --install) install=1 ;;
            *) die "completion takes bash or zsh, and --install" ;;
        esac
        shift
    done
    if [ -z "$shell" ]; then
        case ${SHELL:-} in
            */zsh) shell=zsh ;;
            *) shell=bash ;;
        esac
    fi
    if [ -z "$install" ]; then
        "completion_$shell"
        return 0
    fi
    case $shell in
        zsh) rc=$HOME/.zshrc ;;
        bash) rc=$HOME/.bashrc ;;
    esac
    line="eval \"\$($(printf '%q' "$MOQ") completion $shell)\""
    if [ -f "$rc" ] && grep -qF -- "$line" "$rc"; then
        info "$rc already loads moq completion"
        return 0
    fi
    if dry_run; then
        show_cmd append "$rc" "$line"
        return 0
    fi
    printf '\n# moq (MR Moqui) tab completion\n%s\n' "$line" >>"$rc" || die "could not write $rc"
    info "Added to $rc: $line"
    info "Open a new shell, or run that line, to use it."
    if [ "$shell" = bash ] && [ "$(uname -s)" = Darwin ]; then
        info "On macOS a login shell reads ~/.bash_profile; source ~/.bashrc from there if it does not already."
    fi
}

conf_names() {
    local f
    for f in "$RUNTIME_DIR"/conf/*.xml "$RUNTIME_DIR"/conf/*/*.xml; do
        [ -f "$f" ] && printf '%s\n' "${f#"$RUNTIME_DIR"/}"
    done
}

data_types() {
    local framework=${MOQUI_FRAMEWORK:-$RUNTIME_DIR/$BASE_SET/framework}
    {
        echo all
        find "$framework/data" "$RUNTIME_DIR"/$BASE_SET/*/data "$RUNTIME_DIR"/$ADDON_SET/*/data \
            -name '*.xml' -type f -exec grep -hoE '<entity-facade-xml[^>]*type="[^"]*"' {} + 2>/dev/null |
            sed -E 's/.*type="([^"]*)".*/\1/' | tr ',' '\n' | sed 's/^ *//;s/ *$//'
    } | grep -v '^$' | sort -u
}

# The conf's environment names for -D, each with its =: moqui conf env --names of the first package built (none when
# none is built); the binary merges this runtime's conf, its components' included.
property_names() {
    local package bin
    for package in release debug lite enterprise; do
        bin=$(binary_path "$package")
        if [ -x "$bin" ]; then
            (cd "$ROOT" && "$bin" conf env --names 2>/dev/null) | sed 's/$/=/'
            return
        fi
    done
}

save_names() {
    local d
    for d in "$SAVE_DIR"/*; do
        [ -f "$d/moqui.dump" ] && basename "$d"
    done
}

# Names (from component.xml) of the components that have test/*.spec.js files, in test or a subdirectory of it.
# Components with server specs, in runtime/components/base, runtime/components/addon and runtime/components/dev; not a template,
# whose name is a placeholder (runtime/components/dev/template).
test_components() {
    spec_components test "${COMPONENT_SETS[@]}"
}

# spec_component_dirs <tree> <parent...>: the directories under runtime/<parent> of the components with a *.spec.js
# in <tree> (test, test/client, test/acceptance or test/acceptance/client), outside its reserved directories; not the
# template.
spec_component_dirs() {
    local tree=$1 parent d
    local -a prune
    shift
    case $tree in
        */client) prune=(-path "@/process") ;;
        test) prune=(-path "@/client" -o -path "@/agent" -o -path "@/acceptance" -o -path "@/load" -o -path "@/proofs") ;;
        *) prune=(-path "@/client" -o -path "@/agent") ;;
    esac
    for parent in "$@"; do
        for d in "$RUNTIME_DIR/$parent"/*/; do
            [ -d "${d}$tree" ] && [ -f "${d}component.xml" ] || continue
            [ -n "$(find "${d}$tree" \( "${prune[@]//@/${d}$tree}" \) -prune -o -name '*.spec.js' -print -quit)" ] ||
                continue
            printf '%s\n' "${d%/}"
        done
    done
}

# spec_components <tree> <parent...>: the names of the components of spec_component_dirs.
spec_components() {
    local d name
    while IFS= read -r d; do
        name=$(component_name_of "$d")
        case $name in *@*) ;; *) printf '%s\n' "$name" ;; esac
    done < <(spec_component_dirs "$@")
}

# served_components <tree> <parent...>: the components of spec_components whose <tree>/suite.js declares a server.
served_components() {
    local tree=$1 d name
    while IFS= read -r d; do
        [ -f "$d/$tree/suite.js" ] && grep -Eq 'server *: *[{]' "$d/$tree/suite.js" || continue
        name=$(component_name_of "$d")
        case $name in *@*) ;; *) printf '%s\n' "$name" ;; esac
    done < <(spec_component_dirs "$@")
}

# The components whose <tree>/suite.js declares no server: their specs are offline (server: false) pages.
offline_components() {
    local tree=$1 d name
    while IFS= read -r d; do
        [ -f "$d/$tree/suite.js" ] && ! grep -Eq 'server *: *[{]' "$d/$tree/suite.js" || continue
        name=$(component_name_of "$d")
        case $name in *@*) ;; *) printf '%s\n' "$name" ;; esac
    done < <(spec_component_dirs "$@")
}

component_names_local() {
    local d
    while IFS= read -r d; do
        basename "$d"
    done < <(component_dirs)
}

git_refs() {
    git -C "$ROOT" for-each-ref --format='%(refname:short)' refs/heads refs/tags refs/remotes 2>/dev/null
}

# Components with client specs, in runtime/components/base, runtime/components/addon and runtime/components/dev.
client_test_components() {
    spec_components test/client "${COMPONENT_SETS[@]}"
}

# Candidate lists below come from $(...) and are split on purpose: names hold no spaces or glob characters.
# shellcheck disable=SC2046
complete_run_like() {
    local cur=$1 prev=$2 positional=$3
    case $prev in
        --embed)
            filter_prefix "$cur" framework production all
            return
            ;;
        -c | --conf)
            local confs
            confs=$(filter_prefix "$cur" $(conf_names))
            if [ -n "$confs" ]; then printf '%s\n' "$confs"; else echo @files; fi
            return
            ;;
        -l | --location) echo @files; return ;;
        -D) filter_prefix "$cur" $(property_names); return ;;
        -p | --port | --http-port | -n | --name) return ;;
        --https-port) filter_prefix "$cur" off; return ;;
    esac
    case $cur in
        -*) filter_prefix "$cur" --lite --debug --release --enterprise --embed --embed-opensearch --embed-chromium -c --conf \
            -p --port --http-port --https-port -D --nobuild --; return ;;
    esac
    printf '%s\n' "$positional"
}

# shellcheck disable=SC2046
complete_task() {
    local task=$1 cur=$2 prev=$3 nargs=$4 first_arg=$5
    case $task in
        trust-dev-cert) filter_prefix "$cur" --system --untrust ;;
        build)
            case $prev in
                --embed) filter_prefix "$cur" framework production all ;;
                *) filter_prefix "$cur" --lite --debug --release --enterprise --embed --embed-opensearch --embed-chromium ;;
            esac
            ;;
        run | run-production)
            local out
            out=$(complete_run_like "$cur" "$prev" @binary)
            if [ "$out" = @binary ]; then
                filter_prefix "$cur" lint types convert load save-db load-save
            else
                printf '%s\n' "$out"
            fi
            ;;
        test)
            local out
            out=$(complete_run_like "$cur" "$prev" @binary)
            if [ "$out" = @binary ]; then
                filter_prefix "$cur" $(test_components) --spec --feature --acceptance --db --junit --keep --serve \
                    --ready-file --log-level
            else
                printf '%s\n' "$out"
            fi
            ;;
        test-client)
            case $prev in
                --server | --ssh | --spec | --feature | --param | --wait | --timeout | --log-level) return ;;
                --junit | --out | --chromium) echo @files; return ;;
            esac
            local out
            out=$(complete_run_like "$cur" "$prev" @binary)
            case $out in
                @binary) filter_prefix "$cur" $(client_test_components) ;;
                ?*) printf '%s\n' "$out" ;;
            esac
            case $cur in
                -*)
                    filter_prefix "$cur" --server --ssh --serve --spec --feature --acceptance --proofs --profile --param --junit \
                        --out --wait --timeout --chromium --headed --video --offline --insecure --allow-http --fail-on-skip \
                        --log-level
                    ;;
            esac
            ;;
        test-regression)
            case $prev in
                --spec | --feature | --start-timeout | --log-level) return ;;
                --out | --java-repo) echo @files; return ;;
            esac
            local out
            out=$(complete_run_like "$cur" "$prev" @binary)
            case $out in
                @binary) filter_prefix "$cur" rust:. rust:master java:. java:master $(test_components) ;;
                ?*) printf '%s\n' "$out" ;;
            esac
            case $cur in
                -*) filter_prefix "$cur" --spec --feature --java-repo --out --keep --start-timeout --log-level ;;
            esac
            ;;
        test-load)
            case $prev in
                --server | --ssh | --script | --param | --users | --ramp-up | --duration | --iterations | --scenario | --label | \
                    --log-level) return ;;
                --out | --compare) echo @files; return ;;
            esac
            local out
            out=$(complete_run_like "$cur" "$prev" @binary)
            case $out in
                @binary) filter_prefix "$cur" $(test_components) ;;
                ?*) printf '%s\n' "$out" ;;
            esac
            case $cur in
                -*)
                    filter_prefix "$cur" --script --server --ssh --param --users --ramp-up --duration --iterations --scenario \
                        --label --out --compare --insecure --allow-http --log-level
                    ;;
            esac
            ;;
        load | load-production | load-save)
            local out
            if [ "$task" = load-save ]; then
                case $prev in
                    -n | --name) filter_prefix "$cur" $(save_names); return ;;
                esac
            fi
            out=$(complete_run_like "$cur" "$prev" @types)
            case $out in
                @types)
                    filter_prefix "$cur" $(data_types) components= timeout= raw= disableEntityEca= disableAuditLog= \
                        disableFkCreate= disableDataFeed=
                    ;;
                *)
                    case $cur in
                        -*)
                            filter_prefix "$cur" -l --location
                            [ "$task" != load-save ] || filter_prefix "$cur" -n --name
                            ;;
                    esac
                    printf '%s\n' "$out"
                    ;;
            esac
            ;;
        save-db | reload-save)
            case $cur in
                -*) filter_prefix "$cur" --lite --debug --release --enterprise --nobuild --skip-opensearch ;;
                *) filter_prefix "$cur" $(save_names) ;;
            esac
            ;;
        opensearch-download) filter_prefix "$cur" -f --force --cache-only "$OPENSEARCH_VERSION" ;;
        chromium-download) filter_prefix "$cur" --latest --cache-only ;;
        desktop) filter_prefix "$cur" dev build --release ;;
        android) filter_prefix "$cur" emulator init dev build --release --window ;;
        test-all | test-apps | test-framework) filter_prefix "$cur" --lite --debug --release --enterprise --nobuild ;;
        test-clients) filter_prefix "$cur" client tauri mobile desktop --nobuild ;;
        accept) filter_prefix "$cur" $(spec_components test/acceptance "${COMPONENT_SETS[@]}") --spec --junit \
            --lite --debug --release --enterprise --nobuild ;;
        accept-all) filter_prefix "$cur" --junit --lite --debug --release --enterprise --nobuild ;;
        autodoc)
            case $prev in
                --out | --templates | --project | --java) echo @dirs; return ;;
                --kinds | --kind) filter_prefix "$cur" entity service screen eca rest; return ;;
                --components | --component) filter_prefix "$cur" $(component_names_local); return ;;
                --packages | --package) return ;;
            esac
            filter_prefix "$cur" --out --kinds --components --packages --include-source --templates --clean --project \
                --java --lite --debug --release --enterprise --nobuild
            ;;
        git)
            if [ "$nargs" -eq 1 ]; then
                filter_prefix "$cur" status pull fetch checkout switch branch tag log diff merge push stash reset rebase \
                    remote show
            else
                case $first_arg in
                    checkout | switch | merge | rebase | log | diff | branch | tag | push | reset | show)
                        filter_prefix "$cur" $(git_refs)
                        ;;
                    *) echo @files ;;
                esac
            fi
            ;;
        git-status) filter_prefix "$cur" -s --short ;;
        git-pull) filter_prefix "$cur" --rebase --ff-only --no-rebase ;;
        component-get | component-depends | component-create)
            case $prev in
                -t | --type) filter_prefix "$cur" git current release binary; return ;;
                -r | --repo) filter_prefix "$cur" $(addons_names repository); return ;;
                --from) filter_prefix "$cur" $(component_template_names); return ;;
                --git-remote | --myaddons) return ;;
            esac
            case $cur in
                -*)
                    if [ "$task" = component-create ]; then
                        filter_prefix "$cur" --dev --test --from --rest --screens --git-remote --no-git --myaddons
                    elif [ "$task" = component-get ]; then
                        filter_prefix "$cur" -t --type -r --repo --dev --test
                    else
                        filter_prefix "$cur" -t --type -r --repo
                    fi
                    ;;
                *)
                    if [ "$task" = component-get ]; then
                        filter_prefix "$cur" $(addons_names component) $(addons_names component-set)
                    fi
                    ;;
            esac
            ;;
        component-zip) filter_prefix "$cur" $(component_names_local) --with-tests ;;
        archive)
            case $prev in
                -c | --conf)
                    filter_prefix "$cur" $(conf_names)
                    return
                    ;;
                --embed)
                    filter_prefix "$cur" framework production all
                    return
                    ;;
            esac
            case $cur in
                -*)
                    filter_prefix "$cur" --lite --debug --release --enterprise --nobuild -c --conf --tgz --with-save --with-tests --with-dev \
                        --no-data -f --force --binary --embed --embed-opensearch --embed-chromium
                    ;;
                *) echo @files ;;
            esac
            ;;
        completion) filter_prefix "$cur" bash zsh --install ;;
        help) filter_prefix "$cur" $(task_names) ;;
    esac
}

# __complete <cword> <words after moq...>: candidates for the word at cword (1 is the task name), one per line,
# already filtered, or @files / @dirs for the shell's own file completion.
complete_main() {
    local cword=$1 cur prev name found=''
    shift
    local -a words
    words=("$@")
    cur=${words[cword - 1]}
    if [ "$cword" -le 1 ]; then
        for name in $(task_names); do
            case $name in "$cur"*) found="$found $name" ;; esac
        done
        if [ -z "$found" ] && [ -n "$cur" ]; then
            local want
            want=$(normalize_name "$cur")
            for name in $(task_names); do
                if segment_match "$want" "$name"; then found="$found $name"; fi
            done
        fi
        for name in $found; do printf '%s\n' "$name"; done
        return 0
    fi
    resolve_task "${words[0]}" || return 0
    prev=${words[cword - 2]}
    # An alias that carries arguments (git-checkout-all -> git checkout) counts them as typed.
    local first_arg=${words[1]} nargs=$((cword - 1))
    if [ -n "$RESOLVED_ARGS" ]; then
        # shellcheck disable=SC2086
        set -- $RESOLVED_ARGS
        nargs=$((nargs + $#))
        first_arg=$1
        [ "$cword" -gt 2 ] || prev=$1
    fi
    complete_task "$RESOLVED_TASK" "$cur" "$prev" "$nargs" "$first_arg"
}


# ========== help ==========

help_completion() {
    cat <<EOF
Prints the completion script for bash or zsh (default: from \$SHELL). It completes task names
(also partial ones like r-p), options, conf files, data types, save names, component names, git
refs, and file arguments. Load it from your shell's startup file:
  eval "\$($(printf '%q' "$MOQ") completion bash)"
  eval "\$($(printf '%q' "$MOQ") completion zsh)"

Options:
      --install        Add that line to ~/.bashrc or ~/.zshrc if it is not there yet
EOF
}
