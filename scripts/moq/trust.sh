# shellcheck shell=bash
# Trust the public root of the per-user dev CA. Keys stay with the Rust issuer.

# The common name of every development root, and the start of its NSS nickname (`<prefix> <sha-256>`).
DEV_ROOT_PREFIX='MR Moqui development root'

task_trust_dev_cert() {
    local system='' remove='' arg shared root bin digest nick os
    for arg in "$@"; do
        case $arg in --system) system=1 ;; --untrust) remove=1 ;; *) die "unknown trust-dev-cert option: $arg" ;; esac
    done
    os=$(uname -s)
    case $os in Darwin|Linux) ;; *) die 'per-user dev certificate trust is supported on macOS and Linux' ;; esac
    shared=${MOQUI_SHARED_RUNTIME:-"$HOME/.mr-moqui"}
    case $shared in /*) ;; *) die 'MOQUI_SHARED_RUNTIME must name an absolute directory to install trust' ;; esac
    root=$shared/cert/dev-ca/ca.crt
    if dry_run; then
        if [ -n "$remove" ] && [ ! -f "$root" ]; then
            info "would remove every dev CA root named '$DEV_ROOT_PREFIX ...' (no shared dev CA at $root)"
        else
            info "would update dev CA trust for $root"
        fi
        return 0
    fi
    if [ -z "$remove" ]; then
        bin=$(moqui_binary "${OPT_PACKAGE:-release}") || return 1
        if ! MOQUI_RUNTIME="$RUNTIME_DIR" "$bin" dev-cert --shared; then
            echo 'An older self-signed runtime/cert/localhost certificate is moved aside, and a managed one issued, by:' >&2
            echo '  ./moq moqui dev-cert --replace   (then ./moq trust-dev-cert again)' >&2
            return 1
        fi
    fi
    # with no root left to compare, --untrust removes every development root by its name
    digest=
    if [ -f "$root" ]; then
        digest=$(dev_root_fingerprint "$root") || return 1
    elif [ -n "$remove" ]; then
        echo "No shared dev CA at $root: removing every root named '$DEV_ROOT_PREFIX ...'"
    else
        die "missing dev CA $root"
    fi
    nick="$DEV_ROOT_PREFIX $digest"
    if [ "$os" = Darwin ]; then
        local keychain hash
        if [ -n "$system" ]; then
            keychain=/Library/Keychains/System.keychain
        else
            # A login keychain can exist without a login-keychain preference.
            keychain=$(security login-keychain -d user) || keychain=
            if [ -z "$keychain" ]; then
                keychain=$HOME/Library/Keychains/login.keychain-db
                [ -f "$keychain" ] || die "no macOS login keychain at $keychain"
            fi
            keychain=${keychain#*\"}
            keychain=${keychain%\"*}
        fi
        if [ -n "$remove" ]; then
            while IFS= read -r hash; do
                [ -n "$hash" ] || continue
                [ -z "$digest" ] || [ "$hash" = "$digest" ] || continue
                if [ -n "$system" ]; then
                    if [ -n "$digest" ]; then sudo security remove-trusted-cert -d "$root" || return 1; fi
                    sudo security delete-certificate -Z "$hash" "$keychain" || return 1
                else
                    security delete-certificate -t -Z "$hash" "$keychain" || return 1
                fi
            done < <(keychain_dev_roots "$keychain")
        elif [ -n "$system" ]; then
            sudo security add-trusted-cert -d -r trustRoot -p ssl -k "$keychain" "$root" || return 1
        else
            security add-trusted-cert -r trustRoot -p ssl -k "$keychain" "$root" || return 1
            # a root an earlier --rotate replaced
            while IFS= read -r hash; do
                [ -z "$hash" ] || [ "$hash" = "$digest" ] || security delete-certificate -t -Z "$hash" "$keychain" || return 1
            done < <(keychain_dev_roots "$keychain")
        fi
        echo "Checked $keychain for ${digest:-$DEV_ROOT_PREFIX}"
        return
    fi
    local db profile failed=0
    if command -v certutil >/dev/null 2>&1; then
        # Chromium and Chrome read ~/.pki/nssdb on Linux; their snap and Flatpak builds and Firefox keep their own
        db=$HOME/.pki/nssdb
        if [ -z "$remove" ] && [ ! -f "$db/cert9.db" ]; then
            mkdir -p "$db" && certutil -N -d "sql:$db" --empty-password || return 1
        fi
        while IFS= read -r profile; do
            [ -f "$profile/cert9.db" ] || continue
            trust_nss_db "$profile" "$nick" "$root" "$digest" "$remove" || failed=1
        done < <({
            printf '%s\n' "$db" "$HOME/.local/share/pki/nssdb" "$HOME/snap/chromium/current/.pki/nssdb" \
                "$HOME/.var/app/org.chromium.Chromium/.pki/nssdb" "$HOME/.var/app/com.google.Chrome/.pki/nssdb"
            for profile in "$HOME/.mozilla/firefox" "$HOME/snap/firefox/common/.mozilla/firefox" \
                "$HOME/.var/app/org.mozilla.firefox/.mozilla/firefox"; do
                firefox_nss_profiles "$profile"
            done
        } | sort -u)
    else
        echo 'NSS stores skipped. Install certutil to configure Chromium and Firefox.' >&2
        failed=1
    fi
    local anchor_dir refresh anchor stale
    if command -v update-ca-certificates >/dev/null 2>&1; then
        anchor_dir=/usr/local/share/ca-certificates
        refresh=(update-ca-certificates)
    elif command -v update-ca-trust >/dev/null 2>&1; then
        anchor_dir=/etc/pki/ca-trust/source/anchors
        refresh=(update-ca-trust extract)
    else
        [ -z "$system" ] || die 'no supported Linux system trust command found'
        echo "System store unchanged. Use the public root at $root for clients with a custom CA file."
        return "$failed"
    fi
    anchor="$anchor_dir/mr-moqui-$digest.crt"
    if [ -n "$system" ]; then
        if [ -n "$digest" ] && [ -f "$anchor" ] && [ "$(dev_root_fingerprint "$anchor")" != "$digest" ]; then
            die "different certificate at $anchor"
        fi
        if [ -n "$remove" ] && [ -n "$digest" ]; then
            if [ -f "$anchor" ]; then sudo rm -- "$anchor" || return 1; fi
        elif [ -z "$remove" ]; then
            sudo install -m 0644 "$root" "$anchor" || return 1
        fi
        # with no root, every development root's anchor; when trusting, the ones an earlier --rotate replaced
        if [ -z "$digest" ] || [ -z "$remove" ]; then
            for stale in "$anchor_dir"/mr-moqui-*.crt; do
                [ -f "$stale" ] && [ "$stale" != "$anchor" ] || continue
                sudo rm -- "$stale" || return 1
            done
        fi
        # without --fresh, update-ca-certificates keeps the /etc/ssl/certs links of a removed local root, dangling
        if [ -n "$remove" ] && [ "${refresh[0]}" = update-ca-certificates ]; then refresh+=(--fresh); fi
        sudo "${refresh[@]}" || return 1
        echo "Checked system trust in $anchor_dir for ${digest:-$DEV_ROOT_PREFIX}"
    elif [ -n "$remove" ]; then
        printf 'To remove system trust, run ./moq trust-dev-cert --system --untrust\n'
    else
        printf 'System trust: sudo install -m 0644 %q %q && sudo' "$root" "$anchor"
        printf ' %q' "${refresh[@]}"
        printf '\n'
    fi
    return "$failed"
}

# The SHA-256 hash of each development root in a macOS keychain, by its common name.
keychain_dev_roots() {
    security find-certificate -a -c "$DEV_ROOT_PREFIX" -Z "$1" 2>/dev/null | awk '/^SHA-256 hash:/ { print $3 }'
}

# An existing Firefox NSS database, including a custom absolute Path in profiles.ini.
firefox_nss_profiles() {
    local base=$1 database
    for database in "$base"/*/cert9.db; do
        [ ! -f "$database" ] || printf '%s\n' "${database%/*}"
    done
    [ -f "$base/profiles.ini" ] || return 0
    awk -v base="$base" '
        function emit() {
            if (profile && path != "") print (relative == 0 ? path : base "/" path)
        }
        { sub(/\r$/, "") }
        /^\[/ { emit(); profile = /^\[Profile[0-9]+\]$/; path = ""; relative = 1; next }
        /^Path=/ { path = substr($0, 6) }
        /^IsRelative=/ { relative = substr($0, 12) }
        END { emit() }
    ' "$base/profiles.ini"
}

