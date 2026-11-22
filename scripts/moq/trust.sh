# shellcheck shell=bash
# Trust the public root of the per-user dev CA. Keys stay with the Rust issuer.
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
        info "would update dev CA trust for $root"
        return 0
    fi
    if [ -z "$remove" ]; then
        bin=$(moqui_binary "${OPT_PACKAGE:-release}") || return 1
        MOQUI_RUNTIME="$RUNTIME_DIR" "$bin" dev-cert --shared || return 1
    fi
    if [ ! -f "$root" ]; then
        [ -n "$remove" ] && { echo "No shared dev CA at $root"; return 0; }
        die "missing dev CA $root"
    fi
    digest=$(dev_root_fingerprint "$root") || return 1
    nick="MR Moqui development root $digest"
    if [ "$os" = Darwin ]; then
        local keychain
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
            local fingerprints
            fingerprints=$(security find-certificate -a -Z "$keychain") || { [ "$?" = 44 ] || return 1; }
            if [[ "$fingerprints" == *"SHA-256 hash: $digest"* ]]; then
                if [ -n "$system" ]; then
                    sudo security remove-trusted-cert -d "$root" && sudo security delete-certificate -Z "$digest" "$keychain" || return 1
                else
                    security delete-certificate -t -Z "$digest" "$keychain" || return 1
                fi
            fi
        elif [ -n "$system" ]; then
            sudo security add-trusted-cert -d -r trustRoot -p ssl -k "$keychain" "$root" || return 1
        else
            security add-trusted-cert -r trustRoot -p ssl -k "$keychain" "$root" || return 1
        fi
        echo "Checked $keychain for $digest"
        return
    fi
    local db profile failed=0
    if command -v certutil >/dev/null 2>&1; then
        db=$HOME/.local/share/pki/nssdb
        if [ -f "$HOME/.pki/nssdb/cert9.db" ]; then db=$HOME/.pki/nssdb; fi
        if [ -z "$remove" ] && [ ! -f "$db/cert9.db" ]; then
            mkdir -p "$db" && certutil -N -d "sql:$db" --empty-password || return 1
        fi
        while IFS= read -r profile; do
            [ -f "$profile/cert9.db" ] || continue
            trust_nss_db "$profile" "$nick" "$root" "$digest" "$remove" || failed=1
        done < <({
            printf '%s\n' "$db"
            if [ -n "$remove" ]; then printf '%s\n' "$HOME/.pki/nssdb" "$HOME/.local/share/pki/nssdb"; fi
            for profile in "$HOME/.mozilla/firefox" "$HOME/snap/firefox/common/.mozilla/firefox" \
                "$HOME/.var/app/org.mozilla.firefox/.mozilla/firefox"; do
                firefox_nss_profiles "$profile"
            done
        } | sort -u)
    else
        echo 'NSS stores skipped. Install certutil to configure Chromium and Firefox.' >&2
        failed=1
    fi
    local anchor_dir refresh anchor
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
        if [ -f "$anchor" ] && [ "$(dev_root_fingerprint "$anchor")" != "$digest" ]; then
            die "different certificate at $anchor"
        fi
        if [ -n "$remove" ]; then
            if [ -f "$anchor" ]; then sudo rm -- "$anchor" || return 1; fi
        else
            sudo install -m 0644 "$root" "$anchor" || return 1
        fi
        sudo "${refresh[@]}" || return 1
        echo "Checked system trust in $anchor for $digest"
    elif [ -n "$remove" ]; then
        printf 'To remove system trust, run ./moq trust-dev-cert --system --untrust\n'
    else
        printf 'System trust: sudo install -m 0644 %q %q && sudo' "$root" "$anchor"
        printf ' %q' "${refresh[@]}"
        printf '\n'
    fi
    return "$failed"
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

trust_nss_db() {
    local db=$1 nick=$2 root=$3 digest=$4 remove=$5 found
    if certutil -L -d "sql:$db" -n "$nick" >/dev/null 2>&1; then
        found=$(certutil -L -d "sql:$db" -n "$nick" -a | openssl x509 -noout -fingerprint -sha256) || return 1
        found=${found#*=}
        [ "${found//:/}" = "$digest" ] || { echo "Different root under $nick in $db" >&2; return 1; }
        if [ -n "$remove" ]; then certutil -D -d "sql:$db" -n "$nick" || return 1
        else certutil -M -d "sql:$db" -n "$nick" -t 'C,,' || return 1; fi
    elif [ -z "$remove" ]; then
        certutil -A -d "sql:$db" -n "$nick" -t 'C,,' -i "$root" || return 1
    fi
    echo "Checked NSS trust in $db for $digest"
}

help_trust_dev_cert() {
    echo 'moq trust-dev-cert [--system] [--untrust]
Trust the shared localhost dev CA for the current user. --system selects system trust.
Use --untrust to remove the matching root from the selected stores.'
}
