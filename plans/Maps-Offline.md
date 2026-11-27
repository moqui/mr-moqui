# Maps offline: region packages for the Android app and the desktop client

Status 2026-11-27: OF1 built (§3); OF2 next. It carries the "offline mobile packages" item of
[Maps §9, Later research](archive/Maps.md#9-delivery-and-acceptance) forward as work for now, on the user's decision of
November 26, 2026: a person downloads a region of the map to the Android app or the desktop client and browses it with
no network, its basemap, its own places and overlays, and the region's postal codes and Overture places. The web PWA is
left out. The live location feeds of the same section are [Location-Sharing](Location-Sharing.md).

## 1. What it builds on

| Piece | Today | What the package needs of it |
| --- | --- | --- |
| PMTiles | `moqui-lib` `pmtiles.rs`: tile IDs (`tile_id`, `tile_zxy`), directories (`decode_directory`, `encode_directory`), `locate_tile` (depth at most 3), `Header`, and `write_archive` (:786), which copies already-compressed tiles in tile-ID order, builds the whole archive in one `Vec`, and keeps only the parsed `Metadata` (name, attribution, version, type, layer IDs) | A writer that streams (a regional extract is hundreds of MB), and the source's metadata JSON copied whole |
| Tile reads | `moqui-tools` `maps_tiles.rs`: a directory cache keyed by the archive's tag and size, `answer_tile` with the revision check, `read_archive_tile` opening the location through the facade; 16 MiB per directory and per tile; gzip or none | The same reads for every tile of a bbox and zoom range, without a request per tile |
| Jobs | `MapOverlayTiles` (`MapsData.xml:125`, `MapOverlays.js:385` `buildTiles`, `maps_simplify.rs:77` progress and cancel on the row, a stale run ended) | The model for the package build |
| Zip | `ec.resource.writeZip(target, entries)` (`script/host/files.rs:880`): each file streamed in windows to a part file, zip64 past 4 GiB, unreadable files reported in `failures`; `FileServices.download#Zip` and the hourly `clean#DriveTempFiles` | The package file |
| Style and assets | `LOCAL_PROTOMAPS` (`MapsSetupData.xml:87`): `/libs/protomaps/{light,dark}.json`, sprites `/libs/protomaps/sprites/v4/`, fonts `/libs/mapfonts/noto-sans/` (CSS and woff2), the outline `/libs/naturalearth/countries-110m.json` | Copied into the package, the style's sprite and the fonts' template made package-relative |
| Configuration | `MapServices.get#EffectiveConfig` (`MapConfig.js:695`): profile, basemaps (`url` naming the revision), layers, projections, attribution, `allowedOrigins` | Frozen into the manifest, its URLs rewritten |
| The user's data | Saved places (`Maps/places.geojson`, `MapOverlay` `isPlaces`), overlays (`MapOverlay`, `overlayContent` 4 MiB sanitized), saved views (`MapSavedView`) | Copied as they read now, under the same access |
| Lookup data | `GeoPostalCode`, `GeoPostalCodePlace`, `GeoPoint` through `GeoPointAssoc` `GPR_REPRESENTATIVE`; `OverturePlace`, `OvertureAddress` of a region's active import | A subset of the bbox |
| The apps | One WebView. Server pages go through the app scheme to Rust (`moqui-client` proxy), and every answer is a `Vec` in memory (wry `binding.rs:270`); a 206 works on Android's WebView (`RustWebViewClient.kt:109`). Offline answers cover only the shell, Chat, Mail and Compose (`offline.rs:15`). The per-account store (`client/data/<account>.db`, `store-max-mb` 512) has an outbox with `clientRef`, and `FlushWorker` (WorkManager). WebView storage is wiped at every sign-out (`PageStorage.kt`). No page gets the filesystem (Mobile-Desktop.md, "Never expose"). The shell bundles no map library (`shell/libs.txt`) | A package store held by the host, served to the page by range through the scheme; a bundled offline Maps screen |

## 2. Design

### 2.1 A package

A **map package** is one region, at a zoom range, built on the server from what its owner may see, and downloaded as
one file. `moqui.maps.MapPackage` (in `MapEntities.xml`): `packageId`, `ownerUserId`, `name`, the bbox (`west`,
`south`, `east`, `north`, WGS84, across the antimeridian as the viewport queries are), `minZoom`, `maxZoom`, the
profile and its basemap layer IDs, what it includes (`includePlaces`, `includeOverlays`, `includePostal`,
`includeOverture`), `statusId` (pending, building, ready, failed, cancelled), `progress`, `cancelRequested`,
`jobRunId`, the result's `location`, `size`, `sha256` and `revision`, `builtDate`, and the source revisions it was cut
from. A package is the owner's; it is not shared (a household member builds their own).

The package file is a zip (`writeZip`, stored members, no recompression of tiles):

| Member | Contents |
| --- | --- |
| `manifest.json` | Schema version; the package's id, name, bbox, zooms, revision, built date; the frozen effective configuration with every URL package-relative; each source with its attribution, license and revision; each member's path, size and SHA-256 |
| `basemap/<layerId>.pmtiles` | The extract of each basemap layer (light and dark share one archive when the profile does) |
| `style/<flavor>.json`, `sprites/…`, `fonts/…` | The style with its `sprite` and `ol:webfonts` made relative; the sprites; the fonts' CSS and woff2 the style names |
| `outline.json` | The Natural Earth outline, for the zooms below the extract |
| `places.geojson`, `overlays/<overlayId>.geojson` | The owner's places and the overlays they can read that meet the bbox, as `overlayContent` gives them; a tiled overlay as its tiles' extract (`overlays/<overlayId>.pmtiles`) |
| `postal.json` | The postal codes whose representative point is in the bbox: code, display code, place names, admin names, point, precision |
| `overture-places.json`, `overture-addresses.json` | The rows of the active imports whose point is in the bbox, with their license per source |

Record layers (Mantle's facilities, assets, a party's addresses) are not packaged in this first build: their access is
read at each request, and a copy on a device would outlive a revoked grant. Said in the screen.

### 2.2 Building it

- **The streaming writer** (`moqui-lib` `pmtiles.rs`): `ArchiveWriter` takes tiles in ascending ID order and writes
  their bytes to a part file as they come, keeping only the directory entries (with runs of identical neighbours, as
  now); `finish` writes the header, root, metadata and leaves to the final file and copies the data part after them
  (the header's offsets then known). `write_archive` becomes a wrapper writing to a `Vec`, and a test checks both give
  the same bytes on the overlay goldens. The metadata is the source's JSON copied whole, not the parsed subset.
- **The extract** (`moqui-tools` `maps_extract.rs`, `MapSourceServices.build#PackageExtract`, `type="rust"`): for each
  zoom, the tiles of the bbox, sorted by tile ID (a bbox is not one run of the Hilbert order); each located through the
  directory cache of `maps_tiles.rs` and read with `read_at`, then handed to the writer as stored. Reads are batched by
  offset so neighbouring tiles are one read. The archive's tag is checked at the start and the end, so an archive
  replaced while it was cut ends the run as failed, never a mixed extract.
- **The estimate**, before anything is built: tiles per zoom from the directories (entries in the bbox and their
  lengths), so the screen shows the size and the tile count before Build. A package over `maps@package-max-bytes` (a
  new setting on the `maps` element: its XSD attribute, its `conf_schema.rs` entry and its default, 2 GiB, in
  `MoquiDefaultConf.xml`) is refused at the estimate.
- **The job** `MapPackageBuild` (a `ServiceJob` as `MapOverlayTiles` is): the extract, the other members read under
  the owner's access, the manifest, the zip written to `runtime://tmp/map-packages/<packageId>-<revision>.zip`; progress
  and cancel on the row between tiles of a batch; a run that stopped with no word for an hour is ended stale. A package
  is rebuilt with Update (a new revision); the old file is removed once the new one is ready. Files of a package
  deleted, or not downloaded for `maps@package-keep-days` (30), are removed by the hourly clean job.
- **Access.** Only basemaps the owner's profile offers, overlays they can read now, their own places; a provider's
  remote tiles (OpenFreeMap) are never packaged, since its terms allow a limited offline download, not a mirror: an
  offline package needs a registered archive. The postal and Overture members carry their licenses.

### 2.3 Serving it

`Maps/Offline.xml` (a subscreen of the Maps app, `MAPS_APP`): the list of the user's packages with their size, zooms,
state and built date; New package from the current view of Browse (drawn as a box on the map, the zooms chosen, the
estimate); Update, Cancel and Delete. A `package` transition sends the zip by `ec.web.sendPinnedResourceResponse` with
its SHA-256 as the strong tag, by range, so a download resumes; it checks the owner on every request, and a revision
that is no longer the package's is 409. Its `.tui.json` lists packages and their states (no build from the terminal
beyond Update and Delete).

### 2.4 On the device

`moqui-client` `packages.rs`, used by both apps:

- **Store.** `client/data/<account>.packages/<packageId>/<revision>/`, owner-only like the store; outside
  `store-max-mb`, with its own `packages-max-mb` (client conf, 4096 by default) and a free-space check before a
  download starts. `personal` device mode keeps packages across sign-outs; `shared` mode wipes them with the account's
  data.
- **Download.** By ranges of a few MiB, each with `If-Range` the package's tag, written to a part file and resumed
  from its length; then the whole file's SHA-256 checked against the server's tag; then unpacked (only the members the
  manifest lists, each within its stated size, each checked against its SHA-256, no path outside the directory), and
  the revision directory swapped in by rename; the previous revision removed after. A failed check leaves the previous
  revision in use. Android runs it in `PackageDownloadWorker` (WorkManager, resumable, with an "only on Wi-Fi" choice),
  the desktop in a task of the host.
- **Serving to the page.** The app scheme answers `/__moqui-app/maps/<packageId>/<path>` from the package directory,
  for the app's own origin only: HEAD, GET and one `Range` as a 206 slice read from the file (never the whole file in
  memory), the member's SHA-256 as its ETag. A path not in the manifest is 404.
