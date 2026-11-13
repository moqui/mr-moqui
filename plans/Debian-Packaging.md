# Debian Packaging

Status 2026-11-06: research and plan, nothing built. It covers what Debian's rules ask of the server, how a package gets into the archive, and the tools involved. It also proposes a Debian build that manages its own cluster on Debian's `postgresql-18` binaries. Debian inclusion of the desktop app comes second (§7). Two Debian Developers are looking at a possible inclusion. The crate and library counts below were measured against the Debian sid and forky package indexes on 2026-11-06 and will drift.

## 1. Summary

There are four large pieces of work, and three smaller ones.

1. **Rust crates.** Debian builds Rust programs only from crates packaged in the archive as `librust-*-dev`, never from a vendored `cargo vendor` tree (§3). A Maximal build without the dev tools, on Linux, needs 657 crates from crates.io. Of those, 406 are in sid at the same or a newer semver-compatible version and 100 are in sid at an older version of the same series. 81 are in sid only at a different series, and 70 are not packaged at all. Each missing crate is a source package of its own that goes through NEW. This is the largest piece and is mostly upstream and Rust-team work, not packaging.
2. **The vendored web libraries.** `runtime/components/base/webroot/screen/webroot/libs` holds 21 MB of minified JavaScript and CSS, esbuild bundles, fonts, and a prebuilt WebAssembly binary (matrix-sdk-crypto). Debian needs the preferred form for modification of every file, and wants Debian's own copy used where one exists (§4).
3. **A system-Postgres build.** It would keep the embedded cluster management but run Debian's `postgresql-18` binaries in place of the theseus archive (§5). The code supports most of this already. It needs a feature split, a major-version upgrade path, and a decision about the cluster that installing `postgresql-18` creates.
4. **An FHS layout and a real source package.** That means the binary in `/usr`, read-only components in `/usr/share`, configuration in `/etc`, state in `/var/lib`, and a `debian/` directory built with debhelper and dh-cargo. It would reuse `systemd/moqui.service`, `systemd/moqui.sysusers` and the podman entrypoint's symlink layout (§6).

The smaller pieces:

5. **Upstream release hygiene.** Tagged, signed release tarballs, a version scheme, a man page, and a supported upgrade path for data. Debian stable users upgrade in place across releases, and the README still says the data model changes without migrations until 0.1.0 (§2).
6. **Privacy and downloads.** Resolved for shipped pages: the default fonts are vendored and no page asks a CDN or font host (§6.4). The binary still has download commands for Chromium and other tools, and Debian expects no fetches by default (§6.4).
7. **The Debian process.** An ITP bug, a Salsa repository, sponsored uploads by the two Debian Developers, ftpmaster review in NEW, then autopkgtest and testing migration (§8).

A `.deb` for our own apt repository does not have to wait for any of this. The same `debian/` directory can build one today from a vendored crate tree (§6.5), while the archive version follows as its crates land.

## 2. Upstream prerequisites

These are things Debian needs from us as upstream, before or alongside the packaging.

- **Releases.** A version scheme (the crate version `0.1.0` is the start). Release tarballs come from signed tags, and `debian/watch` finds them, with `debian/upstream/signing-key.asc` for uscan to verify them. Five repositories make up the apps: mr-moqui, mr-mantle, mr-marble, mr-boss and mr-pass. Each would be its own source package, or a component tarball of one (format 3.0 (quilt) allows `moqui_0.1.0.orig-mantle.tar.xz`). The framework package alone is a useful first upload: the Tools app, Comms and the system apps run without the others.
- **Upgrades.** Debian stable carries one version for about two years plus LTS. An upgrade between Debian releases replaces the package in place with the database kept. "Empty the database and load again" is not acceptable there. Inclusion in a stable release wants the 0.1.0 series and its migrations. Inclusion in unstable can come earlier, with a NEWS.Debian entry saying so.
- **Security fixes.** The Debian security team backports fixes to the version in stable. A project moving this fast should plan on giving them a fix commit, not only "upgrade to the latest". A rebuild after a fix in a statically linked crate is a binNMU of `moqui`, which dh-cargo's `Static-Built-Using` makes visible.
- **A man page.** lintian reports `no-manual-page` for `/usr/bin/moqui`. A `moqui.1`, from the `--help` text or written by hand, is enough.
- **Architectures.** buildds build `Architecture: any` on every release architecture, including i386, armhf (32-bit) and s390x (big-endian). A new package that fails on an architecture where it has never built does not block migration to testing. Starting with `Architecture: amd64 arm64 ppc64el riscv64`, or `any` with failures fixed as they come, are both fine.
- **Build without git.** `crates/moqui-core/build.rs` runs `git rev-parse` and already tolerates its failure. A Debian source tree has no `.git`. Nothing in the build should depend on the time or the path (reproducible builds, §8).

