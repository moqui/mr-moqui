# shellcheck shell=bash
# moq's ./moq desktop and android: the Tauri apps' builds, signing and staging. Sourced by moq, which holds the task table; bash 3.2, as moq is.

# ========== desktop and android tasks ==========
# The Tauri CLI version the desktop app is built with: the tauri crate's version in crates/moqui-desktop/Cargo.toml.
TAURI_CLI_VERSION=3.0.0-alpha.6
# Where the AppImage step gets the helper sources quick-sharun.sh compiles in (task_desktop).
ANYLINUX_APPIMAGES_LIB=https://raw.githubusercontent.com/pkgforge-dev/Anylinux-AppImages/df9cd3246ccbcf61eb22fc321a52277351047b4b/useful-tools/lib

# cargo, and the Tauri CLI of TAURI_CLI_VERSION.
need_tauri_cli() {
    need_cargo
    local version
    version=$(cargo tauri --version 2>/dev/null) \
        || die "the Tauri CLI is not installed: cargo install tauri-cli --version $TAURI_CLI_VERSION --locked"
    [ "${version##* }" = "$TAURI_CLI_VERSION" ] \
        || die "the Tauri CLI is ${version##* }, the app needs $TAURI_CLI_VERSION: cargo install tauri-cli --version $TAURI_CLI_VERSION --locked"
}

# cef-dll-sys runs cmake with the Ninja generator. On Windows, download a pinned CMake once
# (scripts/cmake.sh) when none is on PATH, and prepend its bin. A dry run prints the curl and does not download.
# MOQUI_CMAKE, a directory or cmake.exe, is used as it is. Linux and macOS keep the cmake on PATH.
ensure_cmake() {
    local script=$ROOT/scripts/cmake.sh dir
    [ -f "$script" ] || die "missing $(rel "$script")"
    dir=$("$script" ensure | tr -d '\r') || die "could not get CMake"
    [ -n "$dir" ] || return 0
    export PATH="$dir:$PATH"
}

# The same build needs ninja. On Windows, download a pinned Ninja once (scripts/ninja.sh) when none is on PATH.
# A dry run prints the curl and does not download. MOQUI_NINJA, a directory or ninja.exe, is used as it is.
ensure_ninja() {
    local script=$ROOT/scripts/ninja.sh dir
    [ -f "$script" ] || die "missing $(rel "$script")"
    dir=$("$script" ensure | tr -d '\r') || die "could not get Ninja"
    [ -n "$dir" ] || return 0
    export PATH="$dir:$PATH"
}

# macOS: the cmake and ninja on PATH, which this script never downloads. cef-dll-sys builds CEF's C++ wrapper with them
# on Windows and macOS only; on Linux it links libcef.so and needs neither.
need_cef_build_tools() {
    local tool missing=
    for tool in cmake ninja; do
        command -v "$tool" >/dev/null 2>&1 || missing+="${missing:+ or }$tool"
    done
    [ -z "$missing" ] || die "the desktop build needs CMake and Ninja to build CEF's C++ wrapper, and the PATH has no $missing." \
        "Install them: brew install cmake ninja."
}

task_desktop() {
    local command=dev release=
    local -a cargo_args
    cargo_args=()
    while [ $# -gt 0 ]; do
        case $1 in
            dev | build) command=$1 ;;
            --release) release=1 ;;
            --)
                shift
                cargo_args+=("$@")
                break
                ;;
            *) cargo_args+=("$1") ;;
        esac
        shift
    done
    need_tauri_cli
    # CMake and Ninja build CEF's C++ wrapper on Windows and macOS. On Windows they are downloaded when missing
    # (scripts/cmake.sh, scripts/ninja.sh); on macOS they have to be on PATH; Linux needs neither. The C++ compiler is
    # never fetched.
    ensure_cmake
    ensure_ninja
    [ "$(uname -s)" != Darwin ] || need_cef_build_tools
    ensure_pg_archive
    write_desktop_app_crate
    local app=$ROOT/crates/moqui-desktop
    if [ ! -f "$app/Cargo.lock" ] || [ "$ROOT/Cargo.lock" -nt "$app/Cargo.lock" ]; then
        run_cmd cp "$ROOT/Cargo.lock" "$app/Cargo.lock" || die "could not copy Cargo.lock"
        # The workspace's lock has none of the app's own crates (Tauri, CEF); resolve them now, since a build stages the
        # notices from the cef crate the lock names before cargo tauri runs.
        (cd "$app" && run_cmd cargo metadata --format-version 1 >/dev/null) || die "could not resolve $(rel "$app")/Cargo.lock"
    fi
    local -a tauri_args
    tauri_args=()
    if [ "$command" = dev ] && [ -n "$release" ]; then
        tauri_args+=(--release)
    fi
    if [ "$command" = build ]; then
        stage_desktop_runtime "$ROOT/target/desktop-runtime"
        stage_client_shell "$ROOT/target/desktop-shell"
        stage_desktop_sidecar "$ROOT/target/desktop-bin"
        stage_desktop_notices "$app/Cargo.lock" "$ROOT/target/desktop-notices"
        tauri_args+=(--config '{"bundle":{"resources":{"../../target/desktop-runtime/":"runtime/","../../target/desktop-shell/":"shell-assets/","../../target/desktop-notices/":"notices/"},"externalBin":["../../target/desktop-bin/moqui"]}}')
        local updater
        updater=$(desktop_updater_config) || exit 1
        [ -z "$updater" ] || tauri_args+=(--config "$updater")
        desktop_signing_info
    fi
    # the CEF archive a build takes, by its pinned SHA-256: one there now, and one the build downloads (after it)
    check_cef_archives "$app/Cargo.lock"
    # The repository's target directory, so the framework crates are built once for the server and the app.
    export_var CARGO_TARGET_DIR "$ROOT/target"
    if [ "$command" = build ] && [ "$(uname -s)" = Linux ]; then
        # The Linux bundles copy libEGL.so and libGLESv2.so from the CEF distribution, which no longer has them, so
        # the bundle step runs on a staged distribution (stage_desktop_cef). The build keeps the CEF_PATH it has, so
        # cef-dll-sys is not rebuilt between dev and build runs.
        (cd "$app" && run_cmd cargo tauri build --no-bundle ${tauri_args[@]+"${tauri_args[@]}"} \
            -- ${cargo_args[@]+"${cargo_args[@]}"}) || die "cargo tauri build failed"
        local cef_path
        cef_path=$(stage_desktop_cef "$app/Cargo.lock" "$ROOT/target/desktop-cef") || exit 1
        # The AppImage is made by quick-sharun.sh, which tauri downloads from a fork that still fetches two helper
        # sources Anylinux-AppImages has since removed: they come from the last commit that has them. Its strace mode
        # would run the app (and so a node and its Postgres) for a few seconds to see what it loads; the
        # DEPLOY_CHROMIUM rules cover the libraries without that.
        (cd "$app" && CEF_PATH=$cef_path STRACE_MODE=0 \
            ANYLINUX_LIB_SOURCE=$ANYLINUX_APPIMAGES_LIB/anylinux.c \
            GTK_CLASS_FIX_SOURCE=$ANYLINUX_APPIMAGES_LIB/gtk-class-fix.c \
            run_cmd cargo tauri bundle ${tauri_args[@]+"${tauri_args[@]}"}) \
            || die "cargo tauri bundle failed"
        remake_appimage_without_namespaces_hook "$ROOT/target/release/bundle/appimage"
        check_cef_archives "$app/Cargo.lock"
        return
    fi
    (cd "$app" && run_cmd cargo tauri "$command" ${tauri_args[@]+"${tauri_args[@]}"} -- ${cargo_args[@]+"${cargo_args[@]}"}) \
        || die "cargo tauri $command failed"
    check_cef_archives "$app/Cargo.lock"
}