dev_root_fingerprint() {
    local fingerprint
    fingerprint=$(openssl x509 -in "$1" -noout -fingerprint -sha256) || return 1
    fingerprint=${fingerprint#*=}
    printf '%s\n' "${fingerprint//:/}"
}

# The nicknames of the development roots in an NSS database.
nss_dev_roots() {
    certutil -L -d "sql:$1" 2>/dev/null | grep -oE "^$DEV_ROOT_PREFIX [0-9A-Fa-f]{64}"
}

# Trust, or with remove untrust, the root under nick in one NSS database. With no digest (the root is gone), remove
# every development root by its nickname; when trusting, also the roots an earlier --rotate replaced.
trust_nss_db() {
    local db=$1 nick=$2 root=$3 digest=$4 remove=$5 found other
    if [ -n "$digest" ] && certutil -L -d "sql:$db" -n "$nick" >/dev/null 2>&1; then
        found=$(certutil -L -d "sql:$db" -n "$nick" -a | openssl x509 -noout -fingerprint -sha256) || return 1
        found=${found#*=}
        [ "${found//:/}" = "$digest" ] || { echo "Different root under $nick in $db" >&2; return 1; }
        if [ -n "$remove" ]; then certutil -D -d "sql:$db" -n "$nick" || return 1
        else certutil -M -d "sql:$db" -n "$nick" -t 'C,,' || return 1; fi
    elif [ -z "$remove" ]; then
        certutil -A -d "sql:$db" -n "$nick" -t 'C,,' -i "$root" || return 1
    fi
    if [ -z "$digest" ] || [ -z "$remove" ]; then
        while IFS= read -r other; do
            [ -n "$other" ] && [ "$other" != "$nick" ] || continue
            certutil -D -d "sql:$db" -n "$other" || return 1
        done < <(nss_dev_roots "$db")
    fi
    echo "Checked NSS trust in $db for ${digest:-$DEV_ROOT_PREFIX}"
}

help_trust_dev_cert() {
    echo 'moq trust-dev-cert [--system] [--untrust]
Trust the shared localhost dev CA for the current user (Chromium, Chrome and Firefox NSS stores, including their
snap and Flatpak builds, and the macOS login keychain). --system selects system trust. A root an earlier
moqui dev-cert --rotate replaced is removed. Use --untrust to remove the root from the selected stores; with no
shared dev CA left, it removes every root named "MR Moqui development root".'
}
