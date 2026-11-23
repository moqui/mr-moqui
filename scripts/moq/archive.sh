# shellcheck shell=bash
# moq's ./moq archive. Sourced by moq, which holds the task table; bash 3.2, as moq is.

# ========== archive ==========

# The archive for a file name ($1, may be empty) and default extension ($2, .zip or .tgz): relative to the caller's
# directory, the extension added unless it has one, and the default name inside a directory.
archive_path() {
    local file=$1 default
    default=mr-moqui-$(date +%Y%m%d)$2
    if [ -z "$file" ]; then
        abs_path "$default"
        return
    fi
    file=$(abs_path "$file")
    case $file in
        */)
            printf '%s\n' "$file$default"
            return
            ;;
    esac
    if [ -d "$file" ]; then
        printf '%s/%s\n' "$file" "$default"
        return
    fi
    case $file in *.zip | *.tgz | *.tar.gz) ;; *) file=$file$2 ;; esac
    printf '%s\n' "$file"
}

package_name() {
    case $1 in
        # default) echo Default ;;
        lite) echo Lite ;;
        debug) echo Debug ;;
        release) echo Maximal ;;
        enterprise) echo Enterprise ;;
    esac
}

# README.txt of an archive. $1 file, $2 directory name, $3 binary, $4 package, $5 conf, then the runtime entries.
archive_readme() {
    local file=$1 name=$2 bin=$3 package=$4 conf=$5 commit version entry list='' secrets=''
    shift 5
    for entry in "$@"; do
        list="$list ${entry#"$name"/}"
        [ "$entry" != "$name/.env" ] || secrets=1
    done
    commit=$(git -C "$ROOT" rev-parse --short HEAD 2>/dev/null) || commit=
    if [ -n "$commit" ] && [ -n "$(git -C "$ROOT" status --porcelain --untracked-files=no 2>/dev/null)" ]; then
        commit="$commit, with local changes"
    fi
    version=$(run_moqui "$bin" version 2>/dev/null) || version=
    {
        printf '%s\n\n' "$name"
        printf 'MR Moqui, from ./moq archive on %s%s.\n' "$(date +%Y-%m-%d)" "${commit:+ (commit $commit)}"
        printf 'The %s package, built for %s.\n' "$(package_name "$package")" "$(uname -sm)"
        [ -z "$version" ] || printf '%s\n' "$version" | sed 's/^/  /'
        cat <<EOF

Run
  cd $name && ./moqui starts the server on the listeners selected by MoquiConf.xml: this directory is its
  runtime, found from the working directory. ./moqui help lists its other commands and options.

  MoquiConf.xml is this instance's conf, merged over the framework defaults, a copy of
  $(rel "$conf").
  Edit it for this host, or choose another conf with --conf or MOQUI_CONF.

  With the embedded Postgres, the first start unpacks it from the binary into install/postgres/.

Contents
  moqui, MoquiConf.xml, README.txt, LICENSE.md, and from the runtime:$list
  Left out: log, tmp, save, install, db/install, .git, component target/ directories,${ARCHIVE_LEFT_OUT}
  OpenSearch logs and pid files.
EOF
        if [ -n "$secrets" ]; then
            cat <<EOF

  .env holds the generated secrets of this instance, among them the key of the database's
  encrypted fields. Keep it with the database, and keep this archive private.
EOF
        fi
    } >"$file"
}