# The SHA-256 of each CEF archive the desktop app was built from, by its file name (plans/Security-Review.md
# SEC-CLI-104): download-cef checks an archive only against a SHA-1 from the index of the CDN it downloads it from, so the
# repository pins it. An archive with no entry yet (a new CEF version, another platform) is recorded on its first
# build here, for the developer to commit; one whose hash differs stops the build.
CEF_ARCHIVE_SUMS=crates/moqui-desktop/cef-archives.sha256

# The CEF distributions' directory: CEF_PATH, else tauri-cef's cache.
cef_distribution_dir() {
    if [ -n "${CEF_PATH:-}" ]; then
        printf '%s\n' "$CEF_PATH"
    elif [ "$(uname -s)" = Darwin ]; then
        printf '%s\n' "$HOME/Library/Caches/tauri-cef"
    else
        printf '%s\n' "${XDG_CACHE_HOME:-$HOME/.cache}/tauri-cef"
    fi
}

# Each CEF archive of the cef crate the app's Cargo.lock ($1) names, in the distributions' directory, against
# CEF_ARCHIVE_SUMS.
check_cef_archives() {
    local lock=$1 version dir archive name sum pinned
    [ -f "$lock" ] || return 0
    version=$(awk '$0 == "name = \"cef\"" { getline; sub(/.*\+/, ""); sub(/".*/, ""); print; exit }' "$lock")
    [ -n "$version" ] || return 0
    dir=$(cef_distribution_dir)/$version
    for archive in "$dir"/cef_binary_*.tar.bz2; do
        [ -f "$archive" ] || continue
        name=$(basename "$archive")
        sum=$(sha256_line "$archive" | awk '{print $1}')
        pinned=$(awk -v name="$name" '$2 == name { print $1 }' "$ROOT/$CEF_ARCHIVE_SUMS" 2>/dev/null)
        if [ -z "$pinned" ]; then
            if ! dry_run; then
                printf '%s  %s\n' "$sum" "$name" >>"$ROOT/$CEF_ARCHIVE_SUMS" || die "could not write $CEF_ARCHIVE_SUMS"
            fi
            warn "$name has no pinned SHA-256: $sum is recorded in $CEF_ARCHIVE_SUMS; commit it"
        elif [ "$pinned" != "$sum" ]; then
            die "$name is not the CEF archive $CEF_ARCHIVE_SUMS pins (SHA-256 $sum, pinned $pinned): delete it and its \
extracted directory from $dir, and build again"
        fi
    done
}

# target/moqui-app, the crate that links the framework and the component crates, which the desktop app depends on: the
# component crates as build_binary finds them; the generated crate is written even when there are none.
write_desktop_app_crate() {
    local -a addons
    local addon
    addons=()
    while IFS= read -r addon; do addons+=("$addon"); done \
        < <(component_crate_dirs "$RUNTIME_DIR/$ADDON_SET" "$RUNTIME_DIR/$TEST_SET")
    while IFS= read -r addon; do
        framework_dev_crate "$addon" || { addons+=("$addon"); }
    done < <(component_crate_dirs "$ROOT/runtime/$DEV_SET")
    write_app_crate "$ROOT/target/moqui-app" ${addons[@]+"${addons[@]}"}
}

# The emulator's AVD and the API of the system image it is made from (with Play services, for FCM).
ANDROID_AVD_DEFAULT=moqui-36
ANDROID_AVD_API=36

