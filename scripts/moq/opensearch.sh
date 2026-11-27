# shellcheck shell=bash
# moq's opensearch-download, -start and -stop. Sourced by moq, which holds the task table; bash 3.2, as moq is.

# ========== OpenSearch ==========

opensearch_pid() {
    live_pid "$OS_DIR/pid"
}

opensearch_start() {
    local pid
    [ -x "$OS_DIR/bin/opensearch" ] || die "no OpenSearch in $(rel "$OS_DIR"); get one with ./moq opensearch-download"
    if pid=$(opensearch_pid); then
        info "OpenSearch is already running in $(rel "$OS_DIR") (pid $pid)"
        return 0
    fi
    if [ -f "$OS_DIR/pid" ]; then
        info "Removing the stale pid file $(rel "$OS_DIR/pid")"
        run_cmd rm -f "$OS_DIR/pid"
    fi
    run_cmd mkdir -p "$OS_DIR/logs"
    info "Starting OpenSearch in $(rel "$OS_DIR")"
    if dry_run; then
        show_cmd cd "$OS_DIR"
        show_cmd ./bin/opensearch -d -p pid
        return 0
    fi
    (cd "$OS_DIR" && ./bin/opensearch -d -p pid) || die "OpenSearch did not start; see $(rel "$OS_DIR/logs")"
    if pid=$(opensearch_pid); then
        info "OpenSearch started (pid $pid)"
    fi
}

opensearch_stop() {
    local pid waited=0
    if ! pid=$(opensearch_pid); then
        if [ -f "$OS_DIR/pid" ]; then
            info "Removing the stale pid file $(rel "$OS_DIR/pid")"
            run_cmd rm -f "$OS_DIR/pid"
        else
            info "OpenSearch in $(rel "$OS_DIR") is not running (no pid file)"
        fi
        return 0
    fi
    info "Stopping OpenSearch (pid $pid)"
    run_cmd kill "$pid" || die "could not signal pid $pid"
    dry_run && return 0
    while pid_alive "$pid"; do
        if [ "$waited" -ge 120 ]; then
            die "OpenSearch (pid $pid) did not exit within 120 seconds"
        fi
        sleep 1
        waited=$((waited + 1))
    done
    rm -f "$OS_DIR/pid"
    info "OpenSearch stopped"
}

# Stop a pid-file daemon around changes to its data. OS_PAUSED says whether to start it again.
OS_PAUSED=
opensearch_pause() {
    OS_PAUSED=
    if opensearch_pid >/dev/null; then
        opensearch_stop
        OS_PAUSED=1
    elif server_pid >/dev/null && [ -x "$OS_DIR/bin/opensearch" ] && [ -d "$OS_DIR/data" ]; then
        warn "the server is running and may have started OpenSearch itself; its data may change while this runs"
    fi
}

opensearch_resume() {
    if [ -n "$OS_PAUSED" ]; then
        opensearch_start
    fi
    OS_PAUSED=
}

