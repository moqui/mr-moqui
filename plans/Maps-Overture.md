# Maps: Overture Places and Buildings (O1)

November 26, 2026 — the crate plan [Maps §7](Maps.md#7-useful-rust-server-work) asks for when Overture's Parquet
dependency arrives, and the design of stage O1 ([Maps §4.3](Maps.md#43-optional-overture-data),
[§9](Maps.md#9-delivery-and-acceptance)). O2, regional address lookup, builds on it and is not planned here. The
guide is [Maps › Overture](../docs/tools/Maps.md#overture-places-and-buildings).

## 1. Why a crate, and which Parquet reader

Overture publishes GeoParquet: monthly releases on S3 and Azure, found through a STAC catalog
(`https://stac.overturemaps.org/catalog.json`). What its files are, read from the 2026-11-18.0 release's footers:
written by `parquet-cpp-arrow 17.0.0`, ZSTD level 1, data pages `PLAIN`, `RLE` and `RLE_DICTIONARY`, 256 to 512 row
groups a file (about 13,000 places and 1.7 MB compressed a group), a 3 MB footer per places file, and a GeoParquet
`covering` of `bbox.xmin`..`bbox.ymax` columns whose row group statistics place each group. A place file is 830 MB, a
buildings file 530 MB, 16 and 512 files a theme; a region of a few km² needs one or two files and a few row groups of
each, so the reader reads by HTTP range only the footer and the projected column chunks of the groups whose statistics
meet the region. The release data are deleted from S3 60 days after a release (`x-amz-expiration`, "release data 60
day retention"): nothing here may count on reading a release again.

Measured on 2026-11-26 (Apple M-series, `CARGO_BUILD_JOBS=4`, a scratch crate under `target/`, dev build from clean, and
the stripped release binary against a 0.36 MB one that only reads a file):

| Candidate | Clean build | Binary | Crates | Verdict |
|---|---|---|---|---|
| `parquet` 60.2.0, no default features | 23 s (the crate alone 9 s) | +0.78 MB | 30 (all but `parquet` already in the lock) | Reads no ZSTD: its codecs are closed (`Codec` is public only under `experimental`, `PageHeader` is `pub(crate)`), so no pure-Rust decoder can be plugged in |
| `parquet` 60.2.0 with `zstd` | 29 s | +1.26 MB | 33 | `zstd-sys` compiles the C zstd library: refused by [Build-Test, Native code](../docs/Build-Test.md#native-code-and-c-libraries) ("No C libraries in new dependencies") |
| `ruzstd` 0.9.1 (MIT, pure Rust) | 1 s | +0.12 MB | 2 | Taken, with our own Parquet reader |

**Decision.** Our own Parquet reader of the subset GeoParquet writers use, on `ruzstd` for ZSTD (Snappy our own, a page;
GZIP the workspace's `flate2`), as the workspace already reads PMTiles, MVT, WKB and shapefiles with its own code (§7).
The `parquet` crate, with no default features and `snap` (pure Rust), is a **dev-dependency** only: it writes the test
fixtures, so the reader is checked against another implementation's output (dictionary and plain pages, v1 and v2 data
pages, delta encodings, nested lists of structs, Snappy). The reader is about 2,000 lines; its weight in a build is
`ruzstd` and that.

**Why a crate anyway.** Overture is optional and needs outside data; the reader is a parser of untrusted binary input,
and O2's matching will join it. A feature-gated crate keeps all of it out of the builds that do not want it and out of
`moqui-tools` (in every build), and an edit in it rebuilds only it, the host and the link. It is `moqui-maps`, the name
Maps §7 gave it, an extension crate above core as `moqui-calc` is: services through `moqui_core::extension`, no hook.

## 2. The crate checklist

- `crates/moqui-maps`, a workspace member and `[workspace.dependencies]` entry; `ruzstd` (`default-features = false`,
  `std`) in the workspace dependencies; `parquet` (`default-features = false`, `snap`) a dev-dependency.
- Host feature **`overture`** = `dep:moqui-maps`, in `enterprise` (so Maximal and the default build) and not in Lite;
  `cargo lint-min` builds the host without it. `use moqui_maps as _;` under it in `moqui-host/src/lib.rs`.
- `moqui_core::extension`: the crate's entry with features `["overture"]`, and `("moqui_maps::", "overture")` in
  `LOCATIONS`, so a binary without it loads the service files and says the feature is not built in. A test compares the
  crate's registry with the locations the service files name (as `moqui-calc`'s).
- `scripts/moq/build.sh` `write_app_crate`'s opt-level list; `scripts/check-packages.sh` runs the crate alone
  (`-p moqui-maps --all-targets`), since the host feature sets above do not build its tests without the host.
- Docs: the Features table of [Build-Test](../docs/Build-Test.md), the crate table of
  [Practices-Rust §1](../docs/dev/Practices-Rust.md#1-crates-and-where-code-goes), [Maps](../docs/tools/Maps.md).
- License: `ruzstd` MIT; `parquet` Apache-2.0 (dev only). No new C, no `unsafe` of ours.

## 3. Data flow

1. **Discovery.** `maps@overture-catalog` (https only; the STAC root above by default; a conf setting, so a spec points
   it at its fixtures) lists the releases; Configuration offers them, the newest first. A run is pinned to one release named
   explicitly (`releaseVersion`, recorded before it reads anything) and to its catalog's `schema:version`: schema 2.x is
   read, another (in the release's catalog or the theme's collection) is refused (`SCHEMA_UNSUPPORTED`) before any data
   is. The theme's collection gives the files and the
   extent of each (`extent.spatial.bbox[i + 1]` is item `i`); only the items whose extent meets the region are fetched,
   and an item whose own `bbox` disagrees with the extent makes the run read every item's; a collection of more than
   1,024 items that must be read so is refused (`TOO_MANY_ITEMS`), never cut short.
2. **Reading.** Each file through `moqui_core::outbound` by `Range` (no internal address, https on whatever public host
   the catalog's items name, 60 s a request, a request at most 64 MiB), every range after the first pinned to the first
   answer's `ETag` (`If-Match`, else `If-Unmodified-Since` its `Last-Modified`): a file replaced while it is read is
   refused (412, or a whole answer), never mixed. The footer (at most 64 MiB) is read first, then for each row group
   whose `bbox` statistics meet the region the box columns, and when any row meets the region the theme's other
   projected columns. A row is kept when its own bbox meets the region. **Memory** is the footer's metadata of the
   projected columns, the decoded columns of one row group (each chunk at most two million entries, 32 MiB a page and
   128 MiB decoded; all of a group's together at most 256 MiB, refused past it; Overture's groups decode to a few MB),
   and the rows kept (capped: 200,000 places, 250,000 buildings, whose footprints are held for tiling). Every count a
   file claims (rows, values, block sizes, lengths, nesting) is checked before memory is taken for it. The run reports
   and reads its cancel between row groups and while its tiles are made.
3. **Staging.** Places are written as `OverturePlace` rows of the run (batches of 2,000, a transaction each); Buildings
   are cut into MVT tiles in memory (`moqui-lib` `geo/tiles.rs`, the large overlays' tiler, P4d) and written as one PMTiles
   archive (`pmtiles::write_archive`) to `dbresource://moqui/maps/overture/<regionId>/buildings-<importId>.pmtiles`
   through the ResourceFacade.
4. **Validation**, writing nothing that is in use: the schema's required columns of the theme with their physical
   types (text a byte array, numbers a number type; checked when the footer is read); the
   counts (a run with fewer than half the active run's rows is refused unless forced); the licenses each source row
   names, counted by dataset and license. A license the theme's acknowledgement does not cover, or a source that names
   none (`UNKNOWN`), stops the run as `OviReview` (staged, not active) until an administrator acknowledges it and
   activates the run, or discards it (`OviDiscarded`, its staged data deleted). A run older than the active one is not
   activated (`OLDER_THAN_ACTIVE`). A run cancelled, or taken for abandoned and ended, while it read is not activated.
5. **Activation**, in one transaction: the region theme's active run becomes the new one and the run before it its
   previous one; for Buildings the region's layer (`OVT_BLD_<regionId>`, `MLF_PMTILES_VECTOR`) is registered to
   the new archive (`register#ArchiveLayer`), so a map drawing the old revision is refused it (409) and loads its
   configuration again: the index and the tiles change together; the region theme's other runs waiting for review are
   superseded. Then, in transactions of their own, their rows and archives and those of runs older than the previous
   one are removed.
6. **Rollback** makes the previous run active again (its rows and archive were kept); **Remove** deletes a region
   theme's rows, archives and layer, keeping the run history. A run that fails or is cancelled deletes its own staged
   rows and archive and leaves the active run as it was.

Every step that sends is refused while `maps@external-requests` is false, and for a caller without their own
`MAPS_ADMIN` grant (the Geo data rule). Nothing is read at query time: search and drawing are local.

## 4. Storage and identity

Entities in `framework/entity/MapOvertureEntities.xml`, package `moqui.maps`:

| Entity | What it holds |
|---|---|
| `OvertureTheme` | Seed rows `PLACES` and `BUILDINGS`: the Overture theme and type, the license summary and attribution, the source licenses the acknowledgement covers (`licenses`) and who acknowledged them when |
| `OvertureRegion` | An administrator's region: a name, its box (at most `maps@overture-max-area` km², 2,500 by default), the release to import and the profile its Buildings layer is shown in |
| `OvertureRegionTheme` | A theme imported for a region: its active and previous run and its layer |
| `OvertureImport` | A run: region, theme, release, schema version, status, counts (files, row groups read and skipped, bytes, rows read, kept and rejected), the licenses found (JSON) and any new one, the archive and its tiles, who ran it, when, its job run and failure |
| `OverturePlace` | A place of a run: an internal ID (`<importId>-<n>`) and the external reference (`gersId`, `featureVersion`, the run's release), name, category (`basic_category`, `taxonomy.primary`), point, confidence, operating status, the first address, brand, and the source datasets and licenses |

Places are an indexed table, not basic Geo: search reads the rows of the active runs, the names that start with the
text first (the `importId`, `nameSearch` index), then those that have it anywhere or whose category does, each query
bounded and within the view's box when one is given. Buildings are only tiles. GERS IDs
are references, not keys: a release may change one, so nothing links to a place row by its internal ID across runs.

**Mantle is never written.** An import creates and changes only these entities, its archives and its own layer; it never
creates a party or facility, never changes a Geo, a point or a record's location, and removals or changed IDs in a new
release touch nothing outside them. A user's saved place keeps its own point.

## 5. Licensing and attribution

By theme and source, never one Overture license ([attribution](https://docs.overturemaps.org/attribution/)):

- **Places**: CDLA-Permissive-2.0 (Meta, Microsoft and others), Apache-2.0 (Foursquare, with its NOTICE) and CC0-1.0
  (AllThePlaces). A search result names its source datasets and licenses, and the search says "Places from Overture Maps
  Foundation" with the theme's attribution.
- **Buildings**: ODbL-1.0 (OpenStreetMap, Microsoft ML footprints) with CC-BY-4.0 sources (Esri Community Maps, Google
  Open Buildings). The layer's attribution is "© OpenStreetMap contributors, Overture Maps Foundation" and the theme's
  sources. **ODbL obligations**: the generated archive is a Derivative Database of the ODbL data. Showing it on a map is
  a Produced Work, which needs the attribution notice; giving the archive itself, or the tiles of it, to anyone outside
  the installation is a public use of the Derivative Database, which must then be offered under ODbL (or its source data
  and the means to make it). The archive is outside every drive, so only its layer guards it: the layer is offered to the
  installation's users whose map profile shows it (no `viewPermission` is set by the import); an administrator who wants
  fewer gives the layer a `viewPermission`. Configuration says so beside the theme.

A license found that the acknowledgement does not cover is held for review (§3.4); acknowledging adds it.

## 6. Tests

- **Rust** (`moqui-maps`): the reader on fixtures the `parquet` crate writes in the test (schema 2.x names: `id`,
  `geometry`, `bbox.xmin`.., `names.primary`, `basic_category`, `taxonomy.primary`, `sources.list.element.license`):
  dictionary and plain pages, v1 and v2, delta encodings, Snappy, nulls and empty lists, row groups skipped by
  statistics, a truncated file, a lying length, and bounds; ZSTD frames from `ruzstd`'s encoder; the STAC choice of items;
  WKB; the place and building rows. The spec's Parquet fixtures are goldens of these tests
  (`UPDATE_GOLDEN=1 cargo test -p moqui-maps`).
- **Regression** `framework/test/MapsOverture.spec.js`, with `HttpFixtures` serving a small catalog and the Parquet files
  by range: a holder's import active and searched; a non-holder, a license not acknowledged and outside requests off
  refused; an update that fails part way leaving the previous release active; an update activated and rolled back; a new
  license held for review; Buildings registered as a layer with its attribution, and removed.
- **Acceptance** `framework/test/acceptance/OvertureRegionImport.spec.js`: the latest real release for a box of about
  3 km² (central Florence), Places and Buildings, recording the time, the bytes and row groups read, the rows and the
  memory. Run once, November 26, 2026, on release 2026-11-18.0 (schema 2.0.0), the release binary on macOS: Places 9.0 s,
  one file, 4 row groups read and 508 skipped, 6.7 MB in 82 ranges, 46,200 rows read and 10,264 kept, none rejected, ten
  source datasets all under licenses the acknowledgement covers; Buildings 6.8 s, one file, 3 row groups read and 253
  skipped, 7.9 MB, 52,897 read and 4,837 kept (OpenStreetMap and Microsoft ML Buildings, ODbL), an archive of 221 KB in
  18 tiles to zoom 14; a search 12 ms. Process memory is reported only on Linux (the status map's MemoryUsed), so it was
  not recorded here.

## 7. What O2 builds on

The reader and the STAC client, the run, staging, activation and rollback, the license review and the readiness rows.
O2 adds the `addresses` theme's columns, its own table and the matching through the geocoder interface (Maps §6).