task_android() {
    local command= release= window=
    local -a tauri_args
    tauri_args=()
    while [ $# -gt 0 ]; do
        case $1 in
            --release) release=1 ;;
            --window) window=1 ;;
            --)
                shift
                tauri_args+=("$@")
                break
                ;;
            -*) tauri_args+=("$1") ;;
            *)
                if [ -z "$command" ]; then
                    case $1 in
                        emulator | init | dev | build) command=$1 ;;
                        *) die "unknown android command '$1': emulator, init, dev or build (./moq help android)" ;;
                    esac
                else
                    tauri_args+=("$1")
                fi
                ;;
        esac
        shift
    done
    command=${command:-dev}
    case $command in emulator | init) [ -z "$release" ] || warn "--release is ignored for $command" ;; esac
    [ "$command" = emulator ] || [ -z "$window" ] || warn "--window is ignored for $command"
    # The SDK, the JDK, and for a build the NDK and its compilers; the emulator needs only the SDK.
    local ANDROID_ENV_NO_NDK=
    [ "$command" != emulator ] || ANDROID_ENV_NO_NDK=1
    # shellcheck source=scripts/android-env.sh
    . "$ROOT/scripts/android-env.sh" || exit 1
    [ -n "${ANDROID_HOME:-}" ] \
        || die "no Android SDK: set ANDROID_HOME, or install one in ~/Android/Sdk (see ./moq help android)"
    if [ "$command" = emulator ]; then
        android_emulator "$window"
        return
    fi
    need_tauri_cli
    [ -n "${JAVA_HOME:-}" ] || die "no JDK: set JAVA_HOME, or put a JDK's javac on the PATH"
    local app=$ROOT/crates/moqui-mobile
    # its committed lock, with the repository's versions of the crates they share (./moq update-app-locks)
    seed_app_lock crates/moqui-mobile
    export_var CARGO_TARGET_DIR "$ROOT/target"
    # The app's wry is its patched copy, whose IPC is the main frame's only (crates/moqui-mobile/patches/README.md): a
    # Tauri or wry update that the patch no longer applies to fails here, never builds with the unpatched wry.
    case $command in dev | build) android_wry_patched "$app" ;; esac
    # The shell the app embeds (its frontendDist), from the same files as the desktop's.
    [ "$command" = init ] || stage_client_shell "$ROOT/target/mobile-shell"
    if [ "$command" = dev ]; then
        local adb=$ANDROID_HOME/platform-tools/adb serial
        serial=$(android_emulator_serial "$adb")
        # cargo-mobile2 would wait for the boot animation forever (android_emulator).
        if [ -n "$serial" ] && [ -z "$("$adb" -s "$serial" shell getprop init.svc.bootanim 2>/dev/null | tr -d '\r')" ]; then
            die "the emulator $serial has no boot animation (started with -no-window or -no-boot-anim), so" \
                "cargo tauri android dev would wait for it forever: adb -s $serial emu kill, then ./moq android emulator"
        fi
    fi
    local -a args
    args=()
    case $command in
        init) args=(--ci) ;;
        dev) [ -z "$release" ] || args=(--release) ;;
        build) [ -n "$release" ] || args=(--debug) ;;
    esac
    if [ "$command" != init ]; then
        # The version name, and the version code made from it (major*1000000 + minor*1000 + patch), are the crate's.
        local version
        version=$(android_app_version "$app/Cargo.toml") || exit 1
        # the version code has no place for a prerelease: 0.2.0-beta.1 would have 0.2.0's, which Play then refuses
        case $version in
            *[-+]*) [ "$command" != build ] || [ -z "$release" ] \
                || die "a release build needs a plain version (major.minor.patch), not $version: its version code" \
                    "would be the same as the release's" ;;
        esac
        args+=(--config "{\"version\":\"$version\"}")
        info "The app's version: $version"
    fi
    if [ "$command" = build ] && [ -n "$release" ]; then
        android_signing_info || exit 1
        # no Gradle daemon left running with the signing passwords in its environment
        export GRADLE_OPTS="${GRADLE_OPTS:+$GRADLE_OPTS }-Dorg.gradle.daemon=false"
    fi
    (cd "$app" && run_cmd cargo tauri android "$command" ${args[@]+"${args[@]}"} \
        ${tauri_args[@]+"${tauri_args[@]}"}) || die "cargo tauri android $command failed"
}

# Dies unless every wry in the mobile app's dependency tree ($1, its crate directory) is its patched copy
# (patches/wry, through [patch.crates-io]): cargo builds a wry the patch no longer matches from crates.io, with a warning only.
android_wry_patched() {
    local tree line
    dry_run && return 0
    tree=$(cd "$1" && cargo tree -i wry --target all -e normal --depth 0 2>/dev/null) \
        || die "cargo tree -i wry failed in $(rel "$1"): is wry still a dependency of the app?"
    [ -n "$tree" ] || die "no wry in the dependency tree of $(rel "$1")"
    while IFS= read -r line; do
        case $line in
            "wry v"*"/patches/wry)"*) ;;
            "wry v"*) die "the mobile app's wry is not its patched copy ($line): port patches/wry to it" \
                "(crates/moqui-mobile/patches/README.md)" ;;
        esac
    done <<<"$tree"
}

# The version of the crate whose Cargo.toml is $1 ([package] version), which the Android app's version name and code
# come from.
android_app_version() {
    local version
    version=$(sed -n '/^\[package\]/,/^\[/s/^version *= *"\([^"]*\)".*/\1/p' "$1" | head -1)
    case $version in
        [0-9]*.[0-9]*.[0-9]*) printf '%s\n' "$version" ;;
        *) die "no version in the [package] of $(rel "$1")" ;;
    esac
}