# The exclude patterns of crates/moqui-embedded/archive-excludes.txt, which the runtime built into the binary leaves out
# too, for an archive whose top directory is $1: a name at any depth, a name ending in / at the top of a component.
archive_excludes() {
    local name=$1 line set component dir
    while IFS= read -r line || [ -n "$line" ]; do
        case $line in
            '' | '#'*) continue ;;
            */)
                # named one by one, since a wildcard would also take a directory of that name deeper in a component
                for set in "${COMPONENT_SETS[@]}"; do
                    for component in "$RUNTIME_DIR/$set"/*/; do
                        [ -d "$component${line%/}" ] || continue
                        dir=$name/$set/$(basename "$component")/${line%/}
                        printf '%s\n' "$dir" "$dir/*"
                    done
                done
                ;;
            '*'*) printf '%s\n' "$line" ;;
            *) printf '%s\n' "*/$line" "*/$line/*" ;;
        esac
    done <"$ROOT/crates/moqui-embedded/archive-excludes.txt"
}

# The SHA-256 of the file $1, in sha256sum's format with the file's base name.
sha256_line() {
    local sum
    if command -v sha256sum >/dev/null 2>&1; then
        sum=$(sha256sum "$1" | awk '{print $1}')
    else
        sum=$(shasum -a 256 "$1" | awk '{print $1}')
    fi
    printf '%s  %s\n' "$sum" "$(basename "$1")"
}

# ./moq archive --binary [file]: the binary alone, gzipped, with the runtime built in (--embed production unless another
# level is asked for), and beside it the checksums of its parts and of the .gz (moqui embedded checksum), which the
# binary checks at startup when the file is put where it looks. $1 the file name, $2 force.
archive_binary() {
    local file=$1 force=$2 out sums bin default
    need_tool gzip
    [ -n "$OPT_EMBED" ] || OPT_EMBED=production
    default=moqui-$(date +%Y%m%d).gz
    if [ -z "$file" ]; then
        out=$(abs_path "$default")
    elif [ -d "$file" ]; then
        out=$(abs_path "${file%/}")/$default
    else
        case $file in *.gz) out=$(abs_path "$file") ;; *) out=$(abs_path "$file.gz") ;; esac
    fi
    sums=${out%.gz}.sha256
    [ -d "$(dirname "$out")" ] || die "no directory $(rel "$(dirname "$out")") for $(basename "$out")"
    if { [ -e "$out" ] || [ -e "$sums" ]; } && [ -z "$force" ]; then
        die "$(rel "$out") or $(rel "$sums") is there already; -f replaces them"
    fi
    # the module is built in with the webroot, even with --nobuild
    ensure_js
    ensure_vault_wasm
    bin=$(moqui_binary "${OPT_PACKAGE:-release}") || exit 1
    if dry_run; then
        show_cmd gzip -9 -c "$bin" ">" "$out"
        show_cmd "$bin" embedded checksum ">" "$sums"
        return 0
    fi
    info "Writing $(rel "$out")"
    if ! gzip -9 -c "$bin" >"$out.part"; then
        rm -f -- "$out.part"
        die "could not write $(rel "$out")"
    fi
    mv -- "$out.part" "$out" || die "could not move $(rel "$out.part") to $(rel "$out")"
    if ! {
        printf '# The parts built into %s (moqui embedded checksum), checked at startup when this file is the\n' \
            "$(basename "$out" .gz)"
        printf '# runtime'"'"'s embedded.sha256 or moqui_embedded_checksums names it, and the .gz itself\n'
        printf '# (sha256sum -c --ignore-missing).\n'
        run_moqui "$bin" embedded checksum && sha256_line "$out"
    } >"$sums.part"; then
        rm -f -- "$sums.part"
        die "could not write $(rel "$sums")"
    fi
    mv -- "$sums.part" "$sums" || die "could not move $(rel "$sums.part") to $(rel "$sums")"
    local programs
    programs=$(embed_features | tr ',' '\n' | sed -n 's/^embed-\(opensearch\|chromium\)$/\1/p' | paste -sd, - | sed 's/,/ and /')
    info "Created $(rel "$out") ($(tidy_human_kb "$(du -sk "$out" | awk '{print $1}')")): the $(package_name "${OPT_PACKAGE:-release}") binary with the $OPT_EMBED runtime${programs:+ and $programs} built in, and $(rel "$sums")"
    info "To deploy: gunzip it into an empty directory and run ./moqui there; put $(basename "$sums") there as embedded.sha256 to check the binary at startup"
}

task_archive() {
    local file='' conf=$PRODUCTION_CONF force='' with_save='' no_data='' named='' tgz='' with_tests='' with_dev=''
    local binary='' conf_given='' took
    OPT_PACKAGE=
    OPT_NOBUILD=
    OPT_EMBED=
    OPT_EMBED_OPENSEARCH=
    OPT_EMBED_CHROMIUM=
    while [ $# -gt 0 ]; do
        took=$(set_embed_opt "$@") || exit 1
        if [ -n "$took" ]; then
            set_embed_opt "$@" >/dev/null
            shift "$took"
            continue
        fi
        case $1 in
            --binary) binary=1 ;;
            --lite) set_package lite ;;
            --debug) set_package debug ;;
            --release) set_package release ;;
            --enterprise) set_package enterprise ;;
            --nobuild) OPT_NOBUILD=1 ;;
            -c | --conf)
                [ $# -ge 2 ] || die "$1 needs a file"
                conf=$2
                conf_given=1
                shift
                ;;
            --conf=*)
                conf=${1#--conf=}
                conf_given=1
                ;;
            --tgz) tgz=1 ;;
            --with-save) with_save=1 ;;
            --with-tests) with_tests=1 ;;
            --with-dev) with_dev=1 ;;
            --no-data) no_data=1 ;;
            -f | --force) force=1 ;;
            -*) die "unknown option $1" ;;
            *)
                [ -z "$named" ] || die "archive takes one file name"
                file=$1
                named=1
                ;;
        esac
        shift
    done
    if [ -n "$binary" ]; then
        [ -z "$tgz$with_save$no_data$conf_given" ] ||
            die "--binary writes the binary alone; --tgz, --with-save, --no-data and --conf are for a runtime archive"
        if [ -n "$with_tests$with_dev" ]; then
            [ -z "$OPT_EMBED" ] || [ "$OPT_EMBED" = all ] || die "--with-tests and --with-dev build in --embed all"
            OPT_EMBED=all
        fi
        archive_binary "$file" "$force"
        return
    fi
    local out name
    if [ -n "$tgz" ]; then
        out=$(archive_path "$file" .tgz)
    else
        out=$(archive_path "$file" .zip)
    fi
    case $out in
        *.tar.gz)
            name=$(basename "$out" .tar.gz)
            tgz=1
            ;;
        *.tgz)
            name=$(basename "$out" .tgz)
            tgz=1
            ;;
        *)
            [ -z "$tgz" ] || die "--tgz writes a .tgz or .tar.gz, not $(basename "$out")"
            name=$(basename "$out" .zip)
            ;;
    esac
    if [ -n "$tgz" ]; then
        need_tool tar gzip
        if tar --help 2>&1 | grep -q BusyBox; then
            die "--tgz needs GNU or BSD tar; BusyBox tar takes only one -C"
        fi
    else
        need_tool zip
    fi
    valid_name "$name" || die "archive name '$name' may use only letters, digits, '.', '_' and '-'"
    [ -d "$(dirname "$out")" ] || die "no directory $(rel "$(dirname "$out")") for $(basename "$out")"
    # compared as physical paths: the runtime through a symlink, or a path with //, is still inside it
    local out_dir runtime_dir
    out_dir=$(cd -P "$(dirname "$out")" && pwd)
    runtime_dir=$(cd -P "$RUNTIME_DIR" 2>/dev/null && pwd) || runtime_dir=$RUNTIME_DIR
    case $out_dir/ in
        "$runtime_dir"/*) die "$(rel "$out") is inside $(rel "$RUNTIME_DIR"), which the archive holds" ;;
    esac
    if [ -e "$out" ] && [ -z "$force" ]; then
        die "$(rel "$out") is there already; -f replaces it"
    fi
    conf=$(resolve_conf "$conf") || exit 1
    [ -n "$no_data" ] || require_server_stopped archive
    # the runtime's webroot carries the vault's browser module, even with --nobuild
    ensure_js
    ensure_vault_wasm
    local bin
    bin=$(moqui_binary "${OPT_PACKAGE:-release}") || exit 1

    # The archive's own files take the place of runtime entries with their names.
    local entry base
    local -a entries excludes
    entries=()
    for entry in "$RUNTIME_DIR"/* "$RUNTIME_DIR"/.[!.]*; do
        [ -e "$entry" ] || [ -h "$entry" ] || continue
        base=${entry##*/}
        case $base in
            log | tmp | install | .git | moqui | MoquiConf.xml | README.txt | LICENSE.md) continue ;;
            save) [ -n "$with_save" ] || continue ;;
            db | txlog | .env | cert) [ -z "$no_data" ] || continue ;;
        esac
        entries+=("$name/$base")
    done
    excludes=("$name/cert/dev-ca/signer" "$name/cert/dev-ca/signer/*" "$name/cert/dev-ca/*.key" "$name/cert/dev-ca/.*" "$name/db/install" "$name/db/install/*" "$name/opensearch/logs/*" "$name/opensearch/pid")
    while IFS= read -r entry; do excludes+=("$entry"); done < <(archive_excludes "$name")
    [ -z "$no_data" ] || excludes+=("$name/opensearch/data/*")
    # A production archive carries no tests, plans and dev set unless asked (plans/archive/Components-Dir.md 3.6): each
    # component's test/ (its suites, agent replays and load scripts), plan/ and plans/, the test set, and the dev set.
    # Named one by one, since a wildcard would also take a directory of that name deeper in a component.
    local set component part dir
    ARCHIVE_LEFT_OUT=
    if [ -z "$with_dev" ]; then
        excludes+=("$name/$DEV_SET" "$name/$DEV_SET/*")
        ARCHIVE_LEFT_OUT=" the dev set ($DEV_SET; --with-dev keeps it),"
    fi
    if [ -z "$with_tests" ]; then
        excludes+=("$name/$TEST_SET" "$name/$TEST_SET/*")
        for set in "${COMPONENT_SETS[@]}"; do
            for component in "$RUNTIME_DIR/$set"/*/; do
                for part in test plan plans; do
                    [ -d "$component$part" ] || continue
                    dir=$name/$set/$(basename "$component")/$part
                    excludes+=("$dir" "$dir/*")
                done
            done
        done
        ARCHIVE_LEFT_OUT="$ARCHIVE_LEFT_OUT the test set ($TEST_SET) and each component's test/, plan/ and plans/
  (--with-tests keeps them),"
    fi

    # Beside the archive, so the binary is linked or copied onto the disk the archive goes to.
    local stage
    stage=$(mktemp -d "$(dirname "$out")/.moq-archive.XXXXXX") || die "could not make a temp directory"
    (
        trap 'rm -rf "$stage"' EXIT
        archive_write "$stage" "$out" "$name" "$bin" "$conf" "$no_data" "$tgz" "${#entries[@]}" "${entries[@]}" \
            "${excludes[@]}"
    ) || exit $?
    dry_run && return 0
    info "Created $(rel "$out") ($(tidy_human_kb "$(du -sk "$out" | awk '{print $1}')")), which expands to $name/"
    if [ -z "$no_data" ] && { [ -e "$RUNTIME_DIR/.env" ] || [ -d "$RUNTIME_DIR/cert" ]; }; then
        warn "the archive holds .env and cert/, the secrets of this instance; keep it private"
    fi
}

# Stage the archive's top directory in $1 and write it to $2. Then: name, binary, conf, no_data, tgz, the number of
# runtime entries, the entries, and the exclude patterns. The archive's own files are in $1/top/<name>; the runtime
# entries are read through $1/rt/<name>, a link to the runtime, so the archive holds the links inside it as links.
archive_write() {
    local stage=$1 out=$2 name=$3 bin=$4 conf=$5 no_data=$6 tgz=$7 count=$8 status
    shift 8
    local -a entries excludes top
    entries=("${@:1:count}")
    excludes=("${@:count+1}")
    local dir=$stage/top/$name
    mkdir -p "$dir" "$stage/rt" || die "could not stage $name"
    ln "$bin" "$dir/moqui" 2>/dev/null || run_cmd cp -p "$bin" "$dir/moqui" || die "could not stage $name/moqui"
    cp "$conf" "$dir/MoquiConf.xml" && ln -s "$RUNTIME_DIR" "$stage/rt/$name" || die "could not stage $name"
    top=("$name/moqui" "$name/MoquiConf.xml" "$name/README.txt")
    if [ -f "$ROOT/LICENSE.md" ]; then
        cp "$ROOT/LICENSE.md" "$dir/LICENSE.md" || die "could not stage $name"
        top+=("$name/LICENSE.md")
    fi
    archive_readme "$dir/README.txt" "$name" "$bin" "${OPT_PACKAGE:-release}" "$conf" "${entries[@]}"
    local part=$out.part
    rm -f -- "$part"
    if [ -e "$out" ]; then
        run_cmd rm -f -- "$out" || die "could not replace $(rel "$out")"
    fi
    info "Writing $(rel "$out")"
    if [ -z "$no_data" ] && [ -d "$OS_DIR/data" ]; then
        opensearch_pause
    fi
    if [ -n "$tgz" ]; then
        local pattern
        local -a tar_excludes
        tar_excludes=()
        for pattern in "${excludes[@]}"; do
            tar_excludes+=("--exclude=$pattern")
        done
        # The exclusions go before the files, as GNU tar wants them.
        run_cmd tar -czf "$part" "${tar_excludes[@]}" -C "$stage/top" "${top[@]}" -C "$stage/rt" "${entries[@]}"
        status=$?
    else
        if dry_run; then
            show_cmd cd "$stage/top"
        fi
        (cd "$stage/top" && run_cmd zip -q "$part" "${top[@]}")
        status=$?
        if [ "$status" -eq 0 ]; then
            if dry_run; then
                show_cmd cd "$stage/rt"
            fi
            # -y keeps the links inside the runtime as links; the rt/<name> link is only a directory on the way in.
            (cd "$stage/rt" && run_cmd zip -qry "$part" "${entries[@]}" -x "${excludes[@]}")
            status=$?
        fi
    fi
    opensearch_resume
    case $tgz:$status in
        *:0) ;;
        :18) warn "zip could not read some files, and the archive leaves them out" ;;
        *)
            rm -f -- "$part"
            die "writing $(basename "$out") failed (exit $status)"
            ;;
    esac
    run_cmd mv -- "$part" "$out" || die "could not move $(rel "$part") to $(rel "$out")"
}


# ========== help ==========

help_archive() {
    cat <<EOF
Writes a zip, or with --tgz a gzipped tar, that expands to one directory named for the file
without .zip or .tgz. That directory is a runtime directory with the binary in it:
  moqui           the binary of the package asked for (the Maximal package unless --lite, --debug
                  or --enterprise)
  MoquiConf.xml   the instance conf, merged over the framework defaults: a copy of
                  conf/MoquiProductionConf.xml, or the --conf file
  README.txt      how to run it, the package, and what went in
  LICENSE.md
  and all of runtime/ (.env, components, conf, db, ...) except log/, tmp/, save/ and
  install/, with Chromium, OpenSearch and the embedded database when they are there
Started in that directory (cd <dir> && ./moqui), the binary uses it as the runtime, since it
has components/base/, and merges the MoquiConf.xml there; the runtime is found from the working
directory, never from the binary. On its first start it unpacks the embedded Postgres into
<dir>/install.

The file goes in the current directory unless named elsewhere, with .zip (or .tgz) added when
the name lacks it; a name ending in .tgz or .tar.gz means --tgz. A directory gets the default
name inside it. The archive also leaves out .git, the target/ of component crates, OpenSearch
logs, and pid files. A production archive carries no tests, plans or dev tools: it leaves out each
component's test/ (its regression and acceptance suites, agent replays and load scripts), plan/ and
plans/, and the test set, components/test, unless --with-tests, and the dev set, components/dev,
unless --with-dev. The database is copied as files, so the embedded Postgres must be stopped;
an OpenSearch daemon with a pid file is stopped for the copy and started again after. Needs zip,
or tar and gzip.
  ./moq archive
  ./moq archive --release /backups/shop.zip
  ./moq archive --tgz --no-data dist/

With --binary it writes the binary alone instead, gzipped (moqui-YYYYMMDD.gz), built with the same
file set inside it (--embed production, or --embed all with --with-tests or --with-dev), and beside
it moqui-YYYYMMDD.sha256: the checksums of the parts built in (moqui embedded checksum) and of the
.gz. Unpacked into an empty directory, ./moqui runs from the runtime built in, unpacking Postgres
(and OpenSearch and Chromium with --embed-opensearch and --embed-chromium) into install/ when first
needed; the .sha256 put there as embedded.sha256 makes it check itself at startup and refuse to
start when a part differs (docs/Run-Deploy.md, Deploy a single binary).
  ./moq archive --binary --embed-opensearch --embed-chromium dist/

Options:
$PACKAGE_OPTIONS_HELP
$EMBED_OPTIONS_HELP
      --binary         The binary alone, gzipped, with the runtime built in, and its checksums
      --nobuild        Skip the build that runs first. A binary that is not there is still built
  -c, --conf FILE      The conf put in the archive as MoquiConf.xml (default
                       conf/MoquiProductionConf.xml); relative to the runtime directory, or a path
      --tgz            A gzipped tar (.tgz) instead of a zip
      --with-save      Include runtime/save (the save-db copies)
      --with-tests     Keep each component's test/, plan/ and plans/, and the test set (an archive for a
                       staging server that runs moqui test or client specs against itself)
      --with-dev       Keep the dev set (the dev tools still load only on a dev instance)
      --no-data        Leave out the instance data and secrets: runtime/db, the OpenSearch data,
                       runtime/txlog, runtime/.env and runtime/cert
  -f, --force          Replace the file when it is there
EOF
}
