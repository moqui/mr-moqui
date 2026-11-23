# JS build: npm as a build tool, the browser libraries built rather than committed

Status 2026-11-23: steps 1 to 7 are built (2026-11-22 and 23), with a follow-up the same day from a review of those
commits. What remains is two Linux checks ([Remaining](#remaining-linux)) and the
[Dev AIDE future](#future-npm-in-dev-aide-for-components). The sections below describe
what was built. Where it differs from the first design: each build installs into a temporary tree and deletes it
(no persistent `target/js/<project>`), Mermaid is bundled from the lock instead of copied, and the optional Xiaolai face
is switched off by a guard in the whiteboard bundle (`globalThis.MoquiBoardCjk`), not by editing the editor's font
table at run time.

The locked migration reproduced all 321 original outputs and the original manifest. On macOS arm64 the installed
libraries occupied 394 MiB (274 packages) and the original whiteboard 285 MiB (311 packages). The build uses Node
24.22.3 and npm 11.19.1.

### Security updates after the migration

Both project audits now report zero vulnerabilities. The rebuilt Mermaid 11.17.2 includes DOMPurify 3.4.17 and
KaTeX 0.18.2. The previous distributed file included DOMPurify 3.4.12 and KaTeX 0.16.47. OpenUI now bundles
Zod 4.6.6. The browser editor uses Excalidraw 0.18.1 and its actual editor and converter inputs use Nano ID
3.3.20 and 5.1.16. Updating the editor removed its old nested converter, Mermaid 10.9.3, DOMPurify 3.1.6 and
UUID 9.0.1. The converter still uses the shell's Mermaid. Its installed parser tree resolves lodash-es 4.18.1.
The lock changes resolved with the ordinary 14-day cooldown.

The original lock and bundle findings are grouped by the version that resolves them. Parent packages in the audit inherit these findings.

| Package | Resolved version | Distribution | Advisory |
|---|---|---|---|
| @excalidraw/excalidraw | 0.18.1 | Editor bundle. The editor and converter use the shell Mermaid global. | [GHSA-39h7-pwv7-rc3x](https://github.com/advisories/GHSA-39h7-pwv7-rc3x) |
| @excalidraw/mermaid-to-excalidraw | 2.2.2 | The shipped converter already used 2.2.2. The editor update removed the affected nested converter from the install. | [GHSA-39h7-pwv7-rc3x](https://github.com/advisories/GHSA-39h7-pwv7-rc3x) |
| dompurify | 3.4.17 | Mermaid shipped its own 3.4.12 alongside the standalone 3.4.17. These advisories applied to the shipped copy. | [GHSA-55q2-fjhq-7xh7](https://github.com/advisories/GHSA-55q2-fjhq-7xh7) [GHSA-6688-9rhm-gjv2](https://github.com/advisories/GHSA-6688-9rhm-gjv2) [GHSA-c6gr-qvv4-5v3f](https://github.com/advisories/GHSA-c6gr-qvv4-5v3f) [GHSA-cg2g-xh64-v4q7](https://github.com/advisories/GHSA-cg2g-xh64-v4q7) [GHSA-mrv2-3pqr-j5jr](https://github.com/advisories/GHSA-mrv2-3pqr-j5jr) |
| dompurify | Removed 3.1.6 | The converter alias excluded this older nested install from the editor. Its lockfile carried these additional advisories. | [GHSA-39q2-94rc-95cp](https://github.com/advisories/GHSA-39q2-94rc-95cp) [GHSA-76mc-f452-cxcm](https://github.com/advisories/GHSA-76mc-f452-cxcm) [GHSA-c2j3-45gr-mqc4](https://github.com/advisories/GHSA-c2j3-45gr-mqc4) [GHSA-cj63-jhhr-wcxv](https://github.com/advisories/GHSA-cj63-jhhr-wcxv) [GHSA-cjmm-f4jc-qw8r](https://github.com/advisories/GHSA-cjmm-f4jc-qw8r) [GHSA-cmwh-pvxp-8882](https://github.com/advisories/GHSA-cmwh-pvxp-8882) [GHSA-crv5-9vww-q3g8](https://github.com/advisories/GHSA-crv5-9vww-q3g8) [GHSA-gvmj-g25r-r7wr](https://github.com/advisories/GHSA-gvmj-g25r-r7wr) [GHSA-h7mw-gpvr-xq4m](https://github.com/advisories/GHSA-h7mw-gpvr-xq4m) [GHSA-h8r8-wccr-v5f2](https://github.com/advisories/GHSA-h8r8-wccr-v5f2) [GHSA-hpcv-96wg-7vj8](https://github.com/advisories/GHSA-hpcv-96wg-7vj8) [GHSA-r47g-fvhr-h676](https://github.com/advisories/GHSA-r47g-fvhr-h676) [GHSA-rp9w-3fw7-7cwq](https://github.com/advisories/GHSA-rp9w-3fw7-7cwq) [GHSA-v2wj-7wpq-c8vv](https://github.com/advisories/GHSA-v2wj-7wpq-c8vv) [GHSA-v8jm-5vwx-cfxm](https://github.com/advisories/GHSA-v8jm-5vwx-cfxm) [GHSA-v9jr-rg53-9pgp](https://github.com/advisories/GHSA-v9jr-rg53-9pgp) [GHSA-vhxf-7vqr-mrjg](https://github.com/advisories/GHSA-vhxf-7vqr-mrjg) [GHSA-vxr8-fq34-vvx9](https://github.com/advisories/GHSA-vxr8-fq34-vvx9) [GHSA-x4vx-rjvf-j5p4](https://github.com/advisories/GHSA-x4vx-rjvf-j5p4) |
| katex | 0.18.2 | Previously served inside Mermaid at 0.16.47. | [GHSA-238p-pmpm-9mq7](https://github.com/advisories/GHSA-238p-pmpm-9mq7) |
| lodash-es | 4.18.1 | Installed by the parser. The whiteboard bundle input list contains no lodash-es. | [GHSA-r5fr-rjxr-66jc](https://github.com/advisories/GHSA-r5fr-rjxr-66jc) [GHSA-xxjr-mmjv-4gpg](https://github.com/advisories/GHSA-xxjr-mmjv-4gpg) |
| mermaid | 11.17.2 | The shell serves the 11.x bundle. The board alias excluded the affected 10.x copies, now removed. | [GHSA-2v8p-3f2j-5mp7](https://github.com/advisories/GHSA-2v8p-3f2j-5mp7) [GHSA-6m6c-36f7-fhxh](https://github.com/advisories/GHSA-6m6c-36f7-fhxh) [GHSA-6x64-9x62-f2gx](https://github.com/advisories/GHSA-6x64-9x62-f2gx) [GHSA-7rqq-prvp-x9jh](https://github.com/advisories/GHSA-7rqq-prvp-x9jh) [GHSA-87f9-hvmw-gh4p](https://github.com/advisories/GHSA-87f9-hvmw-gh4p) [GHSA-c4c3-pg64-4m4v](https://github.com/advisories/GHSA-c4c3-pg64-4m4v) [GHSA-ghcm-xqfw-q4vr](https://github.com/advisories/GHSA-ghcm-xqfw-q4vr) [GHSA-xcj9-5m2h-648r](https://github.com/advisories/GHSA-xcj9-5m2h-648r) |
| nanoid | 3.3.20 and 5.1.16 | Both branches ship in the editor and converter assets. | [GHSA-28wg-ghj8-5hjv](https://github.com/advisories/GHSA-28wg-ghj8-5hjv) [GHSA-2v37-7h3g-55p8](https://github.com/advisories/GHSA-2v37-7h3g-55p8) [GHSA-mwcw-c2x4-8c55](https://github.com/advisories/GHSA-mwcw-c2x4-8c55) [GHSA-xwg4-73v4-xw9w](https://github.com/advisories/GHSA-xwg4-73v4-xw9w) |
| uuid | Removed 9.0.1 | Old nested install only. No UUID package appeared in the whiteboard bundle inputs. | [GHSA-w5hq-g745-h8pq](https://github.com/advisories/GHSA-w5hq-g745-h8pq) |
| zod | 4.6.6 | OpenUI ships Zod code in the bundle used by the browser and QuickJS. | [GHSA-mrhx-c7xc-m3w5](https://github.com/advisories/GHSA-mrhx-c7xc-m3w5) |

## Why

MR Moqui is becoming an engine for hosting JavaScript. Screens, the qvt client, Assist and the whiteboard run in the
browser. Service scripts, the Vue template compiler (`libs/vue/compiler-dom.cjs.js`) and the OpenUI bundle run in
QuickJS on the server. npm is the tool that ecosystem builds with, so the build should use it openly. Today npm is a
maintainer-only tool hidden behind committed output.

What that committed output costs:

- **No view of the dependencies.** Dependabot sees only `build/whiteboard/package-lock.json`. The other 25 libraries
  are pinned in `refresh.mjs` and fetched with `npm pack`, so no lockfile names them. Copies bundled inside other
  packages, such as the DOMPurify 3.4.12 inside `mermaid.min.js` 11.17.2, are invisible to it.
- **Two paths.** A developer runs committed bytes. A maintainer runs `refresh.mjs --write`. Nothing on the everyday
  path checks that the two still agree.
- **Size.** `webroot/libs` is 41 MB in 325 files in every checkout. Excalidraw alone is 20 MB, of which 14 MB is
  fonts and 13 MB is one CJK handwriting face (Xiaolai). In history it is smaller: about 21 MiB of the 223 MiB pack,
  since the libraries have changed in few commits. Ignoring the output does not shrink that pack. It stops the growth
  that each later upgrade would add.

## Decision

- **npm is required to build.** Node and npm are build dependencies, like cargo, for every developer, for CI, and
  for the release build.
  - The build needs Node 22.13 or later with its bundled npm: 22 is what current Linux distributions carry, and 22.13
    is the oldest Node a locked package declares (pdfjs-dist). The build image pins Node 24.22.3, which ships npm
    11.19.1. Both npm projects commit lockfile version 3, and the outputs do not depend on the Node or npm version.
  - Production machines still do not need Node. The archive, an embedded binary and the desktop and mobile apps all
    carry the built libraries.
  - There is no separate prebuilt JS archive. An offline build comes from the source release, which carries the
    dependencies as sources, not built output (see [Source release](#source-release)).
- **`./moq` calls npm, not cargo.** No `build.rs` runs npm, for these reasons:
  - No crate compiles from the libraries. The server reads them from disk.
  - Build scripts also run under rust-analyzer, `cargo check` and the lints.
  - A build script should write only to `OUT_DIR`.
  - `cargo build --offline` and a Debian-style build must keep working.
- **What stays in git: the reviewed record, not the output.**
  - `MANIFEST.json` stays committed as the lock of what the build produces: each file's sha256, and each tarball's npm
    integrity and license.
  - `NOTICES.md` stays committed, so a license change shows in review.
  - `licenses/` stays committed, all four files. `refresh.mjs` downloads three of them today with `curl` (LGPL 2.1
    from gnu.org, OFL 1.1 from openfontlicense.org, Unicode 3.0 from unicode.org) and falls back to the committed
    copy. Only `update-js` downloads them from now on. `build-js` reaches no host but the npm registry.
  - The sources the build needs stay committed: the lockfiles, `refresh.mjs`, `patches/` and the whiteboard `build/`.
  - Everything else under `libs/` is gitignored, as `js/vault/moqui_vault.wasm` is now.
- **The build must reproduce the manifest.** `build-js` fails when an output does not match its recorded sha256, so
  every machine serves the reviewed bytes. The verify mode of `refresh.mjs` already relies on esbuild being
  deterministic at one pinned version. That becomes the everyday path, not a maintainer check.
  - So no output byte may depend on a local choice. The font selection decides which files are built, never what is
    in a file (see [Fonts](#fonts)).

## Layout

- **One npm project for the libraries:** `runtime/components/base/webroot/build/libs/`, with `package.json` and a
  committed `package-lock.json` holding every library at its exact pinned version.
  - The `--before` cooldown and `--ignore-scripts` stay.
  - `refresh.mjs` and `patches/` move there from `libs/`, so no build source sits in the served output directory.
    `libs/` then holds `MANIFEST.json`, `NOTICES.md`, `licenses/` and the ignored output.
  - `refresh.mjs` becomes the copy, bundle and patch step that reads from `node_modules` instead of running
    `npm pack` per library.
  - The lock replaces `BUILD_DEPS`, the 150 or so direct and transitive pins of the bundle inputs kept by hand in
    `refresh.mjs`.
  - Dependabot and `npm audit` then see every library and its declared dependencies.
- **What the one project costs, and what its audit does not cover.**
  - `npm pack` fetches a tarball alone. As a dependency, each library brings its whole tree: Mermaid's d3 and
    cytoscape, the optional native canvas of `pdfjs-dist`. None of that is served, since only `dist` files are
    copied. `--omit=optional` is not a way out, because esbuild's binary is an optional dependency.
  - The audit then reports packages that are not served, and misses some that are. A library copied as published
    can inline a dependency at a version the lock does not show: Chart.js's UMD file carries `@kurkle/color` 0.3.2
    while the lock resolves 0.3.4. Mermaid had the same gap (its `dist` carried DOMPurify 3.4.12) until it was bundled
    from the lock. `audit-js` therefore keeps a list of these inlined copies (`BUNDLED_COPIES` in `manage.mjs`): it
    reads each version from the distributed file and asks the registry's advisory endpoint about it, and it names the
    ones that carry no version (rrweb's) for review by hand. A clean `npm audit` covers the bundle inputs and the
    libraries' own versions, not what a copied `dist` has inside it.
  - Hoisting can change the version a bundle input resolves to, which is why the whiteboard is kept apart. The
    byte-for-byte check of step 2 catches it.
  - This is accepted for a complete lock. One lock that names every library and everything it declares gives the
    clearest record of versions and the most reproducible install, and that is worth a larger `node_modules`.
    Step 2 measures the tree. Only a tree out of all proportion reopens the question, and the fallback is then
    `npm pack` for the copy-only libraries, checked against the `tarballIntegrity` already in `MANIFEST.json`.
- **The whiteboard stays its own project** (`build/whiteboard/`). Its isolated React and transitive Mermaid versions
  are deliberate (see `build.mjs`). It should not be merged into a workspace that would hoist them.
- **Each build installs into a temporary tree under `target/js/`, never into the source tree.** A run makes
  `target/js/build-XXXX/` with one `npm ci --prefer-offline` per project, bundles and patches there, and deletes it.
  The output goes where it is served, `webroot/screen/webroot/libs/`.
  - npm's cache (`~/.npm`) shares the tarballs, so a rebuild and a second worktree do not download them again.
  - Nothing installed outlives the run, so `build.mjs` (`replaceOnce`) and the OpenUI `patch -p1` edit the installed
    packages directly: no changed or removed patch is ever left to undo.
  - Node skips `finally` when a signal ends it, so `refresh.mjs` also removes its temporary and staged trees on
    SIGINT, SIGTERM and SIGHUP. The next locked `moq` JS task removes whatever a killed run still left
    (`target/js/build-*`, `update-*` and `screen/webroot/.libs-stage-*`).
  - The stamp makes a rebuild rare, which is why a persistent install was not worth its upkeep.
  - `cargo clean` removes `target/js` with the rest. `tidy-cargo` prunes `target/js` as it prunes `target/moqui-app`.
- **`build-js` decides what to do from two checks:**
  - **The outputs, on every call.** Each file of the selected set is compared with its sha256 in `MANIFEST.json`, in
    shell (`sha256sum -c`, or `shasum -a 256 -c`). 41 MB hashes in well under a second. A deleted or edited file is
    then found, and a built tree needs no Node to run the tests, as with the vault module.
  - **The inputs, by a stamp, `target/js/stamp.json`.** It is not under `libs/`, which is served. It records:
    - the hash of each `package.json` and lockfile, `refresh.mjs`, `patches/`, everything under `build/whiteboard/`
      (`build.mjs`, the entry files, `library/`), `fonts.json` and `MANIFEST.json`;
    - the font packs selected;
    - the Node and npm versions and both esbuild versions (0.28.2 for the libraries, 0.25.12 for the whiteboard), for
      diagnosis only: they are not compared, since the output hashes decide;
    - the checksum list derived from `MANIFEST.json` for the first check.
  
  `build-js` does nothing while the outputs match and the stamp matches. With no stamp and matching outputs it writes
  the stamp and builds nothing.

## moq tasks

| Task | What |
|---|---|
| `build-js [--force]` | `npm ci --ignore-scripts --prefer-offline` for each project into `target/js`, then the copy, bundle and patch steps into `libs/`. Then it checks every file against `MANIFEST.json` and writes the stamp. Skips the work when the outputs and the stamp match. `build`, `test`, `test-client`, `test-framework`, `test-apps`, `archive`, `desktop build` and `android build` call it, as they call `ensure_vault_wasm`. It dies with an install hint when Node or npm is missing or too old. |
| `update-js <lib> <version>` or `update-js --all` | `--all` takes the newest release each `^pin` admits (the same major, or the same minor of a 0.x). A new major is always its own `update-js <lib> <version>`. Changes a pin with the cooldown (`npm install --before`), rebuilds, and rewrites `MANIFEST.json` and `NOTICES.md`: today's `refresh.mjs --write`. It builds every font pack, selected or not, since the manifest records them all, and it is the only task that downloads the license texts. The diff of the lockfile, manifest and notices is what gets reviewed. `plans/archive/License-Review.md` is updated as now. |
| `verify-js` | A from-scratch rebuild in a scratch directory, compared with the manifest, ignoring the stamp and the cache: today's `node refresh.mjs`. Part of the pre-commit checks when a lockfile, `refresh.mjs`, `patches/` or `build/` changed. |
| `outdated-js` | Each pin compared with the registry: the newest release `^pin` admits, the latest, and the newest, each as of the cooldown cutoff where it applies. `npm outdated` needs an installed tree, which the source directories do not have. |
| `audit-js` | `npm audit` for each project. It also checks the dependencies inlined in copied `dist` files that the lockfiles do not show, from the `BUNDLED_COPIES` list in `manage.mjs`. |
| `clean-js` | Removes the built `libs/` output (everything gitignored there), the extra font packs and `target/js/`. It leaves the npm cache and the checkout's font selection (`build/fonts/selected.txt`) alone. |
| `fonts-download [pack...] [--list] [--remove pack]` | Adds or removes optional font packs (see [Fonts](#fonts)) and runs `build-js`. Named like `opensearch-download` and `chromium-download`. |
| `source-archive [file]` | The source release with the npm cache and the vendored crates (see [Source release](#source-release)). In the Package group. |

`build` gains `--offline`, which `build-js` honors. `./moq help` gains a **JavaScript** group for these tasks. Shell
completion picks them up from the task table.

## Clean tasks

Today `clean-all` runs `clean-db`, `clean-log`, `runtime/tmp` and `tidy-cargo`, and there is no `clean`. The defaults
should follow what each name suggests.

| Task | What |
|---|---|
| `clean` | `tidy-cargo`: the old cargo artifacts go, the current build stays. Instance data is not touched. |
| `clean-dev` | The opposite of an artifact clean. It resets the local instance for a fresh load and run, and keeps all build output (Rust, JS, the vault module, the downloads). It runs `clean-db` (the database, txlog and OpenSearch data), `clean-log` and `runtime/tmp`. It keeps `runtime/drive-export` and `runtime/git-export`, which are exports a user named, and `runtime/cert`, `runtime/.env`, `runtime/MoquiConf.xml`, `runtime/save` and `runtime/worktrees`, which hold Dev AIDE work. It refuses while the server runs, as `clean-db` does. |
| `clean-all [--yes]` | `clean-dev`, `clean-js`, `runtime/types` and `cargo clean`: back to a fresh checkout, apart from the program downloads (`runtime/opensearch*`, `runtime/chromium*`, the embed cache), the saves, the certificate and the local conf. It first prints what it will remove, each path with its size, and what it keeps, then asks for confirmation. `--yes` skips the question, for scripts. With no terminal and no `--yes` it removes nothing and fails. |
| `clean-db`, `clean-log`, `tidy-cargo` | Unchanged. |

`load-save` uses the `clean_all` function today, which deliberately leaves `target/` alone. It will call `clean_dev`
instead, which is what it means. Check `core.sh` so that `clean` as an exact name never becomes ambiguous with the
prefix match. Update the task tables in `docs/Run-Deploy.md` (the Gradle equivalents) and the README.

`clean-all` changes meaning: today the build survives it, and after this it costs a cold build of the whole
workspace. The project is new enough that the name can change meaning. The summary and the confirmation are there
because the cost is large whoever runs it. The summary comes from the same list of paths the removal uses, so the
two cannot disagree. `--dry-run` prints it and stops, as for the other tasks.

## Rust side

- **The embedded stub.**
  - `crates/moqui-embedded/webroot-files.txt` puts `libs/tom-select`, `libs/sortablejs` and `libs/NOTICES.md` into
    every binary.
  - At the stub level, `select.rs` skips a missing `libs/` entry with a `cargo:warning` naming `./moq build-js`.
    `cargo lint` and `cargo lint-min` in CI therefore need no Node.
  - `build.rs` still emits `rerun-if-changed` for the missing path, so a later `build-js` embeds the files on the
    next cargo build.
  - At `--embed production` and `all`, a missing library fails the build naming the task, as the vault module does.
- **The tests that read `libs/`.** `crates/moqui-core/tests/browser_libs.rs`, the compiler-dom tests in
  `screen/statics.rs`, `moqui-client` assets, the mobile tests and `pdf.rs` each fail with one message naming
  `./moq build-js` rather than a missing-file error. `browser_libs.rs` keeps the check that the built tree matches
  the manifest and that nothing else is in `libs/`. Its exception list now covers `fonts-extra/installed.json` and the
  files of unselected font packs, and no longer `refresh.mjs`.
- **The server.**
  - At startup it logs one error naming the missing libraries and goes on (the failure-tolerance rule).
  - Without `libs/vue/compiler-dom.cjs.js`, qvt server compilation reports that the libraries are not built instead
    of a script error.

## Fonts

Default (built by `build-js`):

- Atkinson Hyperlegible Next and Mono, latin and latin-ext, as now.
- Font Awesome, Material Icons and Material Symbols.
- Excalidraw's Latin faces: Excalifont, Virgil, Nunito, Lilita, Comic Shanns, Cascadia, Assistant, and all Arimo
  subsets at 132 KB.
- **Not Xiaolai.** That takes the whiteboard from 20 MB to about 7 MB.

`build.mjs` already rewrites Excalidraw's font table for Arimo. The bundle keeps every face in that table, Xiaolai
included, so its bytes are the same whatever is selected. `build.mjs` guards Xiaolai's registration and its place in
the Excalifont fallback with `globalThis.MoquiBoardCjk`. The board page sets that flag from
`libs/fonts-extra/installed.json` before the editor loads, and to false when the file cannot be read. A pack that is
not installed therefore falls back to the system font, not to a 404, in drawing and in SVG export.

CJK text on a board then draws in the system CJK font until the pack is added. On a shared board each viewer's
system font measures that text differently, so it can wrap differently for each of them, and an export takes the
exporter's font.

Optional packs, defined in a committed `build/fonts/fonts.json`:

- each pack's npm package, version and integrity;
- its license;
- which `@font-face` rules or Excalidraw faces it supplies.

The packs:

- `excalidraw-cjk`: Xiaolai, taken from the Excalidraw tarball already in `target/js`.
- `cjk`, `arabic`, `devanagari`, `cyrillic-greek` and others for the qvt shell's text, as fontsource subsets of
  Atkinson where they exist and Noto otherwise. Each is emitted as `libs/fonts-extra/<pack>/` with its own
  `<pack>.css`, whose `unicode-range` rules mean a page loads a face only when it shows those characters. There is
  no one generated style sheet for all packs: its content would change with the selection and could not be in the
  manifest.

How packs are chosen and shipped:

- There is one selection: the packs installed in the checkout. `fonts-download` records it in
  `build/fonts/selected.txt`, which is gitignored and per checkout, and builds the files.
- What is installed is what ships. `archive`, the embedded production binary, `desktop build` and `android build`
  carry every installed pack, and each prints the packs it includes. A pack left in by accident costs size and
  nothing else.
- No optional pack is installed by default, so a build from a fresh checkout ships the default fonts alone.
  `excalidraw-cjk` is left out, decided: it is 13 MB for one whiteboard face. A build that wants it runs
  `fonts-download excalidraw-cjk` first.
- `MOQUI_FONT_PACKS` sets the selection without the file, for a CI job or a scripted release build.
- The selection is part of the stamp.
- `MANIFEST.json` records every pack's files, so a pack is reviewed and checked like the rest. A file of an
  unselected pack is not required to exist. The Noto CJK subsets are hundreds of files, so the manifest grows with
  them. `NOTICES.md` covers every pack, selected or not.
- `build-js` writes `libs/fonts-extra/installed.json`, the list of installed packs. It is the one built file that
  is not in the manifest. The server reads it to link each pack's style sheet, and the board page passes it to the
  editor.
- `installed.json` goes into the archive, the binary and the apps with the packs, as the record of what that build
  holds.

## Builds elsewhere

- **CI** (`ci.xml`, `podman/Containerfile.build`).
  - The build image adds Node and npm.
  - The framework and app jobs run `./moq build-js` before cargo tests, with `target/js` and `~/.npm` in the job
    cache keyed by the lockfile hashes. The lint job needs neither.
  - `libs/` is never in a job cache, so the framework job's `build-js` rebuilds from the locks on every push and fails
    on any difference from `MANIFEST.json`. That is the `verify-js` check, so no job runs `verify-js` as well.
- **Offline and failure.**
  - A fresh clone needs the registry once, to build and so to test. After that `npm ci --prefer-offline` installs
    from `~/.npm` with no network, and a built tree needs neither Node nor the network.
  - A registry outage stops only a machine with a cold cache. CI keeps `~/.npm` in the job cache.
  - A pinned version that is unpublished fails `build-js` on a cold cache, naming the package. The fix is a new pin
    through `update-js`. Nothing else in the tree changes.
  - A build with no network at all is what the [source release](#source-release) is for.
- **Debian** (`plans/Debian-Packaging.md`).
  - Debian forbids network access in a package build. That is unchanged: today's committed minified files are what
    lintian rejects.
  - Building from source in the package build is now the default path, so that plan's option 2 (esbuild from Debian,
    and `node-*` packages or an npm cache prepared offline) fits better than before. That plan gets a line pointing
    here, and its 21 MB figure for `libs/` becomes 41 MB.
- **Desktop and mobile.** `scripts/moq/desktop.sh` stages `libs/<id>` from the built tree. Its stage step calls
  `build-js` first.

## Docs and rules to change

- `webroot/screen/webroot/README.md`: the vendored-libraries section (built, not committed; the tasks; the manifest
  as the lock).
- `docs/Build-Test.md`: "Node is not needed to build or test" becomes the requirement and the tasks.
- The building and pre-commit skills: `verify-js` when JS inputs change.
- `AGENTS.md`: "Node is not a test dependency" is still true of the test runners: `moqui test` and
  `moqui test-client` run the JavaScript specs without Node. Add that `./moq build-js` needs Node and npm, and that
  libraries change through `update-js`, never by hand.
- `docs/Run-Deploy.md`: the clean tasks.
- The pre-production skill and `docs/Build-Test.md`: the source archive and its no-network job.
- `docs/interfaces/Pipelines.md` and the comment at the top of `podman/Containerfile.build`: both say Node is not
  needed.
- The pages that name `libs/refresh.mjs` or `node libs/refresh.mjs`, for its new path and the tasks:
  `docs/ui/Qvt-Client.md`, `docs/ai/Assist.md`, and the proofs pages `V3-Web-Frontend-Security.md` and
  `V15-Secure-Coding-and-Architecture.md`.

## Source release

A release has a source archive that builds with no network: the JavaScript libraries and the Rust crates are built
from sources it carries. It holds no built output. This is the same build as a developer's, with the downloads
done ahead of time.

`./moq source-archive [file]` writes it from a clean checkout, labelled with the tag at HEAD for a release and with the
commit otherwise:

- **The tree**, as `git archive` gives it, for mr-moqui and for each component repository the release includes.
- **npm.** A cache directory, `vendor/npm/`, filled by `build/libs/source-cache.mjs` (`npm cache add` of every
  tarball the locks name). `build-js` uses it with `--offline` when it is there. The lock's integrity hashes check
  every tarball, so the cache needs no review of its own.
  - The lock holds esbuild's binary for every platform as optional packages, and `npm ci` would fetch only the
    host's. The archive carries all of them: simpler than a platform list, at a few tens of megabytes.
- **Cargo.** `cargo vendor --locked --versioned-dirs` into `vendor/cargo/`, with the source replacement it prints
  written to a config file the offline build passes (`cargo --config vendor/cargo/config.toml --offline`). It is not
  written into `.cargo/config.toml`, which a developer's build also reads.
  - It covers every lock: the workspace's, `crates/moqui-client-tauri` and `crates/moqui-mobile` (one
    `cargo vendor --sync` run over the three), and the generated crate `./moq build` links add-on components
    through.
  - It covers the git dependencies, which `cargo vendor` takes at their pinned revisions: quickjs-oxide, IronCalc,
    rust-mysql-simple, stun-proto and y-crdt, 9 of the 842 packages in the workspace lock.
  - `Cargo.lock` checksums check each crate.
- **What else the build downloads.** Each is either carried or named as a prerequisite:
  - the Postgres archive that a package with embedded Postgres embeds (`crates/moqui-drivers/build.rs`): carried,
    with the checksum the build already checks;
  - the Rust toolchain of `rust-toolchain.toml` (1.100.0) with the wasm target for the vault module, and Node and
    npm: prerequisites, named in the archive's README with their versions;
  - OpenSearch and Chromium, which only the `--embed` options need: not carried. The offline build is the package
    without them.
- **`./moq build --offline`** passes `--offline` to cargo and npm and fails, naming the item, where something would
  be downloaded. It works in any checkout whose caches are warm, not only in the source archive.

The check is a job of the release: unpack the archive in a container with no network and the prerequisites, run
`./moq build --offline --release`, then `./moq test-framework`. Until that job passes, "offline" is not claimed.
This belongs to the pre-production skill, not to every push.

How far this goes:

- npm and cargo are expected to reach a fully offline build. The size is the cost: a vendored crate tree of this
  workspace is likely several hundred megabytes unpacked, measured when this is built.
- A crate whose build script downloads something is found by the no-network job, and is fixed by carrying the file
  or setting the crate's option for a local copy. The embedded Postgres archive is the one known case.
- This is not Debian's form. Debian builds from packaged crates, not a vendored tree
  (`plans/Debian-Packaging.md` §3). It is the "project apt repository" track of that plan (§6.5), which already
  assumes `cargo vendor` output as an extra tarball.

## Release size

The release ships one source archive. The arm64 Linux verification fixture, including seven application repositories,
compressed to 847 MiB. Its extracted inputs occupied 1.2 GiB for Cargo, 503 MiB for npm and 100 MiB for the five
PostgreSQL payloads. The release commands and CI runner requirements are in [Build-Test](../docs/Build-Test.md#offline-source-release).

## Steps

Steps 1 to 7 are built.

1. Add the clean tasks: `clean`, `clean-dev`, and the new `clean-all` with its summary, confirmation and `--yes`
   (without `clean-js` until step 3). Switch
   `load-save` to `clean_dev`. Update the docs. These do not depend on the rest.
2. Make the libraries one npm project. Move the pins from `refresh.mjs` into `build/libs/package.json` and its lock,
   move `refresh.mjs` and `patches/` beside them, and point `refresh.mjs` at `node_modules`, patching copies only.
   Record the package count and size of the installed tree. The complete lock stays unless that is out of
   proportion. `verify-js` must reproduce today's `MANIFEST.json` byte for byte before anything else changes.
3. Add `build-js`, `verify-js`, `update-js`, `outdated-js`, `audit-js` and `clean-js`. Wire `build-js` into `build`,
   the test tasks, `archive` and the app builds. Make `tidy-cargo` prune `target/js`.
4. The Rust side: stub tolerance, test messages, the startup log.
5. Gitignore the output and remove it from the tree in one commit. Update CI and the build image in the same
   change, so no push runs without Node.
6. Fonts: prove first that the editor can drop faces at run time with the bundle's bytes unchanged. Then drop
   Xiaolai from the default, and add `fonts.json`, the per-pack style sheets, `installed.json`
   and `fonts-download`.
7. The source release: `build --offline`, `source-archive` with the npm cache and the vendored crates, and the
   no-network job. Add it to the pre-production skill.

## Remaining (Linux)

- Run the `offline-source` job on a provisioned `linux-offline` pool (rootless Podman or Docker and the
  `localhost/moqui-build` image with Chromium). Until it passes there, "offline" is not claimed.
- Build the libraries with Node 22 in CI. The build image has Node 24; Node 22.23.5 with npm 10.9.9 reproduced the
  manifest on macOS.

## Future: npm in Dev AIDE for components

Built 2026-11-23: `build-js` finds its projects rather than listing them. A component with a `build/js-build.json`
(its output directory under `screen/`, its npm projects under `build/`, its build script) gets the same `npm ci`,
bundle, manifest check and stamp, through the shared `build/js-kit.mjs`. Its output is ignored in its own repository
apart from its `MANIFEST.json`, which is committed there with its locks. The framework's webroot is simply the first
component built this way. The contract is in [Build-Test](../docs/Build-Test.md#browser-libraries-of-a-component).

Still to come, on that, Dev AIDE can:

- add a JavaScript library to a component while building screens: search npm, pick a version past the cooldown, and
  see its license against an allow list;
- write the lock and manifest entry, run `build-js` for that component, and show the library's global or module to
  the screen being edited;
- do the same for server-side bundles that run in QuickJS, as the compiler-dom and OpenUI ones do.

The rules stay the same as for the framework: `--ignore-scripts`, the cooldown, exact pins, recorded integrity and a
reviewed manifest diff. `--ignore-scripts` covers the packages, not the component's own build script, which is code
that Node runs on the developer's machine. It is reviewed as the component's other code is, and `build-js` runs it
only for a component the checkout already builds. Running npm from a server process is a supply-chain surface, so
this is for dev instances only and runs as the instance's own user. A production node takes a component's libraries
only as built output in a release.
