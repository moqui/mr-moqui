# MR Moqui terminal shell integration for bash (plans/Terminal-Fancy.md §11.2), read through ENV by a bash started in
# POSIX mode: it leaves POSIX mode, reads the user's own startup files as the shell would have, then writes OSC 133
# around each prompt and command and OSC 7 with the directory, so the terminal knows where prompts are, when a command
# runs and how it ended. A DEBUG trap of the user's own is replaced.
builtin set +o posix
builtin unset ENV
if [ -n "${MOQUI_SHELL_LOGIN-}" ]; then
    builtin unset MOQUI_SHELL_LOGIN
    [ -r /etc/profile ] && builtin source /etc/profile
    for __moqui_file in ~/.bash_profile ~/.bash_login ~/.profile; do
        if [ -r "$__moqui_file" ]; then builtin source "$__moqui_file"; break; fi
    done
    builtin unset __moqui_file
elif [ -r ~/.bashrc ]; then
    builtin source ~/.bashrc
fi

__moqui_ran=
__moqui_at_prompt=
__moqui_prompt() {
    local status=$?
    [ -n "$__moqui_ran" ] && builtin printf '\e]133;D;%s\a' "$status"
    __moqui_ran=
    builtin printf '\e]7;file://%s%s\a\e]133;A\a' "${HOSTNAME-}" "${PWD// /%20}"
    return $status
}
__moqui_preexec() {
    [ -n "${COMP_LINE-}" ] && return
    [ -z "$__moqui_at_prompt" ] && return
    __moqui_at_prompt=
    __moqui_ran=1
    builtin printf '\e]133;C\a'
}
if [[ "$(builtin declare -p PROMPT_COMMAND 2>/dev/null)" == "declare -a"* ]]; then
    PROMPT_COMMAND=(__moqui_prompt "${PROMPT_COMMAND[@]}" "__moqui_at_prompt=1")
else
    PROMPT_COMMAND="__moqui_prompt${PROMPT_COMMAND:+;$PROMPT_COMMAND};__moqui_at_prompt=1"
fi
[[ "$PS1" == *'133;B'* ]] || PS1="$PS1"'\[\e]133;B\a\]'
builtin trap '__moqui_preexec' DEBUG