- **Commands.** `packages_list`, `packages_download`, `packages_cancel`, `packages_delete`, `packages_status`.

### 2.5 Offline Maps in the apps

- The OpenLayers main entry (and `ol-pmtiles`, `pmtiles`) join the shell's bundled libraries (`shell/libs.txt`), and a
  bundled screen `MapsOffline.qvue` joins `bundled-screens.txt` (`precompiled_qvue`); the raster part is not bundled (a
  COG layer is not packaged in this build).
- A package's source attribution comes from its archive's metadata and is often HTML: the offline screen shows it
  as text, never through OpenLayers' HTML attributions (review of OF1).
- `m-map` gets a **package mode**: its configuration is the manifest's, its allowed origin the package path, and its
  fonts and sprites the package's; everything else (styles, layers, Locate me, places) is the same component.
- Search offline: the package's places, postal codes and Overture places, in the page over the package's JSON (bounded
  by the bbox). Overture address matching (`moqui-maps` `address.rs`) runs in the host if that module builds without
  the Parquet reader and its codecs; otherwise addresses are searched by street key only and that is said.
- A place saved offline is an outbox item (`store#Place` with a `clientRef`), sent when the server answers; a place
  changed on the server meanwhile follows the existing `baseStamp` conflict rule.
- The shell's Maps entry opens Browse when the server answers and the offline screen (with the device's packages) when
  it does not; the offline screen says it is offline and the package's built date.

