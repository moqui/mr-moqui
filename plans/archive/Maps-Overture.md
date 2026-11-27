# Maps: Overture Places, Buildings and Addresses (O1, O2)

> Archived 2026-11-26 with [Maps](Maps.md): O1 and O2 built. Open testing is in
> [Testing-Needed](../Testing-Needed.md#maps); guide [Maps › Overture](../../docs/tools/Maps.md#overture-places-and-buildings).

November 26, 2026 — the crate plan [Maps §7](Maps.md#7-useful-rust-server-work) asks for when Overture's Parquet
dependency arrives, the design of stage O1 ([Maps §4.3](Maps.md#43-optional-overture-data),
[§9](Maps.md#9-delivery-and-acceptance)), and stage O2, regional address lookup, with its evaluation ([§8](#8-o2-regional-addresses)).
The guides are [Maps › Overture](../../docs/tools/Maps.md#overture-places-and-buildings) and
[Maps › Overture addresses](../../docs/tools/Maps.md#overture-addresses).

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
| `parquet` 60.2.0 with `zstd` | 29 s | +1.26 MB | 33 | `zstd-sys` compiles the C zstd library: refused by [Build-Test, Native code](../../docs/Build-Test.md#native-code-and-c-libraries) ("No C libraries in new dependencies") |
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
- Docs: the Features table of [Build-Test](../../docs/Build-Test.md), the crate table of
  [Practices-Rust §1](../../docs/dev/Practices-Rust.md#1-crates-and-where-code-goes), [Maps](../../docs/tools/Maps.md).
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

## 8. O2: regional addresses

### 8.1 Evaluation

Measured on 2026-11-26 against release 2026-11-18.0 (schema 2.0.0) with this crate's reader, a dev build on macOS, for a
small box of a US metro and of a European city (and two more for coverage). The `addresses` collection is 64 files of
about 7.4 million rows and 256 row groups each, 475,239,461 rows in all; its columns are `id`, `street`, `number`,
`unit`, `postcode`, `postal_city`, `address_levels` (a list of `{value}`, the most general first), `country`, `geometry`
(a point), `sources`, `version` and `bbox`. **Nothing says what a point stands for**: the schema has no position kind (an
entrance, a building or parcel centre, a position on the road).

| Box (west, south, east, north) | Read | Points | street / number / unit / postcode | Levels | Source and license |
|---|---|---|---|---|---|
| Boston Back Bay (-71.09, 42.345, -71.065, 42.36), about 3.4 km² | 1 file, 4 of 256 row groups, 4.3 MB, 8 s | 29,925 | 100 % / 99.997 % / 84.5 % / 99.98 % | 2 (state, city) | `us/ma/statewide`, `LicenseRef-Proprietary` |
| Florence centre (11.24, 43.76, 11.27, 43.78), about 5.3 km² | 1 file, 4 of 256, 6.4 MB, 8 s | 21,481 | 100 % / 100 % / 7.4 % / **0 %** | 3 (region, province, comune) | `it/countrywide`, `CC-BY-4.0` |
| Austin downtown (-97.755, 30.26, -97.735, 30.275), about 3.2 km² | 1 file, 2 of 256, 3.8 MB, 4 s | 2,455 | 100 % / 100 % / 0 % / 100 % (and `postal_city`) | 2 | `National Address Database`, `LicenseRef-Public-Domain-US` |
| Amsterdam centre (4.88, 52.365, 4.905, 52.38), about 2.8 km² | 1 file, 3 of 256, 4.0 MB, 6 s | 28,822 | 100 % / 100 % / 74 % / 98.8 % | 1 | `BAG Light`, `CC0-1.0` |

What the points of one street and number are:

- **Boston**: 4,778 street-and-number pairs, 2,651 with several points, nearly all a point per unit (a unit's numbers,
  `3F`, `B`): spread median 0 m, 90th percentile 4.8 m, 99th 30 m (14 pairs over 50 m). Every number has its ZIP code.
- **Florence**: 14,040 pairs, 6,160 with several points and nothing to tell them apart (the units are 1,581 letters, the
  Italian *esponente* `12/A`; Florence's red business numbers are not marked): spread median 39 m, 90th percentile 148 m,
  2,650 pairs over 50 m. No point has a postal code.
- **Austin**: one point a number, about 760 a km², a tenth of Boston's: coverage there is partial or by structure only, which
  the data alone cannot tell.
- **Amsterdam**: points per unit (`A`, `B`, `H`), same-number spread 99th percentile 28 m.

**What O2 can promise.** For a region whose source the release has, a number and a street matched to the source's own
points: an **address match** (`GPP_ADDRESS_MATCH`) whose position kind is said to be not stated, with its source dataset,
license, GERS ID and release; the postal code agreeing where both have one; several points of one number offered as
choices; a street without the number, and a number found only without its suffix, as estimates. Matching is local:
nothing is sent at query time.

**What it cannot.** Completeness (Austin; Overture calls the theme alpha with partial coverage, the US included); a postal
code where the source has none (Italy), and so no postal-code directory from these points (Maps §4.3: the postal lookup
stays the fallback); a rooftop or entrance position (not stated); interpolation along a street (not done: a number not in
the data is the street's estimate); stable IDs (a refresh is compared by address, never by GERS ID); and one license (one
source is marked proprietary: each source's license is reviewed before its first run is active).

### 8.2 Design

- **Import** is O1's run for the theme `ADDRESSES` (`addresses`/`address`): the same discovery, ranged reading, staging,
  checks, license review, activation and rollback. Its rows are `moqui.maps.OvertureAddress` (internal `addressId`
  `<importId>-<n>`; `gersId` and the run's release an external reference only; the country; street, number, unit and
  postal code as written and as keys; postal city, locality (the last level), region (the first) and every level; the
  point; `positionKind`, empty while the schema says none; source datasets and licenses), written 2,000 a transaction,
  at most 1,000,000 a run (`TOO_MANY_ROWS`; about 115 km² at Boston's density of 8,800 a km²), indexed on `(importId, streetNorm,
  numberNorm)`. The run records its countries (`countryCodes`). A row with no ISO country, or with neither a street nor a
  number, is rejected. The theme's acknowledgement covers no source license at first (`MapsOvertureData.xml`), so a
  region's first run waits for review with what its sources name. Memory: O1's reader bounds, the batch, and the
  refresh comparison's two sets of addresses (a 64-bit digest and a mean point each: a hash table of about 50 MB at the
  cap, so about 100 MB for both, and for a moment half as much again while a set grows).
- **Refresh reconciliation.** Activation is O1's one transaction, so the index changes as a whole; the run before is kept
  for rollback. The run records what changed against the run active when it was read, by address (country, street,
  number, unit and postal code keys, each cut to the length its row keeps), never by ID: added, removed, moved more than
  25 m, unchanged (`changesJson`, shown in Configuration's runs). A change of the normalization tables between two runs
  changes keys, and shows as addresses removed and added. Nothing outside these rows is touched: a record's point, a user-confirmed point and
  Mantle are never read or written by an import, and a match is computed when asked, so a refresh needs no fix-up.
- **Matching** (`moqui-maps` `address.rs`, `address_service.rs`; `search#OvertureAddresses`):
  - *Parse*: parts by comma; the postal code by the country's shape (the last one, never the house number of a street
    written number-last); a unit after a unit word or `#` when a unit's name follows (a digit, or one or two characters;
    never `Ste` where French is written); the number first in a number-first country (US, CA, GB, IE, AU, NZ, FR and a
    few more), else after the street (with a letter or `bis`, `r`), and where a comma ends the street only the number that
    ends it (`Via 4 Novembre 12`, `Piazza 1 Maggio`); an ordinal (`5th`) is never a number; the first later part that is
    not a two-letter region or a country is the locality. A text of one part tries the longest street first.
  - *Normalize*: case and diacritics folded, dots dropped, the street words of the country's languages to one spelling
    (English with directionals, Italian, French, German, Spanish, Portuguese, Dutch; the shared forms `ave`, `blvd`,
    `st`, `pl`), German and Dutch `-straße`/`-straat`/`-platz` endings shortened, Romance, German and Dutch particles left
    out of a street of more words; numbers without spaces or a `/` before a letter; units without their designator; postal
    codes by the country's rules (`moqui_lib::geo::postal`).
  - *Retrieve*: the rows of the active runs by street key and number key (at most 200), then by the number's digits
    (`12` for `12/A` or `51r`), then the street (at most 500), in each country of the runs (or the country given); at most
    24 indexed queries a search, and 20 searches a second from one user (each server by itself).
  - *Rank*: number and street matched (100); postal code agreeing +20, differing −40; locality agreeing +10, differing −20;
    unit +5 or −3, a unit-less point before its units. Points of one address within 30 m are one candidate (Boston's 99th
    percentile), named without a unit or GERS ID unless it is the unit asked for (a unit asked for that is not there is
    not named), its sources those of all its points; candidates within 15 of the best are choices (`ambiguous`), the
    nearest the view's middle first. A number's letter the source keeps as the unit (Italy's `12/A`) is an address match;
    a number found only without its suffix is an estimate offered as a choice; a street is an estimate at its point
    nearest the middle of its points, with their extent, one per locality.
  - *Precision* (Maps §6.1): an address match is `GPP_ADDRESS_MATCH`, said as an address point whose kind the source
    does not state, only when the postal code and the locality asked for do not differ from the point's; a number and
    street whose postal code or locality differs is an estimate offered as a choice; the others `GPP_PLACE_ESTIMATE`;
    nothing is called exact.
- **The provider.** `MPK_GEOCODER_OVERTURE`, seeded disabled as `OVERTURE_ADDRESSES`: no URL, tested by a fixed query made
  from a row of its own data (an address match within 30 m of that row), testable and usable with outside requests off,
  asked only while a region's addresses are active and the server has the `overture` feature. find#Places asks it as any
  geocoder (one provider; its failure says so); a text it does not match is the postal area of the code read from it
  (`NO_ADDRESS_MATCH`, approximate) or nothing (`NO_MATCH`: choose the point). Browse asks it third, as any geocoder,
  each candidate with its own source.
- **Records.** A record whose adapter answers an address with no point (`address1`, `address2` when it names a unit, `city`,
  `postalCode`, `countryGeoId`) is matched by the local geocoder when one is enabled, as the dialog's own read (not
  `find#Places`, the Maps app's): one address match whose postal code and locality agree is the point shown
  (said to be found from the address and not saved), several are choices on the map with the postal area as the view,
  none is the postal area. Saving stays the adapter's `store#RecordLocation`: the record point, then the address match,
  then the postal area, then the point a user chooses.

### 8.3 Tests

- **Rust**: `address_tests.rs` (street keys across case, diacritics, abbreviations and particles in each language; number,
  unit and postal-code keys; US, Italian, Dutch and French texts read; one-part texts; units of one address as one
  candidate; two far points as choices; a unit asked for; a street estimate; a number without its suffix never an
  address match), `address_service_tests.rs` (the index over the in-memory facade: exact, ambiguous, `12/A`, estimated,
  missing and no data; a refresh compared by address with every ID changed), `overture_tests.rs` (the addresses reader on
  fixtures the `parquet` crate writes; the spec's fixtures `addresses-r1.parquet`, `addresses-r2.parquet` are goldens).
- **Regression** `framework/test/MapsOvertureAddresses.spec.js` (above, with `HttpFixtures`).
- **Acceptance** `framework/test/acceptance/OvertureAddressLookup.spec.js`: the latest real release for central Florence
  and Boston's Back Bay, recording time, bytes, rows and sample lookups with their precision. Run once, November 26, 2026,
  on release 2026-11-18.0 (schema 2.0.0), the release binary on macOS: Florence (11.245, 43.765, 11.265, 43.78) 6.7 s, one
  file, 3 row groups read and 253 skipped, 4.6 MB, 80,209 rows read and 15,336 kept, none rejected, held for `CC-BY-4.0`
  and activated once acknowledged; Boston (-71.09, 42.345, -71.075, 42.355) 5.9 s, 3 and 253 row groups, 3.4 MB, 93,977
  read and 15,880 kept, held for `LicenseRef-Proprietary`. The geocoder's test passed; lookups took 0 to 5 ms: `52
  Hemenway St, Boston, MA 02115`, `373 Commonwealth Ave Apt 503, …` and `250 Newbury Street #3F, Boston` one address match
  each (the unit's point); `Via dei Fossi 51, Firenze` one address match; `Via Maggio 15, Firenze` two address matches 110 m
  apart, ambiguous; `Via Por Santa Maria 53r` number 53 as an estimate; `Via dei Fossi 999` and `999 Hemenway Street` the
  street's estimate; `Via Inesistente 1, 50123 Firenze` and `1 Nowhere Street, Boston, MA 02116` nothing (`NO_MATCH`: the
  test database has no Italian or matching postal data for the fallback). Process memory is reported only on Linux, so it
  was not recorded here. Run again after the review's fixes (a differing postal code or locality an estimate, units, the
  street's point): the same answers, the street estimates now at one of the street's own points.