## 3. Rust crates

### 3.1 The rule

The Rust team's policy says: "Builds must use the packaged versions of crate sources". The build must not access the network, download sources, or check out code. Crates come from `librust-*-dev` packages, which together form a Cargo directory registry under `/usr/share/cargo/registry`. dh-cargo implements this, and Debian Policy 4.9 forbids network access in the build for every package. A few very large packages are accepted with vendored crates (rustc itself, Firefox), but the Rust team and the ftpmasters expect an ordinary application to use packaged crates. Our sponsors should be asked early whether they would consider vendoring at all; the plan below assumes not.

Each missing crate becomes a `rust-<crate>` source package, usually made with `debcargo` in the team's [debcargo-conf](https://salsa.debian.org/rust-team/debcargo-conf) repository and uploaded through NEW. The work is largely mechanical for small crates. It still takes review time from the Rust team and the ftpmasters, and it must be coordinated with the team (the debian-rust list, `#debian-rust` on OFTC).

### 3.2 Where we stand

These were measured with `cargo tree -e normal,build --target x86_64-unknown-linux-gnu` and matched against the `librust-<crate>-<semver>-dev` names and Provides in the sid amd64 `Packages` index of 2026-11-06. Dev-dependencies are left out because a Debian build can skip the tests that need them. Platform-only crates (windows-*, core-foundation and the like) do not count on Linux.

| Build | Crates | In sid, same or newer | In sid, older in the series | Other series only | Not packaged |
|---|---|---|---|---|---|
| Lite (`moqui-host`, `lite`) | 413 | 281 | 57 | 36 | 39 |
| Lite plus mail, git, image, xlsx | 537 | 335 | 96 | 61 | 45 |
| Maximal without `devtools` | 657 | 406 | 100 | 81 | 70 |
| Desktop app (`crates/moqui-desktop/Cargo.lock`, all targets) | 988 | 569 | 131 | 151 | 137 |

What each column means for the work:

- **Older in the series.** Often nothing: our `Cargo.toml` minimum may already accept Debian's version, and dh-cargo builds against what is installed. Otherwise a new upstream version is uploaded to the existing Debian package, which needs no NEW.
- **Other series only.** Either we move to Debian's series, or a versioned package such as `rust-rand-0.9` is added through NEW. Many of ours come from duplicate series in our own tree, such as `darling` 0.20 and 0.23, `getrandom` 0.2 and 0.3, `hashbrown` 0.15 and 0.17, and `phf` 0.14 where sid has 0.11.
- **Not packaged.** These are new source packages. The notable families are `oxc_*` (13 crates, the expression parser in `moqui-core`), `rquickjs*` (4, QuickJS), `postgresql_embedded` and its two crates, `huskarl*` (4, SSO), `str0m*`, `stun-*` and `turn-*` (calls), `mysql*` (3), `opentelemetry*` (3), `yrs` (co-editing), `revue` (the TUI, pinned `=3.12.0`), `imap` (an alpha), `lettre`, `mail-auth`, `rsipstack`, `rust_xlsxwriter`, `ipp` and `datamatrix`.

### 3.3 Embedded C code in crates