## 3. Phases

| Phase | Deliverable |
| --- | --- |
| OF1 — server | `MapPackage`; the streaming writer; the extract and its estimate; `maps@package-max-bytes` and `package-keep-days`; `MapPackageBuild`; `Maps/Offline.xml` and its terminal twin; the `package` transition. Regression `framework/test/MapsPackages.spec.js` (a small fixture archive cut and served by range; a layer the owner cannot see left out; over the cap refused at the estimate; cancel; a replaced source ending the run); Rust tests of the bbox tile set (antimeridian too), the writer's equal bytes and the extract's bounds. Built November 26 and 27, 2026, after six review rounds: a build per package and one pending or building per user, an installation queue (`maps@package-concurrent-builds`, 1) under one scheduler that runs after commit, ends lost runs (a queued run judged by its job run's own stamp after 6 hours, a started one by `progressDate` after an hour) and passes by an owner in a tarpit lock; a random `buildKey` naming each run's files and guarding every write; the build signed in as the package's owner whoever started it; `maps@package-user-max-bytes` (8 GiB) and the whole-package budget enforced while cutting; one zoom at a time with a bounded key map; another user's places never packaged |
| OF2 — device store | `moqui-client` `packages.rs`, the scheme path in both apps, the commands, `PackageDownloadWorker`. Tests in `moqui-client` (a range answered from the file, resume after a cut, a checksum mismatch keeping the previous revision, a member outside the manifest refused, the shared-mode wipe) and the Kotlin unit test of the worker's resume |
| OF3 — offline Maps | The bundled libraries and screen, `m-map`'s package mode, offline search, places through the outbox, the shell's entry. A page test of the package mode; `./moq verify-js` |
| OF4 — finish | Acceptance `MapsOfflinePackage` on the emulator (`./moq android emulator`): a package built, downloaded, the network cut, the region browsed and searched, a place saved and sent when the network returns. Guides: Maps "Offline packages", Mobile-Client, Desktop-Client, Privacy (what a package keeps on the device) |

## 4. Licensing and privacy

A basemap extract from OpenStreetMap data is a produced work under ODbL: the package carries the attribution and the
source's notice, shows them on the map, and says in the manifest what it was cut from. Overture members carry each
source's license as the server shows them. A package holds only what its owner could read when it was built; the
screen says that a package on a device stays readable there until it is deleted. The `package` transition records each
download's device (its `UserLoginKey`) and revision (`MapPackageDownload`), and the account's Devices list shows each
device's packages, so an owner can see what is where.
