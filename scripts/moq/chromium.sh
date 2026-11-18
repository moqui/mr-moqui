# shellcheck shell=bash
# moq's chromium-download. Sourced by moq, which holds the task table; bash 3.2, as moq is.

# ========== Chromium ==========

# The newest snapshot of $1 with the file $3 at or before the main-branch position the current Stable release of
# chromiumdash platform $2 branched from. Positions all have 7 digits, so the listing's name order is numeric order.
chromium_stable_revision() {
    local plat=$1 dash=$2 zip=$3 pos revs rev tries=0
    pos=$(curl -fsS --max-time 60 "https://chromiumdash.appspot.com/fetch_releases?channel=Stable&platform=$dash&num=1" |
        grep -o '"chromium_main_branch_position":[0-9]*' | head -n 1 | tr -cd '0-9')
    [ -n "$pos" ] || die "could not read the Stable branch position from chromiumdash.appspot.com"
    revs=$(curl -fsS --max-time 60 "https://www.googleapis.com/storage/v1/b/chromium-browser-snapshots/o?delimiter=/&prefix=$plat/&startOffset=$plat/$((pos - 1000))&endOffset=$plat/$((pos + 1))&fields=prefixes" |
        grep -o "$plat/[0-9]*/" | sed "s|^$plat/||; s|/\$||" | sort -rn) ||
        die "could not list the Chromium snapshots for $plat"
    for rev in $revs; do
        if curl -fsI --max-time 60 "$CHROMIUM_SNAPSHOTS/$plat/$rev/$zip" >/dev/null 2>&1; then
            printf '%s\n' "$rev"
            return 0
        fi
        tries=$((tries + 1))
        [ "$tries" -lt 20 ] || break
    done
    die "no $plat snapshot with $zip near the Stable position $pos"
}

# Where AppArmor limits unprivileged user namespaces, a Chromium with no profile cannot start its sandbox.
chromium_apparmor_note() {
    local binary profile=/etc/apparmor.d/moqui-chromium
    [ "$(cat /proc/sys/kernel/apparmor_restrict_unprivileged_userns 2>/dev/null)" = 1 ] || return 0
    # AppArmor matches the path with symlinks resolved.
    binary=$(cd -P "$CHROMIUM_DIR" && pwd)/chrome || return 0
    if grep -qsF "\"$binary\"" "$profile"; then
        return 0
    fi
    warn "AppArmor limits unprivileged user namespaces here, so Chromium cannot start its sandbox and PDF fails."
    warn "Add a profile for $binary (once, with sudo):"
    cat >&2 <<EOF

sudo tee $profile >/dev/null <<'END'
abi <abi/4.0>,
include <tunables/global>

profile moqui-chromium "$binary" flags=(unconfined) {
  userns,
}
END
sudo apparmor_parser -r $profile

EOF
}