# Say how a release build of the Android app is signed, and check the set of variables, which Gradle reads
# (gen/android/app/build.gradle.kts): MOQUI_ANDROID_KEYSTORE (the upload key's .jks or .p12),
# MOQUI_ANDROID_KEYSTORE_PASSWORD, MOQUI_ANDROID_KEY_ALIAS and MOQUI_ANDROID_KEY_PASSWORD (the store's password when
# not set), the passwords also from files (MOQUI_ANDROID_KEYSTORE_PASSWORD_FILE, MOQUI_ANDROID_KEY_PASSWORD_FILE: the
# first line). Without MOQUI_ANDROID_KEYSTORE the APK and AAB are unsigned.
android_signing_info() {
    local keystore=${MOQUI_ANDROID_KEYSTORE:-} name file line
    if [ -z "$keystore" ]; then
        warn "The release build is not signed: MOQUI_ANDROID_KEYSTORE is not set. An unsigned APK does not install" \
            "and Play takes only a signed AAB; set MOQUI_ANDROID_KEYSTORE, MOQUI_ANDROID_KEYSTORE_PASSWORD," \
            "MOQUI_ANDROID_KEY_ALIAS and MOQUI_ANDROID_KEY_PASSWORD (./moq help android)"
        return 0
    fi
    case $keystore in
        /*) ;;
        *) die "MOQUI_ANDROID_KEYSTORE must be an absolute path (Gradle reads it from the app's project): $keystore" ;;
    esac
    [ -f "$keystore" ] || die "MOQUI_ANDROID_KEYSTORE is not a file: $keystore"
    for name in MOQUI_ANDROID_KEYSTORE_PASSWORD MOQUI_ANDROID_KEY_PASSWORD; do
        file=${name}_FILE
        file=${!file:-}
        [ -n "$file" ] || continue
        [ -f "$file" ] || die "${name}_FILE is not a file: $file"
        line=
        IFS= read -r line <"$file" || [ -n "$line" ] || die "${name}_FILE is empty: $file"
        printf -v "$name" '%s' "$line"
    done
    [ -n "${MOQUI_ANDROID_KEYSTORE_PASSWORD:-}" ] && [ -n "${MOQUI_ANDROID_KEY_ALIAS:-}" ] \
        || die "signing with MOQUI_ANDROID_KEYSTORE needs MOQUI_ANDROID_KEYSTORE_PASSWORD and MOQUI_ANDROID_KEY_ALIAS" \
            "(and MOQUI_ANDROID_KEY_PASSWORD when the key's password is not the store's)"
    export MOQUI_ANDROID_KEYSTORE MOQUI_ANDROID_KEYSTORE_PASSWORD MOQUI_ANDROID_KEY_ALIAS
    [ -z "${MOQUI_ANDROID_KEY_PASSWORD:-}" ] || export MOQUI_ANDROID_KEY_PASSWORD
    info "Signing the release build with the key $MOQUI_ANDROID_KEY_ALIAS of $(rel "$keystore")"
}

# The serial of the first emulator adb lists (in any state), or nothing. $1 is adb.
android_emulator_serial() {
    "$1" devices 2>/dev/null | sed -n 's/^\(emulator-[0-9]*\)[[:space:]].*/\1/p' | head -1
}

# Starts the AVD (MOQUI_ANDROID_AVD, default moqui-36), in a hidden window unless $1, and waits for it to boot; creates
# it from the installed system image when missing. An emulator already running is used as it is. Every adb call names
# the emulator's serial, so a phone connected too does not get in the way.
android_emulator() {
    local window=$1 avd=${MOQUI_ANDROID_AVD:-$ANDROID_AVD_DEFAULT}
    local emulator=$ANDROID_HOME/emulator/emulator adb=$ANDROID_HOME/platform-tools/adb
    local avdmanager=$ANDROID_HOME/cmdline-tools/latest/bin/avdmanager
    local log=$ROOT/target/android-emulator.log
    [ -x "$emulator" ] || die "no emulator in $ANDROID_HOME: sdkmanager emulator"
    [ -x "$adb" ] || die "no adb in $ANDROID_HOME: sdkmanager platform-tools"
    if ! "$emulator" -list-avds 2>/dev/null | grep -qxF "$avd"; then
        local abi=x86_64
        [ "$(uname -m)" = x86_64 ] || abi=arm64-v8a
        local image="system-images;android-$ANDROID_AVD_API;google_apis_playstore;$abi"
        [ -d "$ANDROID_HOME/system-images/android-$ANDROID_AVD_API/google_apis_playstore/$abi" ] \
            || die "no system image for the AVD: sdkmanager '$image'"
        [ -x "$avdmanager" ] || die "no avdmanager in $ANDROID_HOME: sdkmanager 'cmdline-tools;latest'"
        info "Creating the AVD $avd from $image"
        echo no | run_cmd "$avdmanager" create avd -n "$avd" -k "$image" -d pixel_7 >/dev/null \
            || die "could not create the AVD $avd"
    fi
    local serial pid=
    serial=$(android_emulator_serial "$adb")
    if [ -n "$serial" ]; then
        info "An emulator is already running ($serial)"
    else
        # The first free console port, so the serial is known: emulator-<port>.
        local port=5554
        while "$adb" devices 2>/dev/null | grep -q "^emulator-$port[[:space:]]"; do port=$((port + 2)); done
        serial=emulator-$port
        local -a emulator_args
        # cargo tauri android dev (cargo-mobile2) waits for an emulator's init.svc.bootanim to be "stopped", which it
        # never is without the boot animation, and -no-window (like -no-boot-anim) turns that off. So headless is a
        # hidden window where there is a display, and -no-window only where there is none.
        emulator_args=(-avd "$avd" -port "$port" -no-audio)
        if [ -z "$window" ]; then
            if android_has_display; then
                emulator_args+=(-qt-hide-window)
            else
                emulator_args+=(-no-window)
                warn "no display: the emulator runs with -no-window, where ./moq android dev cannot install the app" \
                    "(use ./moq android build --target x86_64 --apk and adb install)"
            fi
        fi
        if dry_run; then
            show_cmd "$emulator" "${emulator_args[@]}"
            return 0
        fi
        mkdir -p "$ROOT/target"
        info "Starting $avd as $serial (its output in $(rel "$log"))"
        nohup "$emulator" "${emulator_args[@]}" >"$log" 2>&1 &
        pid=$!
    fi
    # Not adb wait-for-device, which waits forever when the emulator dies: its state and boot polled in one budget.
    local start=$SECONDS
    until [ "$("$adb" -s "$serial" get-state 2>/dev/null)" = device ] \
        && [ "$("$adb" -s "$serial" shell getprop sys.boot_completed 2>/dev/null | tr -d '\r')" = 1 ]; do
        if [ -n "$pid" ] && ! kill -0 "$pid" 2>/dev/null; then
            die "the emulator exited before it booted: see $(rel "$log")"
        fi
        [ $((SECONDS - start)) -lt 300 ] || die "the emulator did not boot in 300 seconds: see $(rel "$log")"
        sleep 2
    done
    info "The emulator is up ($serial); stop it with: adb -s $serial emu kill"
}