Debian Policy 4.13 says a package should not use a convenience copy of code that is in Debian as a library. Two of our `-sys` crates carry C sources:

- `rquickjs-sys` builds the QuickJS sources it vendors. Debian has `quickjs` 2025.04.26 (Bellard's). We should check whether rquickjs's copy is quickjs-ng and whether the Rust team would accept the embedded copy (some C engines are accepted when upstream intends them to be embedded) or want it unbundled.
- `aws-lc-sys` 0.46 and 0.47. sid has `rust-aws-lc-sys` 0.42.0+dfsg. Ours would need a new upload, and two versions in our tree is one too many.

### 3.4 What we can do upstream to shrink this

- **Add a Debian check to CI.** Run a script that matches `cargo tree` against sid's `Packages` (the one used for §3.2 is a few dozen lines), and report every new dependency, or new series, that is not in Debian. Prefer Debian's series when choosing between equivalent crates.
- **Avoid pre-release and exact-pinned crates in the default build.** `rsa` 0.10.0-rc.18, `pkcs1` 0.8.0-rc.4, `ssh-key` 0.7.0-rc.11, `imap` 3.0.0-alpha.15, and `revue = "=3.12.0"` are all hard to package and to keep current.
- **Remove duplicate series** where we control them.
- **Replace thin wrappers.** We use `postgresql_embedded` only to run `initdb`, `pg_ctl` and `createdb` from an install we already trust (`trust_installation_dir`). A small module of our own would drop three unpackaged crates and their dependencies.
- **Stage the features.** The first upload can be Lite plus the features whose crates are already in Debian. Later uploads turn on more features as their crates land. Package features already exist and missing ones fail cleanly (`<feature> is not built into this host`), so this needs no code changes, only a feature list in `debian/rules`.

## 4. Web libraries in the base component

`libs/refresh.mjs` fetches 25 libraries from npm, runs esbuild over some of them, and records each file in `MANIFEST.json`. Every file in a Debian source package must be in its preferred form for modification (DFSG). lintian flags minified files and prebuilt binaries in the source (`source-is-missing`, `source-contains-prebuilt-javascript-object`, `source-contains-prebuilt-wasm-binary`). ftpmasters reject on these. There are three accepted ways to handle each file:

1. **Use Debian's package.** Exclude the copy (`Files-Excluded` in `debian/copyright`, which repacks the tarball as `+dfsg`), add the package to Depends, and point to it with a symlink into `/usr/share/javascript` or `/usr/share/fonts`.
2. **Build it in the package build from source** in the source tree, or from Debian's `node-*` packages. Debian has esbuild 0.28.2, the same version as `MANIFEST.json`.
3. **Ship the unminified source** under `debian/missing-sources/` for a single-file library with no build step. This is the weakest option. Reviewers prefer 1 or 2.

| Library | Ours | Debian sid | Likely path |
|---|---|---|---|
| Vue | 3.5.44 | `node-vue` 3.5.43 | 1 (close enough, or 2) |
| xterm | 6.0.0 | `node-xterm` 5.5.0 | 1 after an update, or 2 |
| DOMPurify | 3.4.17 | `node-dompurify` 3.4.17 | 1 |
| marked | 18.1.1 | `node-marked` 18.1.1 | 1, or 2 for our UMD bundle |
| CodeMirror 6 | 6.43.14 (bundle) | `node-codemirror-*` | 2 from Debian's modules |
| Tiptap, ProseMirror, Yjs | 3.31.4 (bundle) | ProseMirror and `node-yjs` yes; Tiptap no | 2, with Tiptap packaged or in the source |
| highlight.js | 11.12.0 | `node-highlight.js` 10.7.3 | 2, or an update in Debian |
| Chart.js | 4.5.1 | `node-chart.js` 3.9.1 | needs a Debian update to 4 |
| Mermaid | 11.17.2 | `node-mermaid` 9.2.2 | needs a Debian update to 11 (large) |
| pdf.js | 6.4.299 | `libjs-pdf` 2.14.305 | needs a Debian update (large) |
| Font Awesome | 4.7.0 | `fonts-font-awesome` 7.3.0, `fonts-fork-awesome` | move to Fork Awesome (4.7-compatible) or to FA 7 |
| Material Icons | 1.13.14 | `fonts-material-design-icons-iconfont` | 1 |
| Quasar, Tabulator, Tom Select, rrweb, SortableJS, OpenUI lang-core, qz-tray, Material Symbols, world-atlas | various | not packaged (`libjs-sortable` is old) | package each, or 2 from sources in the tarball |
| matrix-sdk-crypto (WASM) | 18.9.0 | not packaged | build from its Rust source for wasm32 (its own crate tree), or make the feature optional |

This is the second largest piece. The cheapest first step is to make the libraries that only some screens use optional in the Debian build, such as Mermaid, pdf.js, the matrix crypto, and rrweb. Each screen that uses one would then load it only when present. Quasar and Vue are the core and cannot be optional.

## 5. Postgres from the Debian package

### 5.1 What the developers asked for

They asked for a build that keeps the embedded cluster management in the binary, but uses the binaries of Debian's `postgresql-18` package instead of the theseus archive. That is the Enterprise feature set plus embedded cluster management, without the archive. sid and forky both have `postgresql-18` 18.6-3, which matches our pinned 18.6.0, `postgresql-common` 293, and `postgresql-18-pgvector` 0.8.7.

### 5.2 What the code does now

`crates/moqui-drivers/src/embedded.rs` already runs the cluster from an install directory that it trusts and does not download into (`settings.installation_dir`, `trust_installation_dir = true`). The cluster listens on a Unix socket in its data directory with `listen_addresses` empty, so it does not collide with a system cluster on 5432. The archive comes in only through `include_bytes!` (`ARCHIVE`), `install()` (unpacking), `build.rs` (staging and the psql/readline check), and the libxml2 shim, which a Debian Postgres does not need.

### 5.3 Changes

- **Split the feature.** `embedded-postgres` would keep the cluster management. A new `bundled-postgres` feature would carry the archive, `build.rs` staging, `install()` and `xmlshim`. Lite and Maximal turn on both, and the Debian build turns on only `embedded-postgres`. Without `bundled-postgres`, the install directory comes from configuration with a build-time default: Debian's `/usr/lib/postgresql/18`, whose `bin/` has `initdb`, `pg_ctl` and `postgres`, and whose share directory is found by Postgres itself.
- **Find the major at run time.** `SHIPPED_MAJOR` and `SHIPPED_VERSION` come from the build today. In the system build they come from the install directory (`bin/postgres --version`, or the `18` in the path).
- **Major upgrades.** Debian moves to a new major in a later release, and `postgresql-common` leaves the old `postgresql-18` installed after a dist-upgrade. The code currently refuses a data directory of another major and does not run `pg_upgrade`. The Debian build needs one of these: Depends on `postgresql-18` exactly, changed in a deliberate upload that runs `pg_upgrade --link` from `/usr/lib/postgresql/18/bin` to the new major; or a `moqui db-upgrade` command that the postinst or the admin runs. The first is simpler and is what other Debian packages that pin a major do.
- **The main cluster.** Installing `postgresql-18` makes and starts the `18/main` cluster unless `create_main_cluster = false` in `/etc/postgresql-common/createcluster.conf`. That is harmless to us, since we use a socket in our own data directory, but it is a second postmaster that an admin may not want. Document it in README.Debian. Do not edit `postgresql-common`'s conffile from our maintainer scripts; Policy forbids changing another package's conffile.
- **The Debian alternative.** The usual way for a Debian web app is dbconfig-common (`dbconfig-pgsql`): a database and role on the system cluster, made at install with debconf questions. The system build can offer it as a second path through the existing external `postgres` datasource. The developers may ask why the package does not use it. The answer is the per-node clusters for entity, search and sim, `save-db` and the backups, which assume a cluster the node owns.
- **Extensions.** `pg_trgm`, `unaccent` and `pg_stat_statements` are in `postgresql-18`'s contrib. `vector` is `postgresql-18-pgvector`, which goes in Recommends, since search works without it.
- **Tests.** `moqui test --db fresh` makes scratch clusters. In a Debian build it can run them on the build-dependency `postgresql-18` as the unprivileged build user. Or `dh_auto_test` runs the `--db none` specs, and the database specs run as an autopkgtest.

### 5.4 Chromium

Chromium stays external and optional. It goes in `Suggests: chromium | chromium-headless-shell`; sid has both. `moqui_chromium_path` already finds an installed Chromium on the PATH. The Debian build should drop, or patch out, `chromium-download` and any other command that fetches executables (§6.4).

## 6. The Debian package

### 6.1 Layout

| Path | What | Notes |
|---|---|---|
| `/usr/bin/moqui` | the binary | |
| `/usr/share/moqui/components/base/` | the base components, read-only | the web libraries from §4 replaced by symlinks |
| `/usr/share/moqui/components/addon/` | components from other packages (mr-mantle and the rest) | one package each, later |
| `/etc/moqui/MoquiConf.xml` | the runtime conf, a conffile | from `conf/MoquiProductionConf.xml` |
| `/etc/moqui/environment` | optional environment for the unit | already in `systemd/moqui.service` |
| `/var/lib/moqui/` | the runtime directory: `db/`, `cert/`, `txlog/`, `install/` (unused), `.env` with the generated secrets (0600) | owned by `moqui` |
| `/var/log/moqui/` | logs, or journald only | |
| `/usr/lib/systemd/system/moqui.service` | the unit | `dh_installsystemd` |
| `/usr/lib/sysusers.d/moqui.conf` | the `moqui` system user | `systemd/moqui.sysusers` with home `/var/lib/moqui` |

The binary expects one writable runtime directory with `components/base/` in it. The podman entrypoint solves the same problem with symlinks from the image's runtime into the volume. The Debian package can do the reverse, with `/var/lib/moqui/components/base -> /usr/share/moqui/components/base` and the conf linked from `/etc`, made in the postinst or shipped as symlinks. That works with no code change. A cleaner way, later, is separate settings for the read-only component roots and the writable state (`MOQUI_RUNTIME` for state, plus a component path list), so `/usr` stays read-only without symlinks.

The `.env` file is generated state, not configuration, so it stays in `/var/lib` and is not a conffile. A purge removes `/var/lib/moqui` only after asking or with `dpkg --purge`, and README.Debian says that `.env` holds the key of the encrypted fields.

### 6.2 The unit

`systemd/moqui.service` already has `Type=notify`, `ExecReload`, and the hardening options (`ProtectSystem=strict`, `ProtectHome`, `PrivateTmp`, `NoNewPrivileges`). For Debian, the paths change to `/usr/bin/moqui`, `WorkingDirectory=/var/lib/moqui` and `ReadWritePaths=/var/lib/moqui /var/log/moqui`, or better `StateDirectory=moqui` and `LogsDirectory=moqui`. The `systemd/drop-in/postgres.conf` drop-in fits only the external-database setup. A Debian package usually does not start a server that has not been configured. We should decide whether to enable it on install (the debhelper default) or ship it disabled with a README.Debian step. A first start loads seed data into an empty database, which takes time but is safe.

### 6.3 Binary packages

- `moqui` (Architecture: any): the binary, the unit, the sysusers file, the man page. Depends: `${shlibs:Depends}`, `${misc:Depends}`, `postgresql-18`, `moqui-common (= ${source:Version})`. Recommends: `postgresql-18-pgvector`, `ca-certificates`. Suggests: `chromium | chromium-headless-shell`.
- `moqui-common` (Architecture: all): `/usr/share/moqui/components/base` and the symlinks into the Debian JavaScript and font packages.
- Later: `moqui-mantle`, `moqui-marble`, `moqui-boss` and `moqui-pass`, each Architecture: all, unless a component has native crates. Components with Rust crates are linked into the binary today, which a split package cannot do. Those need to be built into `moqui` itself, or wait for a plugin mechanism.

### 6.4 Privacy and downloads

- Resolved: shipped pages make no third-party requests by default. The qvt shell's default fonts (Atkinson Hyperlegible Next and Mono) are vendored in `webroot/libs`, Roboto is gone, and the Prism, Chart.js and jquery-validate CDN links were replaced by vendored libraries or removed. The remaining Google Fonts choices of `WebrootQuasarThemeData.xml` load only when a user picks one in Appearance, which labels them "(Google)" and says they come from Google's servers. A Debian build may still drop those `ScreenFontOption` rows, or point them at `fonts-*` packages.
- Commands that download executables or archives (`chromium-download`, `scripts/pg-payload.sh`, an OpenSearch launch from `runtime/opensearch`) should be left out of the Debian build or say what to install instead.
- No default network contact at startup: no update checks, no telemetry export, and no LLM provider calls without configuration. This needs an audit; the desktop updater is already off unless its variables are set.

### 6.5 Two tracks

Build the same `debian/` directory in two ways:

- **Archive.** dh-cargo against packaged crates, the staged features of §3.4, and the web libraries handled as in §4. This version is uploaded to Debian.
- **Project apt repository.** The same `debian/` directory with `cargo vendor` output added as an extra orig tarball, the full feature set, and the libraries as they are. It builds with `dpkg-buildpackage` in sbuild today, gives users a policy-shaped `.deb` with the FHS layout and unit now, and replaces `./moq archive` plus the manual `/opt/moqui` steps of [Run-Deploy](../docs/Run-Deploy.md). Lintian runs on it too, so the two tracks converge.

The Debian developers will likely want `debian/` kept in a Salsa repository (DEP-14 branches, git-buildpackage or dgit), not in our upstream tree. A `debian/` directory in the upstream tarball is replaced by the packaging under format 3.0 (quilt), and it confuses that workflow. Our own apt build can pull it from Salsa.

## 7. The desktop app (second priority)

### 7.1 What Debian expects of a desktop package

- A `.desktop` file that passes `desktop-file-validate`. Ours (`linux/main.desktop`) should list its URL schemes as `MimeType=x-scheme-handler/mailto;...`.
- AppStream metadata in `/usr/share/metainfo/org.moqui.desktop.metainfo.xml` that passes `appstreamcli validate`. Software centers list the app from it.
- Icons in `/usr/share/icons/hicolor/<size>/apps/`.
- No self-updater. Debian disables updaters in packaged apps; ours is off unless built with its variables.
- No setuid helper without a strong case. The Tauri `.deb` installs `chrome-sandbox` setuid root, which lintian reports (`elevated-privileges`) and reviewers question. Debian's own `chromium` uses user namespaces.
- No bundled copies of libraries that Debian has (Policy 4.13), and no prebuilt binaries in the source.

### 7.2 Why the current app cannot go in as it is

- **CEF.** The first build downloads the CEF binary distribution, about 1 GB, prebuilt. Debian does not package CEF. A Debian build may not download, and may not ship a prebuilt Chromium. The security team would also have to follow a second copy of Chromium, which gets a new major every four weeks in stable. In practice the Debian package of a CEF app ships without its CEF part; OBS Studio's browser source is the familiar case.
- **Tauri.** Tauri, wry, tao, the tauri plugins, tauri-runtime-cef and cef-dll-sys are not packaged, and Tauri 3 is alpha. The desktop lock file has 137 unpackaged crates and 151 at another series in sid, beyond the server's.
- **The bundle.** The tauri-bundler `.deb` has no source package. It carries the `moqui` binary inside (it should depend on the `moqui` package instead), and needs the libEGL and libGLESv2 workarounds.

### 7.3 Options

1. **Flathub, plus our own apt repository, now.** A Flatpak may bundle CEF, runs sandboxed, updates itself, and reaches every distribution. Most Chromium-based desktop apps on Linux ship this way. This is the recommendation while Tauri 3 is alpha.
2. **A Debian build on Debian's own `chromium`.** The shell process (tray, deep links, OS integration, the node in-process or as the sidecar) opens Debian's Chromium in app mode with its own profile directory. The server already drives an external Chromium for PDF. This removes CEF and most of the Tauri stack from the Debian build, at the cost of a different window integration than the CEF build.
3. **wry on Debian's WebKitGTK** (`libwebkitgtk-6.0-4`). This is Tauri's default Linux runtime. [Mobile-Desktop](Mobile-Desktop.md) §3 moved away from it for the WebRTC and rendering gaps on Linux. It is a fit only if calls in the desktop app can be given up there or WebKitGTK has caught up. That needs a check against the WebKitGTK in forky, and it still leaves the Tauri crates to package.

In each Debian option, `moqui-desktop` Depends on `moqui` for the node rather than carrying its own copy.

## 8. Process and tools

### 8.1 Steps

1. **ITP.** File an Intent To Package bug against the `wnpp` pseudo-package (`reportbug wnpp`). It gives the name, upstream URL, license and description, goes to debian-devel, and the first upload closes it (`Closes: #NNNNNN` in `debian/changelog`). Check that `moqui` is free as a source and binary name; it was not taken in sid on 2026-11-06.
2. **Salsa.** Create a repository on salsa.debian.org, either under the sponsors' account or a team such as the Rust team for the crates. Enable the Salsa CI pipeline, which runs a build, lintian, piuparts, reprotest, blhc and autopkgtest on every push.
3. **Package.** Write `debian/control`, `rules` (dh with `--buildsystem cargo`), `changelog` (`dch`), `copyright` (DEP-5), `watch`, `source/format` (`3.0 (quilt)`), `tests/control`, `upstream/metadata`, the unit and sysusers files, README.Debian and NEWS. Set `Standards-Version` to the current Policy, and use debhelper compat 13 or 14.
4. **Crates.** Package the missing crates in debcargo-conf with the Rust team, smallest dependencies first. Each is a NEW upload.
5. **Build cleanly.** Build in `sbuild` (or pbuilder) against unstable, in a chroot with no network. Run `lintian -EviIL +pedantic` on the `.changes`, and fix or justify every error and warning (an override needs a comment). Run `piuparts` for install, upgrade and purge, and `autopkgtest` for the tests in `debian/tests`.
6. **Sponsored upload.** We are not Debian Developers, so the two developers sponsor. We sign the `.dsc` and `.changes` with `debsign` (our OpenPGP key) and upload to [mentors.debian.net](https://mentors.debian.net) with `dput mentors`, or push to Salsa. The sponsor reviews, builds, signs with their own key, and uploads to unstable. After some sponsored uploads, the maintainer can apply for Debian Maintainer status and upload the package themselves.
7. **NEW.** Each new source or binary package name waits in the NEW queue for ftpmaster review, which can take days to months. They check `debian/copyright` against every file in the source, and the DFSG status of every file. That is where §4 matters and where most rejections come from. `licensecheck` and `decopy` help draft the copyright file; `deny.toml` and `plans/archive/License-Review.md` already record the crate licenses.
8. **Source-only upload.** Binaries a maintainer uploads (which NEW requires) do not migrate to testing. A source-only upload after acceptance lets the buildds build it.
9. **Testing.** The package migrates from unstable to testing after the age delay (2 to 5 days). It needs no release-critical bugs, its autopkgtests and those of its reverse dependencies passing, and no regression on built architectures. It reaches the next stable release if it is in testing at the freeze. Ubuntu syncs it into universe automatically.

### 8.2 Tools

| Tool | What it does here |
|---|---|
| `lintian` | Checks the source and binary packages against Policy, at the levels error, warning, info and pedantic. Run on every build; the Salsa CI and mentors.debian.net run it too |
| `debsign` (devscripts) | Signs the `.dsc` and `.changes` with an OpenPGP key. dput and the archive check the signature |
| `dput` / `dupload` | Uploads the signed `.changes` and its files to mentors or the archive |
| `sbuild`, `pbuilder` | Builds in a clean unstable chroot, as the buildds do |
| `debcargo` | Makes `rust-*` crate packages from crates.io in debcargo-conf |
| `dh-cargo` | The debhelper build system for Rust applications; adds `Static-Built-Using` |
| `piuparts` | Tests install, upgrade and purge, and that the purge leaves nothing behind |
| `autopkgtest` | Runs `debian/tests` against the installed package: here, start the unit on an empty `/var/lib/moqui`, wait for `/health`, run `moqui test --db none` |
| `reprotest` / [reproducible-builds.org](https://reproducible-builds.org) | Builds twice under varied conditions and compares; tests.reproducible-builds.org reports for every package |
| `uscan` | Finds and verifies new upstream tarballs from `debian/watch` |
| `licensecheck`, `decopy` | Help write `debian/copyright` |
| `desktop-file-validate`, `appstreamcli validate` | The desktop package's metadata (§7) |
| `git-buildpackage`, `dgit` | The Salsa workflow |
| `reportbug` | Files the ITP and later bugs |

## 9. Order of work

1. Ask the sponsors three questions: whether vendored crates are acceptable at all (§3.1), whether they prefer dbconfig-common to the embedded cluster (§5.3), and which team the package would sit in.
2. Add the Debian crate check to CI. Start removing pre-release, exact-pinned and duplicate-series crates (§3.4).
3. Split `embedded-postgres` from `bundled-postgres`, and run on `/usr/lib/postgresql/18` (§5.3).
4. Write the `debian/` directory on Salsa. Build the project-apt-repository track first (§6.5), with the FHS layout, unit and sysusers. This gives users a server `.deb` and finds the layout problems.
5. Fix the privacy items (§6.4) upstream.
6. Plan the web libraries (§4): what is optional in the Debian build, and what gets packaged or rebuilt.
7. File the ITP, start the crate uploads with the Rust team, and make the first archive upload with the staged features.
8. Desktop: Flathub first (§7.3); decide between options 2 and 3 when Debian inclusion of the desktop becomes a goal.

## Sources

- [Debian Policy, chapter 4](https://www.debian.org/doc/debian-policy/ch-source.html): 4.9 (no network access in the build), 4.13 (embedded code copies), 4.5 (copyright), 4.4 (changelog)
- [Debian Rust Team Book: packaging policy](https://rust-team.pages.debian.net/book/policy.html), [getting started](https://rust-team.pages.debian.net/book/getting-started.html), [packaging tools](https://rust-team.pages.debian.net/book/packaging-tools.html)
- [Debian JavaScript policy](https://wiki.debian.org/Javascript/Policy), [EmbeddedCopies](https://wiki.debian.org/EmbeddedCopies), and the [2025 debian-devel thread on embedded copies](https://www.mail-archive.com/debian-devel@lists.debian.org/msg386852.html)
- [Ubuntu's Rust vendoring rules](https://github.com/canonical/ubuntu-mir/blob/main/vendoring/Rust.md), which differ from Debian's
- [LWN: Debian discusses vendoring, again](https://lwn.net/Articles/842319/); [Debian 13 notes on statically linked Rust and Go](https://lwn.net/Articles/1047221/)
- [postgresql-18 in sid](https://packages.debian.org/ja/source/sid/postgresql-18); [postgresql-common 283 making 18 the default](https://postgrespro.com/list/id/E1v1k6A-004gm5-19@atalia.postgresql.org)
- [Tauri: Chromium Embedded Framework](https://v3.tauri.app/develop/cef/); [Debian bug #893448, a chromium-source package for CEF](https://bugs.debian.org/893448)
- [mentors.debian.net](https://mentors.debian.net), the Debian Developer's Reference, and the New Maintainers' Guide for the upload process
- The sid and forky amd64 `Packages` and sid `Sources` indexes from deb.debian.org, 2026-11-06, for every package and version in §3 to §7