# Delete OpenSearch data and logs, keeping the install.
clean_opensearch_data() {
    [ -d "$OS_DIR/data" ] || [ -d "$OS_DIR/logs" ] || return 0
    local f
    rm_path "$OS_DIR/data"
    if [ -d "$OS_DIR/logs" ]; then
        for f in "$OS_DIR"/logs/* "$OS_DIR"/logs/.[!.]*; do
            if [ -e "$f" ]; then
                rm_path "$f"
            fi
        done
    fi
}

task_opensearch_start() {
    [ $# -eq 0 ] || die "opensearch-start takes no arguments"
    opensearch_start
}

task_opensearch_stop() {
    [ $# -eq 0 ] || die "opensearch-stop takes no arguments"
    opensearch_stop
}

task_opensearch_download() {
    local version=$OPENSEARCH_VERSION force='' cache_only='' arch name url archive work top pid cache cached
    while [ $# -gt 0 ]; do
        case $1 in
            -f | --force) force=1 ;;
            --cache-only) cache_only=1 ;;
            -*) die "unknown option $1" ;;
            *) version=$1 ;;
        esac
        shift
    done
    case $(uname -s) in
        Linux) ;;
        Darwin) die "OpenSearch publishes no macOS tarball; run it another way (Docker, Homebrew) and set moqui_conf__search_facade__cluster__url" ;;
        *) die "opensearch-download supports Linux only" ;;
    esac
    case $(uname -m) in
        x86_64 | amd64) arch=x64 ;;
        aarch64 | arm64) arch=arm64 ;;
        *) die "no OpenSearch tarball for $(uname -m)" ;;
    esac
    if [ -z "$cache_only" ] && [ -e "$OS_DIR" ]; then
        if pid=$(opensearch_pid); then
            die "OpenSearch is running in $(rel "$OS_DIR") (pid $pid); ./moq opensearch-stop first"
        fi
        [ -n "$force" ] || die "$(rel "$OS_DIR") exists; add --force to replace it, which deletes its data"
    fi
    need_tool curl tar
    name=opensearch-min-$version-linux-$arch.tar.gz
    url=https://artifacts.opensearch.org/releases/core/opensearch/$version/$name
    # kept in the cache an --embed-opensearch build embeds from (crates/moqui-embedded/build.rs)
    cache=$(embed_cache_dir)
    cached=$cache/opensearch-$version-$(embed_host_target).tar.gz || exit 1
    archive=$RUNTIME_DIR/$name
    if [ -f "$cached" ]; then
        info "Using $cached"
    else
        run_cmd mkdir -p "$cache" || die "could not make $cache"
        if [ -f "$archive" ]; then
            info "Using $(rel "$archive")"
            run_cmd mv "$archive" "$cached.part" || die "could not move $(rel "$archive") to $cache"
        else
            info "Downloading $url"
            run_cmd curl -fL --progress-bar -o "$cached.part" "$url" || die "download failed"
        fi
        verify_published_sha512 "$cached.part" "$url.sha512"
        run_cmd mv "$cached.part" "$cached" || die "could not move the archive into $cache"
    fi
    if [ -n "$cache_only" ]; then
        info "OpenSearch $version is in $cached, for an --embed-opensearch build"
        return 0
    fi
    archive=$cached
    rm_path "$OS_DIR"
    if dry_run; then
        show_cmd tar -xzf "$archive" -C "<temp>"
        show_cmd mv "<temp>/opensearch-$version" "$OS_DIR"
    else
        work=$(mktemp -d "$RUNTIME_DIR/.moq-opensearch.XXXXXX") || die "could not make a temp directory"
        if ! tar -xzf "$archive" -C "$work"; then
            rm -rf "$work"
            die "could not unpack $(rel "$archive")"
        fi
        top=$(ls "$work")
        case $top in
            '' | *[[:space:]]*)
                rm -rf "$work"
                die "unexpected layout in $(rel "$archive")"
                ;;
        esac
        if ! mv "$work/$top" "$OS_DIR"; then
            die "could not move OpenSearch into $(rel "$OS_DIR")"
        fi
        rmdir "$work"
        mkdir -p "$OS_DIR/logs"
    fi
    info "OpenSearch $version is in $(rel "$OS_DIR"); its archive stays in $cache for an --embed-opensearch build"
}

# Check the file $1 against the SHA-512 published at the url $2 (a sha512sum line), which OpenSearch publishes beside
# each tarball; the file is removed and the task stops when it differs, or when no sum can be read: an archive that is
# run is never used unchecked (plans/Security-Review-Low.md SEC-OUT-91).
verify_published_sha512() {
    local file=$1 url=$2 expected actual
    dry_run && return 0
    expected=$(curl -fsSL --max-time 60 "$url" 2>/dev/null | awk '{print tolower($1); exit}')
    if [ -z "$expected" ]; then
        rm -f "$file"
        die "could not read $url, so the archive cannot be checked; it was removed (try again)"
    fi
    if command -v sha512sum >/dev/null 2>&1; then
        actual=$(sha512sum "$file" | awk '{print $1}')
    else
        actual=$(shasum -a 512 "$file" | awk '{print $1}')
    fi
    if [ "$actual" != "$expected" ]; then
        rm -f -- "$file"
        die "the download does not match its published SHA-512 ($url)"
    fi
    info "Checked against $url"
}

# The archive $1 for download $2 (a key that names one fixed archive, such as a Chromium snapshot's revision or an
# add-on's version) against the SHA-256 recorded for it in downloads.sha256 of the user's moqui cache: recorded the first
# time, and a later download of it must match, else it is removed and the task stops (SEC-OUT-14). A moving archive (a
# branch's) is not checked.
check_download_sum() {
    local file=$1 key=$2 sums actual known
    dry_run && return 0
    sums=$(dirname "$(embed_cache_dir)")/downloads.sha256
    actual=$(sha256_line "$file" | awk '{print $1}')
    [ -n "$actual" ] || die "could not compute the SHA-256 of $(rel "$file")"
    known=$(awk -v key="$key" '$2 == key {print $1; exit}' "$sums" 2>/dev/null)
    if [ -z "$known" ]; then
        mkdir -p "$(dirname "$sums")" && printf '%s %s\n' "$actual" "$key" >>"$sums" ||
            die "could not record the SHA-256 of $key in $sums"
        info "Recorded the SHA-256 of $key in $sums"
    elif [ "$known" != "$actual" ]; then
        rm -f -- "$file"
        die "$key does not match the SHA-256 recorded for it in $sums when it was first downloaded; the archive was changed, or $sums names it wrongly"
    else
        info "Checked $key against the SHA-256 recorded for it"
    fi
}


# ========== help ==========

help_opensearch_download() {
    cat <<EOF
Downloads opensearch-min for Linux (x64 or arm64) into runtime/opensearch. The version
defaults to $OPENSEARCH_VERSION (.cargo/config.toml, MOQUI_OPENSEARCH_VERSION). The download is
checked against the SHA-512 OpenSearch publishes beside it. An archive already in runtime/ is used
instead of downloading. OpenSearch publishes no macOS tarball; on macOS run it another way and set
moqui_conf__search_facade__cluster__url.

The archive is kept in $(embed_cache_dir)
(MOQUI_EMBED_CACHE moves it), where an --embed-opensearch build (./moq build --embed-opensearch)
finds it; with --cache-only it only goes there, and runtime/opensearch is left alone.

Options:
  -f, --force          Replace an existing runtime/opensearch, deleting its data
      --cache-only     Only put the archive in the cache, for an --embed-opensearch build
EOF
}

help_opensearch_start() {
    cat <<'EOF'
Starts runtime/opensearch as a daemon (bin/opensearch -d -p pid). The server uses a node that
answers and only starts its own when nothing does and there is no pid file, so a daemon started
here outlives server restarts.
EOF
}

help_opensearch_stop() {
    echo 'Stops the runtime/opensearch daemon named by its pid file and waits for it to exit.'
}