# Whether the emulator can open a window: a GUI session on macOS, an X or Wayland display elsewhere.
android_has_display() {
    if [ "$(uname -s)" = Darwin ]; then
        [ "$(launchctl managername 2>/dev/null)" = Aqua ]
    else
        [ -n "${DISPLAY:-}${WAYLAND_DISPLAY:-}" ]
    fi
}

# The --config of a bundle with an update feed, as JSON: the updater plugin's public key and endpoints, and the
# updater artifacts. MOQUI_DESKTOP_UPDATER_PUBKEY is the public key of `cargo tauri signer generate` (the .pub file's
# text, or the file) and MOQUI_DESKTOP_UPDATER_ENDPOINT the feed's HTTPS URLs, separated by spaces, which may use
# Tauri's {{target}}, {{arch}} and {{current_version}}. The Tauri CLI signs the artifacts with
# TAURI_SIGNING_PRIVATE_KEY (the key, or its file) and TAURI_SIGNING_PRIVATE_KEY_PASSWORD. Prints nothing when neither
# variable is set: the app is built without updates (crates/moqui-desktop/src/updates.rs).
desktop_updater_config() {
    local pubkey=${MOQUI_DESKTOP_UPDATER_PUBKEY:-} endpoint list=
    local -a endpoints
    read -r -a endpoints <<<"${MOQUI_DESKTOP_UPDATER_ENDPOINT:-}"
    [ -z "$pubkey" ] && [ ${#endpoints[@]} -eq 0 ] && return 0
    [ -n "$pubkey" ] && [ ${#endpoints[@]} -gt 0 ] \
        || die "set both MOQUI_DESKTOP_UPDATER_PUBKEY and MOQUI_DESKTOP_UPDATER_ENDPOINT for updates, or neither"
    [ -n "${TAURI_SIGNING_PRIVATE_KEY:-}" ] \
        || die "updates need TAURI_SIGNING_PRIVATE_KEY (and TAURI_SIGNING_PRIVATE_KEY_PASSWORD) to sign the artifacts"
    if [ -f "$pubkey" ]; then
        pubkey=$(cat -- "$pubkey") || die "could not read the updater public key $pubkey"
    fi
    pubkey=$(printf '%s' "$pubkey" | tr -d '[:space:]')
    case $pubkey in
        '' | *[!A-Za-z0-9+/=]*) die "MOQUI_DESKTOP_UPDATER_PUBKEY is not a public key of cargo tauri signer generate (base64)" ;;
    esac
    for endpoint in "${endpoints[@]}"; do
        case $endpoint in
            *'"'* | *'\'*) die "the updater endpoint $endpoint has a quote or backslash" ;;
            https://?*) ;;
            *) die "the updater endpoint $endpoint is not an https:// URL; the updater refuses another scheme" ;;
        esac
        list="${list:+$list,}\"$endpoint\""
    done
    # requireSignedVersion: an update's signature must name its version, so an older release signed with the same key is
    # never taken as a newer one (plans/Security-Review.md SEC-CLI-98); the bundle signs with the app's version
    printf '{"bundle":{"createUpdaterArtifacts":true},"plugins":{"updater":{"pubkey":"%s","endpoints":[%s],"requireSignedVersion":true}}}\n' \
        "$pubkey" "$list"
}

# Say how a macOS bundle is signed. The Tauri CLI reads the variables itself: APPLE_SIGNING_IDENTITY (or
# APPLE_CERTIFICATE and APPLE_CERTIFICATE_PASSWORD, a base64 .p12 for a keychain of its own) signs the app, the moqui
# sidecar, the CEF helpers and framework with the hardened runtime and entitlements.plist; APPLE_ID, APPLE_PASSWORD
# and APPLE_TEAM_ID, or APPLE_API_KEY and APPLE_API_ISSUER (and APPLE_API_KEY_PATH), notarize the signed app.
desktop_signing_info() {
    [ "$(uname -s)" = Darwin ] || return 0
    if [ -z "${APPLE_SIGNING_IDENTITY:-}${APPLE_CERTIFICATE:-}" ]; then
        info "The app is not signed: APPLE_SIGNING_IDENTITY is not set"
        return 0
    fi
    if [ -n "${APPLE_ID:-}" ]; then
        [ -n "${APPLE_PASSWORD:-}" ] && [ -n "${APPLE_TEAM_ID:-}" ] \
            || die "notarizing with APPLE_ID needs APPLE_PASSWORD (an app-specific password) and APPLE_TEAM_ID"
        info "Signing and notarizing the app (APPLE_ID)"
    elif [ -n "${APPLE_API_KEY:-}" ]; then
        [ -n "${APPLE_API_ISSUER:-}" ] || die "notarizing with APPLE_API_KEY needs APPLE_API_ISSUER"
        info "Signing and notarizing the app (APPLE_API_KEY)"
    else
        info "Signing the app; it is not notarized: neither APPLE_ID nor APPLE_API_KEY is set"
    fi
}

# Remake the AppImage in $1 from the AppDir the bundler leaves beside it, without fix-namespaces.hook. Tauri adds that
# hook to every CEF AppImage: where AppArmor restricts unprivileged user namespaces it asks on each start to add an
# AppArmor profile, and it cannot (its pkexec script breaks on a path with a space and it does not check the result).
# The profile would not help anyway: tauri-runtime-cef runs an AppImage with --no-sandbox whenever
# kernel.apparmor_restrict_unprivileged_userns is 1, whatever profiles there are. The .deb keeps Chromium's sandbox
# (a setuid chrome-sandbox). quick-sharun.sh --make-appimage is the bundler's own last step.
remake_appimage_without_namespaces_hook() {
    local dir=$1 appdir image quick_sharun
    quick_sharun=${XDG_CACHE_HOME:-$HOME/.cache}/tauri/quick-sharun.sh
    for appdir in "$dir"/*.AppDir; do
        [ -f "$appdir/bin/fix-namespaces.hook" ] || continue
        for image in "$dir"/*.AppImage; do
            [ -f "$image" ] || die "no AppImage beside $(rel "$appdir")"
            [ -f "$quick_sharun" ] || die "no $quick_sharun to remake $(rel "$image") with"
            # the script tauri downloaded, checked against the SHA-256 recorded the first time for this tauri-cli
            # (SEC-OUT-91)
            check_download_sum "$quick_sharun" "quick-sharun:$(cargo tauri --version 2>/dev/null | tr ' ' '-')"
            run_cmd rm -f -- "$appdir/bin/fix-namespaces.hook" "$image" || die "could not remove the namespaces hook"
            (cd "$dir" && APPDIR=$appdir OUTNAME=${image##*/} run_cmd sh "$quick_sharun" --make-appimage) \
                || die "could not remake $(rel "$image")"
            # The bundle step signed the image it made, for the updater: sign the remade one.
            if [ -f "$image.sig" ]; then resign_desktop_update "$image"; fi
        done
    done
}

