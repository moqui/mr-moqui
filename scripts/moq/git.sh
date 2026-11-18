# shellcheck shell=bash
# moq's git, git-status and git-pull over every repository. Sourced by moq, which holds the task table; bash 3.2, as moq is.

# ========== git ==========

# The root repo, runtime/ when it is its own repo, then component repos (components/base, component and components/dev),
# one per line.
git_repos() {
    local dir
    [ -e "$ROOT/.git" ] && printf '%s\n' "$ROOT"
    if [ -e "$RUNTIME_DIR/.git" ] && [ "$RUNTIME_DIR" != "$ROOT" ]; then
        printf '%s\n' "$RUNTIME_DIR"
    fi
    for dir in "$RUNTIME_DIR"/$BASE_SET/* "$RUNTIME_DIR"/$ADDON_SET/* "$RUNTIME_DIR"/$DEV_SET/* "$RUNTIME_DIR"/$TEST_SET/*; do
        if [ -d "$dir" ] && [ -e "$dir/.git" ]; then
            printf '%s\n' "$dir"
        fi
    done
}

# git_repos into the REPOS array.
load_repos() {
    local repo
    REPOS=()
    while IFS= read -r repo; do
        REPOS+=("$repo")
    done < <(git_repos)
    [ ${#REPOS[@]} -gt 0 ] || die "no git repos found"
}

git_branch_label() {
    git -C "$1" symbolic-ref --short -q HEAD 2>/dev/null ||
        printf 'detached at %s\n' "$(git -C "$1" rev-parse --short HEAD 2>/dev/null)"
}

task_git() {
    [ $# -gt 0 ] || die "git needs arguments, like: ./moq git status"
    need_tool git
    local repo failed='' first=1
    load_repos
    for repo in "${REPOS[@]}"; do
        [ -n "$first" ] || echo
        first=
        heading "== $(rel "$repo") ($(git_branch_label "$repo"))"
        if ! run_cmd git --no-pager -C "$repo" "$@"; then
            failed="$failed $(rel "$repo")"
        fi
    done
    if [ -n "$failed" ]; then
        echo
        die "git failed in:$failed"
    fi
}

task_git_status() {
    local short='' repo branch upstream counts behind ahead changes unpushed remote_ref base lines
    while [ $# -gt 0 ]; do
        case $1 in
            -s | --short) short=1 ;;
            *) die "unknown argument $1" ;;
        esac
        shift
    done
    need_tool git
    load_repos
    for repo in "${REPOS[@]}"; do
        branch=$(git_branch_label "$repo")
        upstream=$(git -C "$repo" rev-parse --abbrev-ref --symbolic-full-name '@{u}' 2>/dev/null)
        behind=0
        ahead=0
        unpushed=
        if [ -n "$upstream" ]; then
            counts=$(git -C "$repo" rev-list --left-right --count '@{u}...HEAD' 2>/dev/null)
            behind=${counts%%[[:space:]]*}
            ahead=${counts##*[[:space:]]}
            if [ "${ahead:-0}" -gt 0 ]; then
                unpushed=$(git -C "$repo" log --oneline --no-decorate -n 20 '@{u}..HEAD')
            fi
        fi
        changes=$(git -C "$repo" status --short 2>/dev/null)
        if [ -n "$short" ] && [ -z "$changes" ] && [ "${ahead:-0}" -eq 0 ] && [ "${behind:-0}" -eq 0 ]; then
            continue
        fi
        lines="$branch"
        if [ -n "$upstream" ]; then
            lines="$lines -> $upstream"
            if [ "${ahead:-0}" -gt 0 ] || [ "${behind:-0}" -gt 0 ]; then
                lines="$lines [ahead $ahead, behind $behind]"
            fi
        else
            lines="$lines (no tracking branch)"
        fi
        heading "== $(rel "$repo")  $lines"
        if git -C "$repo" remote 2>/dev/null | grep -qx upstream; then
            base=${upstream#*/}
            for remote_ref in "$base" master main; do
                [ -n "$remote_ref" ] || continue
                if git -C "$repo" rev-parse -q --verify "refs/remotes/upstream/$remote_ref" >/dev/null; then
                    counts=$(git -C "$repo" rev-list --left-right --count "upstream/$remote_ref...HEAD")
                    info "   upstream/$remote_ref: ahead ${counts##*[[:space:]]}, behind ${counts%%[[:space:]]*}"
                    break
                fi
            done
        fi
        if [ -n "$unpushed" ]; then
            info "   unpushed:"
            printf '%s\n' "$unpushed" | sed 's/^/     /'
            [ "$ahead" -le 20 ] || info "     ... and $((ahead - 20)) more"
        fi
        if [ -n "$changes" ]; then
            printf '%s\n' "$changes" | sed 's/^/   /'
        else
            info "   clean"
        fi
    done
}

task_git_pull() {
    need_tool git
    local repo before after failed='' first=1
    load_repos
    for repo in "${REPOS[@]}"; do
        [ -n "$first" ] || echo
        first=
        heading "== $(rel "$repo") ($(git_branch_label "$repo"))"
        before=$(git -C "$repo" rev-parse -q HEAD 2>/dev/null)
        if ! run_cmd git --no-pager -C "$repo" pull "$@"; then
            failed="$failed $(rel "$repo")"
            continue
        fi
        dry_run && continue
        after=$(git -C "$repo" rev-parse -q HEAD 2>/dev/null)
        if [ -n "$before" ] && [ "$before" != "$after" ]; then
            info "New commits:"
            git --no-pager -C "$repo" log --no-decorate --format='  %h %<(16,trunc)%an %s' "$before..$after"
        fi
    done
    if [ -n "$failed" ]; then
        echo
        die "git pull failed in:$failed"
    fi
}


# ========== help ==========

help_git() {
    cat <<'EOF'
Runs git with these arguments in the root repo, runtime/ when it is its own repo, and each
repo in runtime/components/base and runtime/components/addon, under a header for each. It keeps going
when one fails, and exits non-zero if any failed. The Gradle git tasks become:
  ./moq git checkout mr-moqui             gitCheckoutAll -Pbranch=mr-moqui
  ./moq git checkout -b new-branch        gitCheckoutAll -Pbranch=new-branch -Pcreate=true
  ./moq git pull upstream master          gitUpstreamAll
  ./moq git tag v4.1.0 && ./moq git push --tags     gitTagAll -Ptag=v4.1.0 -Ppush=true
  ./moq git tag -d v4.1.0                 gitTagAll -Ptag=v4.1.0 -Premove=true
  ./moq git log --oneline v4.0.0..HEAD    gitDiffTagsAll -Ptaga=v4.0.0
  ./moq git merge feature-x               gitMergeAll -Pbranch=feature-x
git runs without a pager. For git's own help use git help <command>.
EOF
}

help_git_status() {
    cat <<'EOF'
For each repo: the branch and its tracking branch, commits ahead and behind, how far it is
from upstream/<branch> when there is an upstream remote, the unpushed commits, and the
short status. It does not fetch; run ./moq git fetch first for current counts.

Options:
  -s, --short          Only show repos with changes or commits to push or pull
EOF
}

help_git_pull() {
    cat <<'EOF'
Runs git pull (with any arguments given, like --rebase) in each repo, then lists the commits
the pull brought in. That list runs without a pager.
EOF
}
