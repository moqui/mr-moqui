# shellcheck shell=bash
# moq's ./moq run, run-production, load, load-production and load-save. Sourced by moq, which holds the task table; bash 3.2, as moq is.

# ========== run and load tasks ==========
task_run() {
    parse_run_opts run "$@"
    local bin
    bin=$(moqui_binary "${OPT_PACKAGE:-release}") || exit 1
    apply_run_env ""
    exec_moqui "$bin" "${OPT_REST[@]}"
}

task_run_production() {
    parse_run_opts run "$@"
    local bin
    bin=$(moqui_binary "${OPT_PACKAGE:-release}") || exit 1
    apply_run_env "$PRODUCTION_CONF"
    exec_moqui "$bin" "${OPT_REST[@]}"
}

# $1 conf, $2 default types ("" for moqui's default), then the load arguments.
run_load() {
    local conf=$1 default_types=$2 bin word types='' has_types=''
    local -a flags
    shift 2
    parse_run_opts load "$@"
    flags=()
    for word in "${OPT_REST[@]}"; do
        case $word in
            types=*)
                has_types=1
                flags+=("$word")
                ;;
            location=*) flags+=("location=$(abs_path "${word#location=}")") ;;
            *=*) flags+=("$word") ;;
            *) types="${types:+$types,}$word" ;;
        esac
    done
    if [ -n "$types" ]; then
        [ -z "$has_types" ] || die "give data types as words or as types=, not both"
        flags=("types=$types" "${flags[@]}")
    elif [ -z "$has_types" ] && [ -n "$default_types" ]; then
        flags=("types=$default_types" "${flags[@]}")
    fi
    if [ -n "$OPT_LOCATION" ]; then
        local location
        location=$(abs_path "$OPT_LOCATION")
        [ -f "$location" ] || die "data file $OPT_LOCATION not found"
        flags+=("location=$location")
    fi
    bin=$(moqui_binary "${OPT_PACKAGE:-release}") || exit 1
    apply_run_env "$conf"
    exec_moqui "$bin" load "${flags[@]}"
}

task_load() {
    run_load "" all "$@"
}

task_load_production() {
    run_load "$PRODUCTION_CONF" "" "$@"
}

task_load_save() {
    local name=default
    local -a load_args save_args
    load_args=()
    save_args=()
    while [ $# -gt 0 ]; do
        case $1 in
            # save-db runs the binary the load built, with no second build.
            --lite | --debug | --release | --enterprise)
                load_args+=("$1")
                save_args+=("$1")
                ;;
            -n | --name)
                [ $# -ge 2 ] || die "$1 needs a name"
                name=$2
                shift
                ;;
            --name=*) name=${1#--name=} ;;
            *) load_args+=("$1") ;;
        esac
        shift
    done
    valid_name "$name" || die "save name '$name' may use only letters, digits, '.', '_' and '-'"
    clean_all
    (task_load "${load_args[@]}") || die "load failed; nothing was saved"
    task_save_db "$name" "${save_args[@]}" --nobuild
}


# ========== help ==========

help_run() {
    cat <<EOF
Runs the binary in place with the runtime and framework directories in this repo. With no
moqui arguments it starts the HTTP server. The first argument that is not a moq option, and
everything after it, goes to the binary, so moqui's own commands run with the same conf:
  ./moq run                         serve with runtime/conf/MoquiDevConf.xml
  ./moq run serve                   that server in the background; logs stay in the files and the search indexes
  ./moq run -p 8081 -D moqui_conf__entity_facade__datasource__TRANSACTIONAL__database_conf_name=postgres
  ./moq run --release               the Maximal binary, still the dev conf
  ./moq run lint --screens          moqui lint --screens
  ./moq run types --out /tmp/types

Options:
$RUN_OPTIONS_HELP
EOF
}

help_run_production() {
    cat <<EOF
Like run, but with conf/MoquiProductionConf.xml unless --conf says otherwise. For a distribution,
build the binary as README.md describes (scripts/pg-payload.sh strip, then ./moq build).
  ./moq run-production
  ./moq run-production --enterprise -D moqui_conf__entity_facade__datasource__TRANSACTIONAL__database_conf_name=postgres

Options:
$RUN_OPTIONS_HELP
EOF
}

help_load() {
    cat <<EOF
Loads data into the configured database with the dev conf. With no data
types it loads all of them, like gradle load.
  ./moq load
  ./moq load seed seed-initial install
  ./moq load -l runtime/components/addon/mr-mantle/data/20-install/GeographyEurope.xml
  ./moq load seed components=mr-mantle

$LOAD_HELP
EOF
}

help_load_production() {
    cat <<EOF
Loads data with conf/MoquiProductionConf.xml. With no data types it
loads moqui's default set: seed, seed-initial, install.
  ./moq load-production
  ./moq load-production seed

$LOAD_HELP
EOF
}

help_load_save() {
    cat <<'EOF'
gradle loadSave: clean-all, then load with the given arguments, then save-db.
  ./moq load-save
  ./moq load-save -n demo seed seed-initial install demo

Options:
  -n, --name NAME      The save name (default "default")
Any other argument goes to load.
EOF
}