# Sign the update artifact $1 again, as the bundle step did before it was remade, with the app's version in the
# signature: TAURI_SIGNING_PRIVATE_KEY is the key or its file, and TAURI_SIGNING_PRIVATE_KEY_PASSWORD its password.
resign_desktop_update() {
    local file=$1 version key=${TAURI_SIGNING_PRIVATE_KEY:-}
    version=$(sed -n 's/^  "version": "\(.*\)",$/\1/p' "$ROOT/crates/moqui-desktop/tauri.conf.json")
    [ -n "$version" ] || die "no version in crates/moqui-desktop/tauri.conf.json"
    [ -n "$key" ] || die "no TAURI_SIGNING_PRIVATE_KEY to sign $(rel "$file") again"
    run_cmd rm -f -- "$file.sig" || die "could not remove the signature of $(rel "$file")"
    if [ -f "$key" ]; then
        # The CLI's signer takes a key file only as TAURI_SIGNING_PRIVATE_KEY_PATH, and not with the key's variable.
        (unset TAURI_SIGNING_PRIVATE_KEY && export TAURI_SIGNING_PRIVATE_KEY_PATH=$key \
            && run_cmd cargo tauri signer sign --app-version "$version" "$file")
    else
        run_cmd cargo tauri signer sign --app-version "$version" "$file"
    fi || die "could not sign $(rel "$file") again"
}

# The CEF distribution the Linux bundles are made from, in $2, printing the CEF_PATH that resolves to it: links to the
# files of the one the build used (CEF_PATH, or the tauri-cef cache), the version the cef crate in the lock file $1
# names. tauri-bundler 3.0.0-alpha.4 copies and strips a fixed list of files that still has ANGLE's libEGL.so and
# libGLESv2.so, which CEF no longer ships or lists as redistributable on Linux; an empty shared library stands in for
# each one the distribution is missing.
stage_desktop_cef() {
    local lock=$1 stage=$2 version os_arch source dest file name
    version=$(awk '$0 == "name = \"cef\"" { getline; sub(/.*\+/, ""); sub(/".*/, ""); print; exit }' "$lock")
    [ -n "$version" ] || die "no cef crate in $(rel "$lock")"
    case $(uname -m) in
        x86_64) os_arch=cef_linux_x86_64 ;;
        aarch64 | arm64) os_arch=cef_linux_aarch64 ;;
        *) die "no CEF distribution for $(uname -m)" ;;
    esac
    source=${CEF_PATH:-${XDG_CACHE_HOME:-$HOME/.cache}/tauri-cef}
    [ -f "$source/$version/$os_arch/libcef.so" ] && source=$source/$version/$os_arch
    [ -f "$source/libcef.so" ] || die "no CEF $version distribution at $source"
    dest=$stage/$version/$os_arch
    run_cmd rm -rf -- "$stage" && run_cmd mkdir -p "$dest" || die "could not create $(rel "$dest")"
    for file in "$source"/*; do
        run_cmd ln -s "$file" "$dest/" || die "could not link $file"
    done
    need_tool cc
    for name in libEGL.so libGLESv2.so; do
        [ -e "$dest/$name" ] && continue
        run_cmd cc -shared -nostdlib -o "$dest/$name" -x c /dev/null || die "could not make the stand-in $name"
    done
    printf '%s\n' "$stage"
}

# The notices of what a bundled desktop app ships beside the framework, in $2 (plans/archive/License-Review.md, "Servo,
# Chromium, OpenSearch"): Chromium's, as CEF's distribution has them (CREDITS.html), since the app's window and its PDF
# engine are Chromium. The distribution is the cef crate's of the app's Cargo.lock ($1) in tauri-cef's cache, or
# CEF_PATH.
stage_desktop_notices() {
    local lock=$1 stage=$2 version source credits
    version=$(awk '$0 == "name = \"cef\"" { getline; sub(/.*\+/, ""); sub(/".*/, ""); print; exit }' "$lock")
    [ -n "$version" ] || die "no cef crate in $(rel "$lock")"
    source=$(cef_distribution_dir)
    credits=$(find "$source" -maxdepth 3 -name CREDITS.html -path "*$version*" 2>/dev/null | head -n 1)
    [ -n "$credits" ] || credits=$(find "$source" -maxdepth 1 -name CREDITS.html 2>/dev/null | head -n 1)
    [ -n "$credits" ] || die "no CREDITS.html of CEF $version under $source"
    run_cmd mkdir -p "$stage" || die "could not create $(rel "$stage")"
    run_cmd cp "$credits" "$stage/chromium-credits.html" || die "could not stage $credits"
}