task_chromium_download() {
    local revision='' latest='' cache_only='' plat dash zip name url archive work top cache cached
    while [ $# -gt 0 ]; do
        case $1 in
            --latest) latest=1 ;;
            --cache-only) cache_only=1 ;;
            -*) die "unknown option $1" ;;
            *[!0-9]* | '') die "a revision is a snapshot number, like 1697588, not $1" ;;
            *) revision=$1 ;;
        esac
        shift
    done
    case $(uname -s)/$(uname -m) in
        Linux/x86_64 | Linux/amd64)
            plat=Linux_x64
            dash=Linux
            zip=chrome-linux.zip
            ;;
        Darwin/arm64 | Darwin/aarch64)
            plat=Mac_Arm
            dash=Mac
            zip=chrome-mac.zip
            ;;
        Darwin/x86_64)
            plat=Mac
            dash=Mac
            zip=chrome-mac.zip
            ;;
        *) die "no Chromium snapshot for $(uname -s) $(uname -m); install Chromium another way and set moqui_conf__screen_facade__pdf__chromium_path" ;;
    esac
    need_tool curl unzip tar gzip
    # The snapshot repacked as a gzipped tar (modes and the macOS app's links kept) is kept in the cache an
    # --embed-chromium build embeds from (crates/moqui-embedded/build.rs), with its revision beside it.
    cache=$(embed_cache_dir)
    cached=$cache/chromium-$(embed_host_target).tar.gz || exit 1
    if [ -z "$revision" ]; then
        if [ -n "$latest" ]; then
            revision=$(curl -fsS --max-time 60 "$CHROMIUM_SNAPSHOTS/$plat/LAST_CHANGE" | tr -cd '0-9')
            [ -n "$revision" ] || die "could not read $CHROMIUM_SNAPSHOTS/$plat/LAST_CHANGE"
        else
            revision=$(chromium_stable_revision "$plat" "$dash" "$zip") || exit 1
        fi
    fi
    name=chromium-$revision-$zip
    url=$CHROMIUM_SNAPSHOTS/$plat/$revision/$zip
    archive=$RUNTIME_DIR/$name
    if [ -f "$archive" ]; then
        info "Using $(rel "$archive")"
    else
        info "Downloading $url"
        run_cmd curl -fL --progress-bar -o "$archive.part" "$url" || die "download failed"
        run_cmd mv "$archive.part" "$archive"
    fi
    check_download_sum "$archive" "chromium:$plat/$revision/$zip"
    [ -n "$cache_only" ] || rm_path "$CHROMIUM_DIR"
    if dry_run; then
        show_cmd unzip -q "$archive" -d "<temp>"
        show_cmd tar -czf "$cached" -C "<temp>" "${zip%.zip}"
        [ -n "$cache_only" ] || show_cmd mv "<temp>/${zip%.zip}" "$CHROMIUM_DIR"
    else
        work=$(mktemp -d "$RUNTIME_DIR/.moq-chromium.XXXXXX") || die "could not make a temp directory"
        if ! unzip -q "$archive" -d "$work"; then
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
        if ! { mkdir -p "$cache" && tar -czf "$cached.part" -C "$work" "$top" && mv "$cached.part" "$cached" &&
            printf '%s %s\n' "$revision" "$zip" >"$cached.revision"; }; then
            rm -rf "$work"
            rm -f -- "$cached.part"
            die "could not keep the Chromium archive in $cache"
        fi
        if [ -n "$cache_only" ]; then
            rm -rf "$work"
        else
            if ! mv "$work/$top" "$CHROMIUM_DIR"; then
                die "could not move Chromium into $(rel "$CHROMIUM_DIR")"
            fi
            rmdir "$work"
            printf '%s %s\n' "$revision" "$zip" >"$CHROMIUM_DIR/REVISION"
        fi
    fi
    rm_path "$archive"
    if [ -n "$cache_only" ]; then
        info "Chromium $revision is in $cached, for an --embed-chromium build"
        return 0
    fi
    info "Chromium $revision is in $(rel "$CHROMIUM_DIR"); a copy stays in $cache for an --embed-chromium build"
    [ "$plat" != Linux_x64 ] || chromium_apparmor_note
}


# ========== help ==========

help_chromium_download() {
    cat <<'EOF'
Downloads a Chromium snapshot build into runtime/chromium, replacing what is there. The server
uses it for PDF when screen-facade/pdf@chromium-path (moqui_conf__screen_facade__pdf__chromium_path)
is not set or names no file, before an installed Chromium, Google Chrome or Edge; the browser tests
use it too. Without it an installed one is found on the PATH or where the OS installs it; set
moqui_conf__screen_facade__pdf__chromium_path to use another.

The default build is the one at the main-branch position the current Stable release branched
from (from chromiumdash.appspot.com), or the nearest snapshot before it; it has the Stable code
without the fixes merged to the Stable branch later. A revision argument takes that snapshot.

Snapshots exist for Linux x64 and macOS (x64 and arm64). On Linux it needs the libraries Chromium
uses (nss, nspr, expat, and the like), as the distro chromium package does. Where AppArmor limits
unprivileged user namespaces (Ubuntu 23.10 and later) Chromium cannot start its sandbox without an
AppArmor profile for runtime/chromium/chrome; this task prints how to add one, which needs sudo.

The snapshot, repacked as a gzipped tar, is kept in the cache an --embed-chromium build (./moq
build --embed-chromium) takes it from, with its revision beside it (MOQUI_EMBED_CACHE moves it);
with --cache-only it only goes there, and runtime/chromium is left alone.

Options:
      --latest         The newest snapshot (tip of tree) instead of the Stable position
      --cache-only     Only put the archive in the cache, for an --embed-chromium build
EOF
}
