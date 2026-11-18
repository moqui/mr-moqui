# shellcheck shell=bash
# moq's the agent harness setups (claude-code, claude-desktop). Sourced by moq, which holds the task table; bash 3.2, as moq is.

# ========== agent harness setups ==========

# A setup makes this checkout's agent instructions, AGENTS.md and .agents/skills (the repositories keep no harness's
# own files), visible to one agent harness: name|description. Each is a task (group Harness), listed on one line of
# the task list, with a task_<name> function and a help_<name> text below. What a setup makes goes where git ignores
# it (.claude/ is in .gitignore, target/ is the build's).
HARNESSES='claude-code|Link the skills of .agents/skills into .claude/skills for Claude Code (CLI, and the desktop Code tab)
claude-desktop|Zip each skill of .agents/skills into target/claude-desktop to upload in the Claude Desktop app'

while IFS='|' read -r harness_name harness_desc; do
    TASKS+=$'\n'"$harness_name|Harness||$harness_desc"
done <<<"$HARNESSES"
unset harness_name harness_desc

# The skill directories of .agents/skills (those with a SKILL.md), one name a line.
agent_skills() {
    local dir
    for dir in "$ROOT"/.agents/skills/*/; do
        [ -f "$dir/SKILL.md" ] && basename "$dir"
    done
}

task_claude_code() {
    local name link target found=''
    [ $# -eq 0 ] || die "claude-code takes no arguments"
    run_cmd mkdir -p "$ROOT/.claude/skills"
    while IFS= read -r name; do
        [ -n "$name" ] || continue
        link=$ROOT/.claude/skills/$name
        target=../../.agents/skills/$name
        if [ -L "$link" ] && [ "$(readlink "$link")" = "$target" ]; then
            info "  .claude/skills/$name (already linked)"
        elif [ -e "$link" ] || [ -L "$link" ]; then
            warn ".claude/skills/$name is there and is not a link to $target; left as it is"
        else
            run_cmd ln -s "$target" "$link" && info "  .claude/skills/$name -> $target"
        fi
    done < <(agent_skills)
    # a link to a skill that is gone
    for link in "$ROOT"/.claude/skills/*; do
        [ -L "$link" ] && [ ! -e "$link" ] && case $(readlink "$link") in
            ../../.agents/skills/*) run_cmd rm "$link" && info "  removed .claude/skills/${link##*/} (its skill is gone)" ;;
        esac
    done
    # Claude Code reads AGENTS.md only when no CLAUDE.md is in the checkout or a directory above it
    target=$ROOT
    while :; do
        for name in CLAUDE.md .claude/CLAUDE.md CLAUDE.local.md; do
            [ -e "$target/$name" ] && found="$found $target/$name"
        done
        [ "$target" = / ] && break
        target=$(dirname "$target")
    done
    [ -z "$found" ] || warn "Claude Code reads AGENTS.md only without a CLAUDE.md; found:$found"
    if command -v claude >/dev/null 2>&1; then
        target=$(claude --version 2>/dev/null | grep -oE '[0-9]+\.[0-9]+\.[0-9]+' | head -1)
        if [ -n "$target" ] && [ "$(printf '%s\n' 2.1.277 "$target" | sort -V 2>/dev/null | head -1)" != 2.1.277 ]; then
            warn "claude $target reads CLAUDE.md, not AGENTS.md: update it (2.1.277 or later)"
        fi
    fi
}

task_claude_desktop() {
    local name out=$ROOT/target/claude-desktop given
    [ $# -eq 0 ] || die "claude-desktop takes no arguments"
    need_tool zip
    run_cmd mkdir -p "$out"
    while IFS= read -r name; do
        [ -n "$name" ] || continue
        # the app refuses a skill whose name is not its folder's
        given=$(sed -n 's/^name:[[:space:]]*//p' "$ROOT/.agents/skills/$name/SKILL.md" | head -1)
        [ "$given" = "$name" ] || warn "$name/SKILL.md names itself '$given'; the app wants its folder's name"
        run_cmd rm -f "$out/$name.zip"
        (cd "$ROOT/.agents/skills" && run_cmd zip -qr "$out/$name.zip" "$name") && info "  $(rel "$out/$name.zip")"
    done < <(agent_skills)
    info "Upload each in the Claude Desktop app (Customize, Skills) and turn it on. Its Code tab reads this checkout's"
    info "files as the CLI does: ./moq claude-code."
}

help_claude_code() {
    cat <<'EOF'
Links each skill of .agents/skills into .claude/skills (.claude/skills/<name> -> ../../.agents/skills/<name>), where
Claude Code finds project skills; it reads AGENTS.md itself. Run it again after a skill is added or removed: it links
the new ones and removes links to skills that are gone, and leaves anything else in .claude/skills alone. It warns
when a CLAUDE.md in the checkout or above it would keep Claude Code from reading AGENTS.md, and when the installed
claude is older than 2.1.277. Nothing it makes is committed (.claude/ is in .gitignore). A linked worktree without
its own .claude/skills uses the main checkout's. On Windows without symlink support the links may be copies: run it
again after a skill changes.
EOF
}

help_claude_desktop() {
    cat <<'EOF'
Writes target/claude-desktop/<name>.zip for each skill of .agents/skills, the skill's folder at the root of the zip,
to upload in the Claude Desktop app (Customize, Skills), which reads no folder a script could link into. Run it again
after a skill changes, and upload that zip again. It warns when a skill's frontmatter name is not its folder's name.
The app's Code tab uses this checkout's files as the CLI does: ./moq claude-code.
EOF
}