# The moqui binary a bundled desktop app runs as its sidecar node (crates/moqui-desktop/src/node/sidecar.rs), in $1 as
# moqui-<host triple>, the name bundle.externalBin looks for; the bundle puts it beside the app's executable as moqui.
# It is the Maximal package's binary, the package the app links, built first so it is not stale.
stage_desktop_sidecar() {
    local stage=$1 bin triple
    bin=$(moqui_binary release) || exit 1
    triple=$(rustc -vV | sed -n 's/^host: //p')
    [ -n "$triple" ] || die "could not get the host target triple from rustc -vV"
    run_cmd mkdir -p "$stage" || die "could not create $(rel "$stage")"
    run_cmd cp "$bin" "$stage/moqui-$triple" || die "could not stage $(rel "$bin") for the bundle"
}

# The client's shell, in $1, in the layout moqui_client::assets reads (crates/moqui-client/src/assets.rs): the shell's
# page and scripts as shell/, and the webroot's client and the libraries the shell loads as webroot/js, webroot/css and
# webroot/libs/<id>. Both apps carry it: a desktop bundle in its resources (target/desktop-shell), the mobile app
# embedded as its Tauri assets (target/mobile-shell, its frontendDist).
stage_client_shell() {
    ensure_js
    js_report_fonts || return 1
    local stage=$1 webroot=$ROOT/runtime/$BASE_SET/webroot/screen/webroot lib
    need_tool rsync
    # the libraries of shell/libs.txt only: one dropped from the list goes from the stage too
    run_cmd rm -rf -- "${stage:?}/webroot/libs"
    run_cmd mkdir -p "$stage/webroot/libs" || die "could not create $(rel "$stage")"
    run_cmd rsync -a --delete --exclude '.DS_Store' --exclude bundled-screens.txt --exclude screens \
        "$ROOT/crates/moqui-desktop/shell/" "$stage/shell/" || die "could not stage the shell"
    # not the browser's vault module (js/vault): the apps run the vault natively (moqui_client::vault, V5), and the
    # shell's pages never load it
    for lib in js css; do
        run_cmd rsync -a --delete --delete-excluded --exclude '.DS_Store' --exclude '/vault/' "$webroot/$lib/" "$stage/webroot/$lib/" \
            || die "could not stage webroot/$lib"
    done
    local part files
    while IFS= read -r lib; do
        case $lib in '' | '#'*) continue ;; esac
        # "<lib> <part>": only the files the library's parts.json lists for that part (OpenLayers' main part,
        # plans/archive/Maps-Offline.md §2.5), else the whole library
        part=${lib#* }
        [ "$part" != "$lib" ] || part=
        lib=${lib%% *}
        if [ -n "$part" ]; then
            files=$(node -e 'const parts = JSON.parse(require("fs").readFileSync(process.argv[1], "utf8"));
                const list = parts[process.argv[2]];
                if (!Array.isArray(list) || !list.length || list.some((name) => typeof name !== "string" || name.split("/").some((seg) => !seg || seg === "." || seg === "..")))
                    process.exit(1);
                process.stdout.write(list.join("\n") + "\n");' "$webroot/libs/$lib/parts.json" "$part") \
                || die "webroot/libs/$lib/parts.json names no part $part"
            run_cmd rm -rf -- "${stage:?}/webroot/libs/$lib"
            run_cmd mkdir -p "$stage/webroot/libs/$lib" || die "could not create $(rel "$stage")"
            printf '%s' "$files" | run_cmd rsync -a --files-from=- "$webroot/libs/$lib/" "$stage/webroot/libs/$lib/" \
                || die "could not stage the $part part of webroot/libs/$lib"
        else
            run_cmd rsync -a --delete --exclude '.DS_Store' "$webroot/libs/$lib/" "$stage/webroot/libs/$lib/" \
                || die "could not stage webroot/libs/$lib"
        fi
    done < "$ROOT/crates/moqui-desktop/shell/libs.txt"
}

# The runtime a bundled desktop app carries in its resources, in $1: what ./moq archive --no-data puts in, less the
# binary and the data, with the production conf as MoquiConf.xml. The app links these from its runtime in the app data
# directory (crates/moqui-desktop/src/runtime_dir.rs), so only the read-only parts are staged.
stage_desktop_runtime() {
    local stage=$1 name
    need_tool rsync
    run_cmd mkdir -p "$stage" || die "could not create $(rel "$stage")"
    # components/dev: the Dev AIDE and the example, which load only on a dev instance (the tray's Developer tools). No
    # tests or plans, as in ./moq archive: the test set and each component's test/, plan/ and plans/.
    local -a no_tests
    for name in components conf; do
        no_tests=()
        [ "$name" != components ] || no_tests=(--exclude "/${TEST_SET#components/}/" --exclude '/*/*/test/'
            --exclude '/*/*/plan/' --exclude '/*/*/plans/')
        if [ -d "$RUNTIME_DIR/$name" ]; then
            run_cmd rsync -a --delete --exclude .git --exclude target --exclude '.DS_Store' ${no_tests[@]+"${no_tests[@]}"} \
                "$RUNTIME_DIR/$name/" "$stage/$name/" || die "could not stage runtime/$name"
        else
            run_cmd rm -rf -- "${stage:?}/$name"
        fi
    done
    run_cmd cp "$RUNTIME_DIR/$PRODUCTION_CONF" "$stage/MoquiConf.xml" || die "could not stage MoquiConf.xml"
}


# ========== help ==========

help_desktop() {
    cat <<'EOF'
Runs (dev, the default) or bundles (build) the desktop app in crates/moqui-desktop: Tauri 3 (alpha)
on Chromium (CEF). The node runs in the app's own process, or as the moqui binary in a child process
(the sidecar) with MOQUI_DESKTOP_NODE=sidecar or the tray's node item (plans/Mobile-Desktop.md,
configurations 1 and 2). With MOQUI_DESKTOP_NODE=client, or the tray's client item, there is no node:
the app is a client of a remote server, with its bundled shell in crates/moqui-desktop/shell
(configuration 3), whose files build stages in the bundle too. dev's sidecar is target/release/moqui, else target/debug/moqui (./moq build);
build puts the Maximal binary in the bundle. It first writes target/moqui-app, the crate that links
the framework and the component crates, which the app depends on. It needs the Tauri CLI of the same
version, from cargo:

    cargo install tauri-cli --version 3.0.0-alpha.6 --locked

The first build downloads CEF (about 1 GB) into the tauri-cef cache (CEF_PATH to use another).
On Windows it also downloads CMake and Ninja into %USERPROFILE%\.moqui when they are not on PATH
(MOQUI_CMAKE and MOQUI_NINJA to use another). The C++ compiler is still Visual Studio Build Tools.
dev runs on this repository's runtime (MOQUI_RUNTIME to use another); a bundled app keeps its
runtime in the app data directory. See crates/moqui-desktop/README.md.

build signs the bundles and adds updates when these are set; without them they are unsigned
and the app has no updates:

    APPLE_SIGNING_IDENTITY        macOS: sign the app, the moqui sidecar and the CEF helpers
                                  (or APPLE_CERTIFICATE and APPLE_CERTIFICATE_PASSWORD, a .p12)
    APPLE_ID, APPLE_PASSWORD, APPLE_TEAM_ID
                                  macOS: notarize the signed app (or APPLE_API_KEY,
                                  APPLE_API_ISSUER and APPLE_API_KEY_PATH)
    MOQUI_DESKTOP_UPDATER_PUBKEY    the update feed's public key (cargo tauri signer generate)
    MOQUI_DESKTOP_UPDATER_ENDPOINT  the feed's https:// URLs, separated by spaces
    TAURI_SIGNING_PRIVATE_KEY       the key that signs the updates, or its file; with
                                    TAURI_SIGNING_PRIVATE_KEY_PASSWORD

Options:
      --release        Optimized (build always is)
EOF
}

help_android() {
    cat <<'EOF'
The Android app in crates/moqui-mobile: Tauri on Android's WebView (the wry runtime), with the
client crate (plans/Mobile-Desktop.md, MD6). Commands:

    emulator   Start the emulator (the AVD in MOQUI_ANDROID_AVD, default moqui-36) and wait for
               it to boot: in a hidden window, or shown with --window (with no display,
               -no-window, where dev cannot install: it waits for a boot animation that
               never runs). Creates the AVD from the installed system image
               (system-images;android-36;google_apis_playstore;<arch>) when missing.
               It needs only the SDK. Stop it with: adb -s <its serial> emu kill
    init       cargo tauri android init: regenerates what gen/android does not keep in git
    dev        cargo tauri android dev, on the running emulator or a phone over adb (the default)
    build      cargo tauri android build: an APK and an AAB for each target, debug unless
               --release (e.g. ./moq android build --target x86_64 --apk)

dev and build stage the client's shell, which the app embeds, in target/mobile-shell first. The
app's version name is the crate's version (crates/moqui-mobile/Cargo.toml), and its version code
major*1000000 + minor*1000 + patch.

A release build is signed with the upload key from these variables, else it is unsigned (with a
warning: an unsigned APK does not install, and Play takes only a signed AAB):

    MOQUI_ANDROID_KEYSTORE           the keystore (.jks or .p12), an absolute path
    MOQUI_ANDROID_KEYSTORE_PASSWORD  its password
    MOQUI_ANDROID_KEY_ALIAS          the key's alias
    MOQUI_ANDROID_KEY_PASSWORD       the key's password, when it is not the store's

or the passwords' files (their first line): MOQUI_ANDROID_KEYSTORE_PASSWORD_FILE and
MOQUI_ANDROID_KEY_PASSWORD_FILE. A release build runs Gradle with no daemon, so none is left
running with the passwords. A prerelease version (0.2.0-beta.1) is refused for a release build:
its version code would be the release's.

Other arguments go to the Tauri CLI. The Android SDK is ANDROID_HOME, else ~/Android/Sdk or
~/Library/Android/sdk; the NDK the newest under its ndk/ (or ANDROID_NDK_HOME); the JDK JAVA_HOME,
else the javac on the PATH (scripts/android-env.sh). It needs the Tauri CLI of the desktop app
and the Rust targets:

    cargo install tauri-cli --version 3.0.0-alpha.6 --locked
    rustup target add x86_64-linux-android aarch64-linux-android

The Gradle project downloads Gradle, the Android Gradle plugin and the SDK platform it compiles
against on its first build. See crates/moqui-mobile/README.md.

Options:
      --release        Optimized, for dev and build
      --window         emulator: show the emulator's window
EOF
}
