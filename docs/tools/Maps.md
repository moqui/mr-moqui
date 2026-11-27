# Maps

Maps is the framework's map app, beside Dashboards, System and Tools (`runtime/components/base/tools`, mounted as
`/apps/maps` and `/qapps/maps`). It needs no BOSS, HOME, PASS or Mantle: those components add record adapters and
mount its dialog under their own routes. The design is [plans/archive/Maps.md](../../plans/archive/Maps.md); the map widget
(`map-view`, `m-map`) is in [Qvt-Client](../ui/Qvt-Client.md) and the terminal's `map` component in
[TUI](../ui/TUI.md#components).

A fresh installation makes no outside request: maps draw the Natural Earth country outline from this server until an
administrator enables a map provider, and no address is sent to a geocoder until an administrator enables one and a user
searches.

## The screens

| Screen | What it is |
| --- | --- |
| Browse (`Maps/Browse.xml`, `Browse.qvue`, `Browse.tui.json`) | The map filling the page, with optional browser full screen. One panel at a time beside it, a bottom sheet on a phone: Search (saved views and places, the permitted records of the record layers shown, postal codes and, when one is enabled, the geocoder's addresses), Layers (the basemap, overlays and record layers, and the user's own and shared overlays with Add overlay and, on one's own, Edit ([below](#overlays-and-places)); a layer that is not shown is named with Retry; a record layer cut to `maps@max-features` says so), Saved views (and Save view, which keeps where the map is, its basemap and its layers) and the selected feature's details: a place's, a record's (with its [links](#record-links)) or an overlay's feature, selected by a click or by Enter on the map for the feature at its center, with its label and description as plain text, its overlay and its coordinates (Save place; Remove place on one of one's own places). Live location: the shares of the user's own and those shared with them on the map, the Live location panel and Share my location ([below](#live-location)). The scale, attribution, coordinate readout and Locate me are the map's. A qvue screen: under `/apps` it opens at the same path under `/qapps`. In the terminal, the `map` component over `terminalMap`: a postal search's places (`s`: a code and its country, the local Geo data's approximate place), the saved places (own and, read-only, shared), the saved views' and the records' places as a list over the braille land mask, then the overlays, own and shared, the notices and the attribution, and under it the live location shares with Stop |
| Saved views (`Maps/Views.xml`) | The user's saved views and those others shared, each with its place, owner and whether its layers are all available; Share, Stop sharing and Delete for one's own, View map for any. Below them the user's overlays and saved places and, read-only, those shared with them, with Remove for one's own |
| Offline maps (`Maps/Offline.xml`, `Offline.tui.json`) | The user's offline map packages for the Android app and the desktop client, with Download, Update, Cancel and Delete, and New offline map: a region on the map, its zooms and contents, Estimate and Build ([below](#offline-packages)) |
| My location shares (`Maps/MyShares.xml`) | The user's own active location shares, for every signed-in user (`MAPS_SHARES`): who sees each, its end and its last position's age, Stop, An hour more and, for a Maps user, Open in Maps. Where the app bar's "Sharing location" chip goes ([Live location](#live-location)). A plain XML screen, the same in every client |
| Preferences (`Maps/Preferences.xml`) | The user's map profile, basemap, initial area, units, the layers shown and Restore my last view. Only what an administrator enabled is offered, and a profile's `allowedOverrides` decide what a preference changes. No location is asked for unless Locate me is pressed, and the last view is kept only while its restoring is on |
| Configuration (`Maps/Configuration.xml`, route key `mapsConfiguration`) | For `MAPS_ADMIN`: readiness, the outside-request policy and limits (read from the conf), map providers (connections of the `mapProvider` kind: a provider's page is `Configuration/ConnectionDetail` over `tools/template/ConnectionParts.xml`), profiles (enable, disable, make the installation default, the view projection) and layers; Archives (`Configuration/Archives.xml`, [below](#archives), PMTiles and COG layers); Projections (`Configuration/Projections.xml`, [below](#projections)); Geo data (`Configuration/GeoData.xml`, [below](#geo-data)); and Overture (`Configuration/Overture.xml`, [below](#overture-places-and-buildings)). Its settings index entry is `maps.configuration` |
| Local map (`Maps/LocalMap.xml`) | The dialog below |

Search reads saved places, the records of the record layers shown within the view on screen (each read through its
adapter) and the postal codes of the local Geo data ([Postal lookup](#postal-lookup)): a text that looks like a postal
code (a digit, letters, spaces and hyphens), of the country chosen beside the search (a list of the countries with postal
data, which then shows the country searched), or one named in the text (`US 02134`, `02134, US`) when it has postal data
and its rules read the rest as a code (so `MA 02134` is not Morocco while only US data is there). Otherwise the user's
country is tried first and then every other country with postal data whose rules read the text as a code; a code found
in more than one country lists each, labeled with its country, and says so. A postal result goes to the code's broad
area, not a close zoom, and a code with several places lists each as a choice. The user's saved places and those shared
with them come first ([Overlays and places](#overlays-and-places)), then the imported Overture places
([below](#overture-places-and-buildings)), and the configured geocoder's candidates last, only
when one is enabled ([Address and place search](#address-and-place-search)). Going to a result changes nothing in its
record.

## Access

Three artifact groups in `tools/data/MapsSecurityData.xml`:

- `MAPS_APP`: the app's root, for `MAPS_USERS` and `ADMIN`. `MAPS_APP_SERVICES` gives the same groups the preference,
  saved-view, overlay and place services, each acting on the signed-in user's own (a shared overlay is read through its
  file's drive access), the offline package services (`estimate#`, `create#`, `update#`, `cancel#`, `delete#Package`,
  `find#Packages`, `get#PackageAccess`: the user's own packages only), and the address and place search (`find#Places`).
- `MAPS_DIALOG`: LocalMap and its transitions, the effective configuration, the user's own preferences
  (`get#Preferences`), the postal lookup (`get#PostalLocation`, public reference data only), the map-source check (`MapSourceServices.get#SourceAccess`, [Archives](#archives)) and the adapter registry, for every signed-in user (`ALL_USERS`, always-allow, not inherited). Opening a record's map needs neither the Maps app, System
  nor any app's access.
- `MAPS_ADMIN`: Configuration, its `find#Configuration` and `store#ProfileSettings`, the archive services (`read#ArchiveHeader`,
  `read#CogHeader`, `register#ArchiveLayer`, `find#ArchiveLayers`) and the Geo data services, `ADMIN`'s;
  `MAPS_USERS` is denied it. It is asked for as the user's **own** grant: an allow of `Maps.xml` that an app grants its own
  group is inheritable and would reach Configuration and the services its transitions call, so Configuration's
  always-actions refuse a user without their own grant (`isPermittedOwn`, a 403 for every render and transition below it),
  `Maps.xml` shows its tab only to them, and the Geo data services, the importer and the archive and profile services ask
  the same of their own artifacts. An app that shows Configuration under its own root has its administrators hold
  `MAPS_ADMIN`. `read#ArchiveHeader` and `read#CogHeader` are not remote: only the archive services call them, after their check. A profile setting changes only with Configuration's update (view alone reads it), and a
  profile enabled or disabled and a change of the installation default are in the audit log.

`Maps.xml` has no `always-actions`: a render of LocalMap runs the always-actions of the screens above it without their
own check, so the root reads nothing.

## Configuration

Providers, layers and profiles are rows of `moqui.maps` (`MapProvider`, `MapLayer`, `MapProfile`), seeded with the
outline profile (`OUTLINE`, the default) and the OpenFreeMap provider and profile, disabled. To use OpenFreeMap: open its
provider under Configuration, run its test (which enables it when it passes), enable the OpenFreeMap profile and make
it the default. A provider is enabled only by a passing test of its configuration as it is; a profile drawn from a
provider that has not passed is not drawn, whatever its own state, and Configuration says why.

The installation-wide switches are attributes of the `maps` conf element (`MoquiDefaultConf.xml`), set in a conf file
or the environment (`moqui_conf__maps__external_requests`) and shown read-only under Configuration:
`maps@external-requests` (false: no outside basemap or layer is offered and the server makes no outside request for
maps), `maps@max-features` (the most features one record layer or viewport sends, and a user's overlay may have),
`maps@import-max-bytes`, `maps@overlay-max-bytes` (a user's overlay file, 10 MiB), and `maps@package-max-bytes`,
`maps@package-user-max-bytes`, `maps@package-concurrent-builds` and `maps@package-keep-days`
([Offline packages](#offline-packages)). A page
`Content-Security-Policy` stricter than the default needs each provider's host in `connect-src`, `img-src` and
`font-src`; the readiness checks name what to add. A map with a [COG layer](#cloud-optimized-geotiff) decodes it in module
workers of this server (the OpenLayers build's decoder entry under `libs/openlayers/`), so such a policy also needs `worker-src` (or `child-src`,
`script-src`, `default-src`) to allow `'self'`; the hillshade raster operation needs `worker-src blob:`. The decoder
worker's LERC and Zstandard decoders are WebAssembly (`WebAssembly.instantiate` of a `data:` URL): this server sends a
screen's policy only with screens, so the worker, a static file, has none of its own, but a proxy that adds the policy to
every response needs `'wasm-unsafe-eval'` in `script-src` and `data:` in `connect-src` for it. The `maps.archives`
readiness warns when an enabled COG layer meets a policy whose `worker-src` does not allow `'self'` (`CSP_WORKER`), and notes one
without `'wasm-unsafe-eval'` (`CSP_WASM`, informational).

**A provider through this server.** A provider's page has Through this server (`proxyRequests`) and Key parameter
(`apiKeyParameter`: a query parameter's name, `key` when empty, or `header:Name`). A provider with a key is drawn only so:
its key never reaches a browser. Its test asks as the proxy does, the key added, over https only (a keyed provider with
another URL is refused unless it is allowed the private network), its endpoint and each enabled layer of it that names
another URL of the provider, and records what they name as exact rules (`proxyPaths`, one a line): `doc` a style's,
TileJSON's or data file's path, `sprite` a sprite's path (with `.json`, `.png`, `@2x.json` or `@2x.png` after it), `tile`
a tile template (its `{z}`, `{x}`, `{y}` whole numbers, `{r}` an optional `@2x`), each with the query its document named,
never a directory or `/`; a document naming another host, or a path with a `.` or `..` segment (raw or %-encoded), fails
the test. A layer whose URL the last test did not reach is not drawn until the provider is tested again. The effective configuration then hands its layers
`/apps/maps/LocalMap/providerProxy/{providerId}/{the provider's path}` in place of the provider's URLs, and the proxy
(`MapSourceServices.get#ProviderAsset`, `MapProviders.js`) answers each GET only for a user who may be offered one of the
provider's layers (its `viewPermission`, and the basemap or a layer of an enabled profile whose `viewPermission` they hold),
while outside requests are on and the provider is enabled and tested as it is (decided once in 30 seconds for a user and
provider revision, `maps.provider.proxy`, so a change of a layer, profile or permission reaches it within that), within
its requests per second (each server's) and per day (the cluster's), else 429: one GET to the provider's origin (its
endpoint's, the first tile subdomain) at a path a rule matches, with that rule's query (the browser's own query is not
passed on), no redirect, ten seconds, at most 4 MiB, no internal address
unless the provider is allowed the private network, of a style, TileJSON, sprite or tile type. A style or TileJSON comes
back with each URL rewritten to the proxy's, any query parameter carrying the key dropped and the key's text, plain and
URL-encoded, taken out (glyph ranges are dropped: labels are drawn with web fonts). Its `Cache-Control` is always `private`: `no-store` for a cache policy of none, else its cache seconds or the
answer's own max-age, at most a day; a shared policy also keeps the rewritten documents of at most 256 KiB on this server
(`maps.provider.document`, by the provider's revision, the path and the rule's query). **A proxy is not complete location privacy**: the provider no longer sees the user's address,
but it still sees every area viewed (as this server's requests), and this server sees and could log them all.

## OGC services

A WMS, WMTS or WFS service is a map provider of the kind `MPK_WMS` (WMS 1.3.0), `MPK_WMTS` (WMTS 1.0.0) or `MPK_WFS`
(WFS 2.0.0, read as GeoJSON) (`MapOgc.js`, plans/archive/Maps.md §4.1). Its URL is the service's (`https://host/geoserver/ows`,
a MapServer URL with its own `map=` query, or a WMTS service's `WMTSCapabilities.xml`); an https URL, as every provider's.
The seed has the USGS National Map's topographic WMTS (provider `USGS_NATIONAL_MAP`, layer and profile `USGS_TOPO`; public
domain, the United States only), disabled until its provider passes a test and the profile is enabled.

**The test** asks the service's GetCapabilities (the URL with `SERVICE`, `REQUEST=GetCapabilities` and `VERSION` added; a
WMTS `.xml` URL as it is) as every provider's test asks: no credentials but a proxied provider's key, no redirect, at most
8 MiB, no internal address unless the provider is allowed the private network. The answer is read in Rust
(`MapServices.read#OgcCapabilities`, `moqui-lib` `geo/ogc.rs`) by the framework's XML reader, which refuses a DOCTYPE, an
entity declaration and an entity it does not know, so no DTD or external entity is read (a WMS 1.1.1 answer, which has a
DOCTYPE, is refused). A passing test records what a map draws (`capabilitiesJson`, at most 2 MB, the CRS and style lists
a WMS's layers inherit kept once): at most 2,000 layers (a WMS's named layers, with the CRSs, styles, bounds and time
dimension they inherit; a WMTS's layers with a matrix set in a CRS a map draws, each set kept once with at most 30
matrices, and the tile template; a WFS's feature types with a GeoJSON output), their titles, styles and formats (images:
PNG, JPEG, WebP, GIF, AVIF), and the CRSs of them a map draws: EPSG:3857 and EPSG:4326 under the names OpenLayers knows
(`CRS:84`, `urn:ogc:def:crs:EPSG::3857`, ...). A URL with credentials (`https://a@b/...`) is never the provider's. For a
provider browsers ask themselves the test also asks one sample (a small GetMap, a tile, one feature) with an `Origin` and
records whether it answers CORS: a WMS or WMTS that does not is drawn with plain images, which send the browser's
cookies of the provider's site to it (readiness warns, `PROVIDER_NO_CORS`), and a WFS that does not fails the test, since
a browser cannot read its features: reach either through this server. A layer added to the service since is offered
after the provider is tested again.

**Add OGC layer** (Configuration › Settings › Layers, `store#OgcLayer`) makes a layer of a tested OGC provider from that
record, choosing one of its layers, its style, format, CRS and matrix set (`get#OgcLayerOptions`, no request sent), with
an optional time (a value of the layer's time dimension), one image of the view instead of tiles (WMS), basemap (WMS,
WMTS) and a profile it is shown in: an overlay is added to the profile's layers, a basemap becomes the profile's basemap.
The layer is checked as a map draws it before it is kept. Its fields on `MapLayer` are `ogcLayers` (a WMS layer row may
name several, comma separated), `ogcStyles`, `ogcFormat`, `crs`, `ogcMatrixSet`, `ogcTime` and `ogcSingleImage`; a CRS
left empty is EPSG:3857 when the layer has it.

**What a map asks.** The effective configuration hands an OGC layer (`kind` `wms`, `wmts` or `wfs`, with its `ogc`
options) only from what the last test recorded, and only on the provider's own origin: an operation the capabilities
advertise at another host (a server behind a proxy often names its inner host) is asked at the service's own URL instead,
and a WMTS tile template of another host is not used (KVP at the service's URL is, when it has it). `m-map` draws a WMS
layer with `TileWMS` (or `ImageWMS` for one image), a WMTS layer with its tile grid made from the recorded matrix set (the
browser never reads the capabilities), REST by its template or KVP, and a WFS feature type as GeoJSON for the view's box
(the bbox strategy), at most `maps@max-features` features and no more than 1,000 at a time (`COUNT`, so a typical answer
fits the proxy's 4 MiB; a load that would make more starts the layer again with the view's features only), its features
selectable with plain-text details. It asks a provider that answers CORS without the page's credentials. A WFS layer is
read in its `srsName`: EPSG:3857 when the feature type has it, else CRS84 or the short EPSG:4326 (longitude first), else
the EPSG:4326 URN, whose box is sent latitude first; the GeoJSON is read longitude first, as RFC 7946 has it. The
provider's attribution is shown as text.

**Through this server.** A proxied OGC provider's test records its rules: `wms`, `wmts` and `wfs` the operation's path
with the service URL's own query, and `wmtsrest` a tile template's path (a template with a query, or with a placeholder
other than `{TileMatrix}`, `{TileMatrixSet}`, `{Style}`, `{Layer}`, `{TileRow}`, `{TileCol}` and `{Time}`, is not used
through this server: KVP is, when the service has it). The proxy forwards a request only for one of the provider's
registered layers the caller may be offered (enabled, its `viewPermission` held, and the basemap or a layer of an enabled
profile whose `viewPermission` they hold, or any basemap layer where such a profile allows the basemap override), with
that layer's own names, style, format and matrix set: a WMS GetMap's `LAYERS`, `STYLES` and `FORMAT`; a WMTS GetTile's
`LAYER`, `STYLE`, `FORMAT`, `TILEMATRIXSET` and a `TILEMATRIX` of the set (and a tile template's path with those values);
a WFS GetFeature's `TYPENAMES`, `OUTPUTFORMAT` and `SRSNAME` (decided once in 30 seconds for the user and the provider's
revision, `maps.provider.proxy`). The other parameters are checked by themselves: `CRS` one a map draws, `BBOX` four
plain decimal numbers (a WFS's with its `SRSNAME` after them), `WIDTH` and `HEIGHT` at most 4096, `TRANSPARENT`,
`TILEROW` and `TILECOL` whole numbers, `TIME` a time's characters, and a WFS `COUNT` of at most `maps@max-features`. Each
name may be given once, in any case; anything else, another request (GetFeatureInfo, a WFS Transaction), a filter, a
type or layer not registered or not the caller's, is refused (400; a template path that is no layer's is 404). A WFS
answer is passed on as it came, at most 4 MiB, refused when it carries the key. Editing a WFS layer (WFS-T) is not built.

## Archives

A PMTiles archive on this server draws a map with no outside request (plans/archive/Maps.md §4.1, §5). The seed has the local
Protomaps profile, `LOCAL_PROTOMAPS`, with its layers `LOCAL_PROTOMAPS_LIGHT` and `LOCAL_PROTOMAPS_DARK` (format
`MLF_PMTILES_VECTOR`), all disabled: they draw a regional [Protomaps extract](https://docs.protomaps.com/basemaps/downloads)
with the styles, sprites and fonts packaged under `/libs` (`protomaps/light.json`, `dark.json`, `protomaps/sprites/v4/`,
`mapfonts/`; webroot `build/libs/refresh.mjs`).

**Registering.** Place the archive where the resource facade reads it by range, then register its location under
Configuration › Archives (`MapSourceServices.register#ArchiveLayer`):

- in a drive (`dbresource://...`), read a megabyte chunk at a time from its content blob;
- in a git repository (`git://repository/path.pmtiles`), only as a **Git LFS object**: its `.gitattributes` keeps
  `*.pmtiles` in LFS (`setup#GitLfs`, which content repositories have by default). Any other git object is read whole for
  every request, so registering one is refused (`NOT_LFS`) and readiness names one by name. LFS content is in Postgres
  content blobs whatever the repository's storage;
- on the server's disk, read by seek, only under the runtime's maps directory (`runtime://maps/...`, or a `file:` path in
  `runtime/maps/`; anywhere else is `DISK_ROOT_ONLY`, as an operator's Geo data file is read only from
  `runtime://import/`). Its header is checked first, then its SHA-256 read once, with its size and modification time
  taken before and after (a change while it is read refuses it).

One action registers every layer drawn from an archive: Archives' form takes the layer and the others that draw it too
(`alsoLayerIds`), so the local profile's light and dark layers are registered from one location together (the form offers
them so while neither is), and Register again on a row registers every layer that names the same location (each named
layer is enabled; one that only names the location keeps its state).

Registering reads the archive's header and metadata (`read#ArchiveHeader`, two small reads, in Rust): version 3, the zoom
range (at most 26, the browser reader's), the bounds, every section after the header and within the file and the root
directory in its first 16 KiB, tiles of the layer's kind (vector tiles gzipped or not compressed, images not compressed
again), directories gzipped or not compressed. It records the revision maps read: on git and in a drive the
current version (`sourceVersionName`), so a later write changes nothing a map reads until the layer is registered again,
and the content address of that version (`sourceRevision`, `sourceTag`, `sourceSize`: an LFS oid or a content blob's
SHA-256), with the drive the location is under then (`driveId`, shown in Configuration), the zoom range and bounds, and the archive's attribution when
the layer has none; it enables the layer, and a profile still at the seed's world view starts at the archive's center.
Enable the profile and make it the default on the same page. To replace an archive, write the new file to the same
location and register it again: a map drawing the old revision is refused it (409) and loads its configuration again,
so no map mixes the two. The readiness provider `maps.archives` says when a newer revision is stored but not registered
(`NEWER_REVISION`), and when the registered one is gone, or a file on disk has changed in place (a change of its size or
modification time, which also makes the map-source transition answer 409 until it is registered again). Its
`maps.archive.space` check warns (`ARCHIVE_SPACE_LOW`) when the filesystem an archive is on has less free room than the
largest archive on it once more (the old revision is kept while maps move): the runtime's for one on disk, the database's
for one in a drive or git when its content database is the embedded one. While the scheduled backup runs, the filesystem
its directory is on (a watched one: the database's, the runtime's, ...) needs the database's archives once more for the
next local backup. It reads the status map's filesystems; an external database's room is not measured, nor a backup
directory outside the runtime, and off Unix nothing is (`ARCHIVE_SPACE_EXTERNAL_DB`, `ARCHIVE_SPACE_UNREAD`).

**Reading.** A map reads an archive at `/apps/maps/LocalMap/source?layerId=...&revision=...` (the effective configuration
hands that URL to a user it lets read the archive, and to no other). Every request, each range included, checks the
caller again with nothing written (no `ResourceRecent`): the layer enabled and registered, its `viewPermission`, its
provider enabled, and the storage's own access, decided at that request: view on the drive the location is under, at its
path (a member, or a share of the file or a folder above it; the drive is found by its root through the entity cache, and
its shares by the drive's indexed ID), else for a `git://` location outside every drive view of its repository
(its own `AT_GIT_REPOSITORY` grants, as the git server decides a clone). A revoked share or a removed member is refused at the next request; a revision that is no
longer the registered one is 409. The bytes go out with the
registered tag as a strong `ETag` and `Cache-Control: private, max-age=<resource-facade@pinned-max-age>, immutable` (a
day): after a share is revoked, the ranges a browser already has stay readable in that browser until the max-age passes.
**A location outside every drive and repository has no access of its own**: a file under `runtime/maps/` or a
`dbresource://` path no drive holds is readable by every user who may see the layer (its `viewPermission`, when it has
one). Registering one says so, and Archives says it beside the form. Keep an archive only some users may read in a drive
shared with them, or give its layer a `viewPermission`.

**A Cloud Optimized GeoTIFF** is registered the same way ([below](#cloud-optimized-geotiff)): its header is read instead
of a PMTiles one, and the map-source transition serves its ranges with the same check and pinning.

**Tiles one at a time.** A client with no PMTiles reader (OpenLayers' XYZ vector tile source, a terminal, another
program) reads one tile at `/apps/maps/LocalMap/tile/{layerId}/{revision}/{z}/{x}/{y}` (the `y` may end in `.mvt`,
`.pbf` or `.png`; the revision is the one in the layer's `source` URL). It is the same facade and the same check as the
map-source transition (`MapSourceServices.get#ArchiveTile` over `check()`), on every request and writing nothing; a
revision that is no longer the registered one is 409. The tile is read in Rust (`read#ArchiveTile`, `moqui-tools`
`maps_tiles.rs` over `moqui-lib` `pmtiles.rs`): the header, the root directory and the leaf directories the lookup goes
through (run lengths included), each by range, kept by the archive's tag (a content address, so a new revision is a new
key; at most 256 directories and about two million entries in all), then the tile's own range. It goes out with its type
(`application/vnd.mapbox-vector-tile`, `image/png` and so on), gzipped as stored with `Content-Encoding: gzip` when the
client's `Accept-Encoding` takes it and decompressed when not (at most 16 MiB), a strong `ETag` of the registered tag,
the tile ID and the encoding (304 for `If-None-Match`), `Vary: Accept-Encoding` and the same `Cache-Control: private,
max-age=<pinned-max-age>, immutable`. No such tile is 204 for vector tiles (an empty tile) and 404 for images. The reader
is our own rather than the `pmtiles` crate (MIT or Apache-2.0), whose reader is async over its own backends (Tokio, an
HTTP client, memory maps) and does not fit the facade's synchronous `read_at`; the directory format is a few varints.

**Limits.** An upload is at most 2 GiB (`webapp-list/webapp[webroot]/web-protection/requests@max-upload`), a Git LFS object
5 GiB (`git-server@lfs-max-object-size`), and a drive has its own `quotaBytes`, which a replacement needs room in for both
revisions until the old content is collected; Archives shows each against the archive's size. A larger regional or
planet archive is placed on the server's disk and registered there, not uploaded. Where the per-client web protection is
on, its `all` rate limit (50 requests a second, a burst of 500) counts every range a map asks for, for everyone behind one
address together. A pan of six steps and two zooms asked 17 ranges in about four seconds (`MapsLocalOnly`), and a
range the browser has read is not asked again while it keeps it.

## Cloud Optimized GeoTIFF

A raster of measurements or imagery (an elevation model, a satellite scene, a scanned map) is a layer of the format
`MLF_COG` (P4c), drawn by the browser itself with geotiff.js and OpenLayers' WebGL tiles, reading only the ranges of the
tiles in view (plans/archive/Maps.md §4.1). It is either:

- **a registered resource**, registered as a PMTiles archive is (Configuration › Archives, `register#ArchiveLayer`: a drive,
  a Git LFS object or `runtime://maps/`), its revision pinned and read through the map-source transition with the same
  access check on every range ([Archives](#archives)); or
- **a provider's URL** (a layer with a `sourceUrl` on its provider's own origin and no location): registering it reads one
  range of its first 64 KiB from there (no credentials, no redirect, ten seconds, no internal address unless the provider
  is allowed the private network), which must answer 206 (`NO_RANGES`) and allow other origins by CORS (`NO_CORS`: the
  browser reads it directly, with no credentials of the page); it is drawn only while its provider is enabled and tested
  and outside requests are on, as any provider layer, never through this server's proxy, and nothing of it is pinned.
  Its provider is a raster-tiles provider: for a host that serves only COGs (a bucket), give it the URL of one of them
  (a `.tif` or `.tiff` path with no tile placeholder), and its test reads that COG as registering one does (the range,
  CORS and the header; a failure names its reason, `NO_RANGES`, `NO_CORS`, `COG_INVALID` and so on).

**The header** is read in Rust (`MapSourceServices.read#CogHeader`, `moqui-lib` `geo/tiff.rs`), never a tile: the TIFF or
BigTIFF header, in either byte order, and its image directories, at most 64 of them (one seen twice is a loop, refused), at
most 1024 entries each, at most 1 MiB read past the first 64 KiB, every offset and count checked against the file's size
before it is read, and only the small tags' values read (a tile's offsets are counted, not read). A map draws it when it is
tiled (16 to 4096 pixels, a multiple of 16; a file in strips is not cloud optimized, `gdal_translate -of COG`), compressed
as a browser decodes (none, LZW, JPEG, Deflate, PackBits, LERC, Zstandard, WebP), of 8, 16, 32 or 64-bit samples of one
size, georeferenced by a pixel scale and a tiepoint or an unrotated transformation, in a CRS its GeoTIFF keys name by an
EPSG code (a user-defined one is refused) that is WGS84, Web Mercator or a [registered projection](#projections), and, past
2048 pixels, with overviews; each reason it is not is said (`COG_INVALID`). Registering records its CRS (`crs`), its
bounds in WGS84 (its edges converted) and what its header holds (`rasterJson`: bands, sample type, compression, size,
tiles, overviews, GDAL's no-data and band statistics), with its default band settings: an 8-bit image of three bands or
more as red, green and blue, else its first band through the grey colormap, stretched between its statistics or its
type's range.

**Bands** (Browse › Layers, Bands beside a COG layer): which bands are red, green and blue, or one band through a colormap
(grey, viridis, terrain, magma), the stretch's minimum and maximum, and a no-data value drawn transparent. Applying them
draws them at once and keeps them as the user's own (`store#Preferences` `rasters`, each band checked against the layer's
count, `RASTER_INVALID` otherwise); Its defaults goes back to the layer's. A saved view keeps the settings of the COG
layers it shows. The effective configuration hands each COG layer its bands, sample type, CRS and settings (the user's,
else the layer's).

**In the browser.** The raster and projection part of the OpenLayers bundle (its raster entry under `libs/openlayers/`: OpenLayers'
GeoTIFF source and WebGL tile layer, geotiff.js and its decoders, proj4) is imported only by a map that needs it (a COG
layer, or a view or layer in a registered projection); every other map loads the main bundle alone. They share one
OpenLayers (an ES module build split into chunks), and the decoders load in module workers only for a tile of their
compression. A map without WebGL names the layer as not drawn. The bundle's reviewed hosts are in `refresh.mjs`
(`OPENLAYERS_HOSTS`). Every file of the build is named by its hash and kept a year, immutable; `ol-map.json`, the one file
of a fixed name, names its entries and is revalidated each time a page loads the map, so an upgrade never pairs an entry
with another build's chunks. spatialreference.org, which `ol/proj/proj4` would ask for a CRS it does not know, is replaced by a
refusal when the part loads, so it is never asked ([Projections](#projections)).

## Projections

The registry of projections (P4c) is `moqui.maps.MapProjection`: a code (`EPSG:32633`), a label, a PROJ.4 definition,
the authority's axis order (`neu` for a CRS whose coordinates are northing first, as EPSG:3035 and a geographic CRS's), its
area of use in WGS84, its extent in its own units for a view drawn in it, and notes on its datum shift and its source. The
seed (`framework/data/MapsProjectionData.xml`) has WGS 84 and Web Mercator, NAD83, the 120 WGS 84 UTM zones and NAD83's
zones 1 to 23, the British National Grid (EPSG:27700), Lambert-93 (EPSG:2154), LAEA Europe (EPSG:3035), the US Albers
(EPSG:5070) and the two polar stereographic projections NSIDC and NASA GIBS draw (EPSG:3413, EPSG:3031): the EPSG Geodetic
Parameter Dataset's parameters (IOGP, whose terms let its data be used and redistributed, naming it) written in PROJ's
syntax. An administrator adds a row for another CRS. A definition is never fetched: the effective configuration hands a
map the rows it needs (`projections`: its view's, its COG layers'), which m-map registers with proj4 (a UTM zone
OpenLayers makes itself is left to it), and a lookup of any other code is refused. **Configuration › Projections** (`MAPS_ADMIN`, the
caller's own grant; `find#Projections`, `store#Projection`) lists the rows with whether the server reads each, and adds,
changes, enables or disables one: its definition must be PROJ.4 parameters a browser registers (no URL, no grid file but
`+nadgrids=@null`, `DEFINITION_INVALID`), and the answer says whether the server reads it too (`check#ProjectionDefinition`,
`geo/proj.rs`): one it does not is drawn by maps only, and an overlay in it is refused with that reason rather than as an
unknown CRS. A definition whose ellipsoid is not WGS 84's or GRS 1980's must give its shift (`+datum`, or `+towgs84`, zeros to
take it as WGS84 as written); a parameter its method does not use is refused, so a row is read as written or not at all.

**On the server** (`moqui-lib` `geo/proj.rs`) a row is read for the methods a user's overlay is converted with:
geographic, Mercator, transverse Mercator and UTM (Krüger's series to the sixth order, millimetres within a zone),
Lambert conformal conic, Lambert azimuthal equal-area, Albers equal-area and polar stereographic, each ellipsoidal and
tested against the EPSG guidance examples. A datum whose shift to WGS84 is zero (`+datum=NAD83`, `+towgs84=0,...`: NAD83,
ETRS89, RGF93) is taken as WGS84, which differs from it by about 1 to 2 metres today; a `+towgs84` of three or seven
parameters is a Helmert shift through geocentric coordinates (EPSG method 9606): the British National Grid's is the
Ordnance Survey's approximation, about 5 metres (the OSTN15 grid, good to centimetres, is not used). A definition that
needs a grid (`+nadgrids`, NAD27), another method or a prime meridian other than Greenwich is not used on the server, and
logged by its code. `proj4rs` (MIT or Apache-2.0) would read any PROJ.4 text, but these methods are a few hundred lines
with their own tests, as the overlay readers are the framework's own.

**A profile's view** (Configuration › Settings › Map profiles, View projection; `MapProfile.viewProjection`) is drawn in
Web Mercator (EPSG:3857) unless it names EPSG:4326 or a registered projection (a polar one for a polar basemap, a national
grid for a national WMS). A basemap or layer of vector tiles (a MapLibre style, a vector PMTiles archive, an overlay
simplified into tiles) is not reprojected, so it is drawn only in EPSG:3857: a profile whose basemap is one is refused in
another projection with that reason (`PROJECTION_INCOMPATIBLE`, and the profile setting is refused so), another basemap of
that kind is not offered, and such a layer is named as not drawn. Rasters (XYZ, WMS, WMTS, a raster archive, a COG) are
reprojected by OpenLayers, and vector files (GeoJSON, WFS, a user's overlay) drawn in any. A saved view may name a
registered projection.

## Overlays and places

A user adds a geographic file (the formats below) as a personal overlay with Add overlay under Browse › Layers: an
upload, kept as it was sent in `Maps/overlays` of their personal drive ("My files"; a name already there gets "name
(2).ext"), or a file chosen with the drive picker (basic-comms' `FilePickerServices` through Browse's `pickerData`,
`pickerFolder` and `pickerSearch`: the drives they may open, their folders, recent files and a search by name, listing only
files of those formats), which stays where it is (`MapServices.store#Overlay`, `MapOverlays.js`). The file is read before
anything is kept (`MapSourceServices.convert#Overlay`, in Rust: `moqui-lib` `geo/overlay.rs` and a reader a format in
`geo/overlay/`), an upload as its bytes, so a binary format arrives whole:

| Format | Extension | What is read |
| --- | --- | --- |
| GeoJSON | `.geojson`, `.json` | Features, a FeatureCollection or a bare geometry (a GeometryCollection's members each a feature); a `.json` that is TopoJSON or EsriJSON is read as that |
| TopoJSON | `.topojson` | Every object's geometries, lines and rings stitched from the shared arcs through the topology's transform; the positions stitching makes are bounded in all (`overlay-max-bytes` / 4), since features may share an arc any number of times |
| EsriJSON | `.esrijson`, `.esri.json` | A FeatureSet (or one feature): points, multipoints, polylines, polygons (clockwise outer rings, counterclockwise holes), `attributes` as properties; curves are not read |
| GPX | `.gpx` | Waypoints, routes, tracks |
| KML | `.kml` | Placemarks with their geometries (MultiGeometry, gx:Track); network links, image overlays, styles and icons counted, never fetched |
| GML | `.gml` | GML 3.2 simple features (3.1 and 2 too), as a WFS answer or an OGR export has them: each child of a `featureMember`, `featureMembers` or `member`, its first geometry (points, lines, rings, polygons, their multi forms, a Curve of line segments, a Surface of polygon patches; `pos`, `posList` by `srsDimension`, GML 2's `coordinates`) and its simple properties; arcs and other curves are not read |
| WKT | `.wkt` | One geometry a line (empty and `#` lines passed over), with no label: POINT, LINESTRING, POLYGON, their MULTI forms and GEOMETRYCOLLECTION, Z, M and ZM dropped, EWKT's `SRID=n;` |
| WKB | `.wkb` | OGC, ISO and EWKB geometries one after another, binary, or hex text one a line; every count checked against the bytes left before anything is kept |
| Delimited text | `.csv`, `.tsv` | A header row, then a feature a row (comma, semicolon or tab, quoted as RFC 4180 has it; UTF-8, or read as Windows-1252, noted, when it is not): the geometry from a column named WKT, geometry, geom, the_geom, shape or WKB (WKT, EWKT or hex WKB), or from latitude (lat) and longitude (lon, lng, long) columns |
| Shapefile | `.zip` | The zip's one `.shp` record by record (the `.shx` is not needed), the `.dbf` record of each as its properties (dBASE III or IV; a dBASE 7 table is not read, noted), decoded as its `.cpg` says (UTF-8 or Windows-1252; with none, Windows-1252 where a field is not UTF-8; a field of another code page that is not UTF-8 is left out and counted, not mis-read), the `.prj` (WKT1, or WKT2's GEOGCRS) for its CRS: points, multipoints, polylines, polygons, with Z and M dropped; at most 64 entries, inflated to at most four times `overlay-max-bytes` in all and a hundred times each member's compressed size; one geometry of at most 10,000 rings, whose holes are grouped within a bounded work |

A **GeoPackage** (`.gpkg`) is not read, and says so: it is an SQLite database, and the server links no SQLite reader
(`rusqlite` with its bundled C SQLite is in the browser and client crates only); export its layer as GeoJSON or a zipped
shapefile. Each reader is our own: the formats are small (a few fixed headers, a grammar, an XML or JSON shape), each
reader's bounds are its own to keep, and the `shapefile` crate (MIT, with `dbase`) and GDAL's bindings would add
dependencies for what a few hundred lines do. The rules every format shares:

- At most `maps@overlay-max-bytes` (10 MiB, within `maps@import-max-bytes`), and at most a position for every four of its
  bytes in all its features, whatever the format (only a format whose features share parts could pass it), and at most 4 MiB of characters as the
  GeoJSON it is served as: an overlay is drawn whole in the browser. At most `maps@max-features` features (5000 when that
  is 0, which turns only record layers off); a file with more is refused, not cut. A label or description is read from at
  most eight times its kept length, so a large field costs no more than a short one.
- Coordinates WGS84 (GPX and KML always are), Web Mercator or a [registered projection](#projections), converted: a GeoJSON or TopoJSON file's pre-2016 `crs`, an
  EsriJSON `spatialReference` (WKID 4326, or 3857, 102100 and Web Mercator's other codes; none is WGS84, noted), a GML
  geometry's `srsName` or the nearest one above it (EPSG:4326 as a URN or an `opengis.net/def/crs` URL is latitude first,
  as GML 3.2 has it, the short `EPSG:4326` and `CRS:84` longitude first; a member geometry's own must be its geometry's
  CRS; none is WGS84 longitude first, noted), an EWKT or EWKB SRID (none is WGS84, noted), or a shapefile's `.prj` (a
  GEOGCS of the WGS 84 datum, or Web or Pseudo Mercator, or a registered CRS it names by its EPSG authority, by ESRI's name
  for it, `WGS_1984_UTM_Zone_33N`, `British_National_Grid`, or by its parameters; with no `.prj` the coordinates are read as
  WGS84 when the file's box is within degrees). A registered CRS is converted to WGS84 by its row (a GML URN or URL of one
  is read in its authority's axis order, its short name easting first), a position outside its projection rejecting its
  feature. Any other CRS is refused by its name (`CRS_UNSUPPORTED`). The CRS recorded is the file's first (`sourceCrs`),
  with what converting it did (`crsTransform`: the method, the ellipsoid, the datum shift and its accuracy).
- Each feature's geometry is checked by itself (positions in range and numbers, a line of two positions or more, a ring
  of four or more, closed when it was not): one that fails is left out and counted, with the first ten named, and the rest
  are kept. A file with none left is refused.
- Sanitized: a feature keeps its label and description as plain text (markup, scripts and styles removed); a GeoJSON
  feature's other properties and a KML placemark's extended data are not kept. Nothing a file points to is fetched: KML
  network links, image overlays, styles and icons are counted in the notes and never followed, and the XML reader refuses
  a DOCTYPE or an entity declaration. In the browser the overlay is plain GeoJSON, drawn by `m-map` with its own style. A
  `<` that starts no tag (`a<b`) stays in the text, which is shown as text, never as markup.

A refusal says why and keeps nothing. The overlay is a `moqui.maps.MapOverlay` row of the user's (its file's drive and
path, format, feature and rejected counts, bounds, CRS and notes), at most 100 a user. Browse draws it from its `overlay`
transition (`/apps/maps/Browse/overlay?overlayId=...`, `get#OverlayContent`), which checks the reader's access to the file
in its drive on every request, the owner's as anyone's, writes nothing, and answers sanitized GeoJSON, cached by the
file's version (`maps.overlay.geojson`, 16 entries); a file that grew past 4 MiB of GeoJSON since it was added is a 413,
remembered until it changes. Remove takes an overlay off the user's maps; its file stays in their files. A
saved view keeps the overlays shown (`overlay:ID`), and a reader of the view sees each only while its file is readable to
them.

**Large overlays.** A file past the 4 MiB of GeoJSON a map draws at once, or past `maps@max-features`, is refused with
an offer: Add overlay then shows **Simplify on the server**, which adds it again with `generalize`. The file is kept (an
upload in `Maps/overlays`, a picked file where it is, which the user must be able to add a file beside:
`TILES_NOT_WRITABLE` otherwise), read and sanitized as any overlay but with up to 250,000 features, and a run of the
`MapOverlayTiles` ServiceJob starts as the user (`MapSourceServices.build#OverlayTiles`, `MapOverlays.js`). It cuts the
file into vector tiles in Rust (`generalize#Overlay`, `moqui-tools` `maps_simplify.rs` over `moqui-lib` `geo/tiles.rs`,
`geo/simplify.rs` and `geo/mvt.rs`): projected to Web Mercator, and for each zoom from 0 simplified by Douglas–Peucker to
half a pixel of a 256-pixel tile, clipped to each tile it touches with a small buffer (Sutherland–Hodgman for rings,
Liang–Barsky for lines), points thinned to one a pixel, written as one MVT layer `overlay` with each feature's label and
description. It stops at the first zoom from 12 that keeps every position and point (a map draws that zoom's tiles larger
past it), at zoom 14, or before a zoom that would pass a limit, the note on the row saying which. Before a zoom is made
its cost is counted without making it: the feature and tile pairs it would clip (two million at most; a feature's tiles
counted from its own points and segments, not only its box, so points far apart or a line across the antimeridian count
the tiles they touch), the label and description text those pairs carry (256 MiB), and the positions clipped over the
whole run (100 million); then the tiles allowed (50,000) and their compressed bytes (64 MiB). A zoom is made a tile at a
time, each tile's features clipped, encoded, compressed and dropped before the next, so a run holds the file's features
and their simplified shapes (a few times the file's size, which `maps@overlay-max-bytes` bounds for the run as for Add
overlay), the zoom's pairs and the tiles made. A simplified ring may touch itself and a clipped one runs along the tile's
edge: enough for display, not kept as data. The tiles (gzipped) are written as a PMTiles archive beside the file,
`name.tiles.pmtiles`, through `FileServices.upload#Resource` (the drive's write check, quota and versions), replacing only
this overlay's own earlier archive (another file of that name, such as another reader's archive of a shared file, is kept
and the new one named "name.tiles (2).pmtiles"); its version and content address are recorded on the overlay's row, and
for a drive on the server's disk, which has none, its SHA-256 with its size and modification time, each checked on every
tile. A drive that keeps it in git outside Git LFS is refused (every tile would read the whole archive). Removing the
overlay, or a failed run, leaves the archive in the user's files, as removing an overlay leaves its file. Browse then draws it from the tile transition above, as `overlay:ID` with the archive's
revision, which checks the reader's access to the overlay's **file** in its drive on every request, as the overlay
transition does: a share of the file reaches its tiles, and a revoked share is refused at the next request. Layers shows
the run's progress (written to the row in a transaction of its own after each zoom and at most once a second within one;
Browse asks for it with a light call, `get#OverlayTilesStatus`, waiting twice as long each time up to a minute and not
while the page is hidden) with **Stop simplifying** (`cancel#OverlayTiles`: the run reads it when it writes its progress
and stops), and the user is notified when it ends (topic `MapOverlayTiles`). The row records its job run: a run that
ended without saying so (its run gone or ended, or older than the job's hour-long transaction) is ended by Stop
simplifying or by adding the file again, and the outcome is written in a transaction of its own. Adding the file again simplifies it again; until then a change of the file does not change its
tiles. The MVT writer, the simplification and the clipping are our own, not the `mvt` or `geo` crates (MIT or
Apache-2.0), which bring a protobuf runtime or a dozen geometry crates for these few functions.

**Editing.** Edit, beside one of the user's own overlays under Layers (not one shared with them, not one simplified into
tiles; their saved places too), opens the Edit panel and edits the overlay on the map (`m-map`'s editing, `WebrootMap.js`):
the overlay's features in a layer of their own while the overlay is hidden, read through Browse's `overlayEdit`
(`MapServices.get#OverlayEdit`: the sanitized GeoJSON a map draws and the version of the file). Draw makes points, lines
and areas (a saved place is a point only) with OpenLayers' Draw; a click selects a feature, whose vertices are then dragged
(Modify) and snap to the other features (Snap), and the panel gives its label and description (plain text) and Delete. From
the keyboard, with the map focused: Enter selects the feature at the center, or, while drawing, adds a vertex there (a
point at once; Finish the shape ends a line or an area), Delete removes the selected feature and Escape lets go. Moving a
vertex needs a pointer. Save (`saveOverlayEdit`, `store#OverlayEdit`) sends the FeatureCollection with the version it
started from: the overlay's row is locked, and a file changed since is refused (`STALE_VERSION`, with Open it again; a
write to the file by other means in the moment between that check and the save is not told apart, and the drive keeps
both versions); the edit is read by `convert#Overlay` as any overlay (a geometry that is not valid refuses the save,
`INVALID_GEOMETRY` with its notes; only labels and descriptions are kept, as plain text). Where it goes is decided from
the file as it is at the save: the overlay's own file gets the edit as its new version (`FileServices.save#Text` as the
user: the drive's write check, quota and versions) only when nothing of it would change but the edit, that is a GeoJSON
file that is exactly what the map makes of it (compared as JSON), the user's saved places, or an edited copy made here.
Any other file (another format, a `.json` that is TopoJSON or EsriJSON, GeoJSON with a `name`, other properties, markup,
a declared CRS or anything the map does not keep) stays as it is, and the edit is written beside it as
`name.edited.geojson` (or the next free `name.edited (2).geojson`; `FileServices.upload#Resource` with `keepBoth`, so no
file there is replaced), which the overlay draws from then on, its row keeping the original as `sourcePath` with what
reading the original found (its CRS and notes). A reader of the original through a share of that file alone does not
read the edited copy; a share of the folder reaches both. An overlay with no feature left is refused (remove it
instead); places may all be removed. A WFS layer is not edited (WFS-T, plans/archive/Maps.md §4.1).

**Saved places** are named points in one GeoJSON file, `Maps/places.geojson` of the personal drive: Save place on the
selected place or search result (`store#Place`, written through `FileServices.save#Text`, so the drive keeps each change
as a version), Remove place on one of one's own (`delete#Place`). The file is the user's overlay "My places", and Browse's
search finds its places by name, as user-confirmed points. A save locks the user's places row first, so two saves at once
are one after the other, and the file is read only up to `maps@overlay-max-bytes` (a larger or broken one is refused,
never overwritten).

**Sharing.** A collection is shared the way any file is, with a share of the file, or of the `Maps` folder, in Files
(`ResourceShare`, to a user, a group or a party): there is no other access model. An overlay or places file shared with a
user lists, read-only and named by its owner, in their Layers and Saved views, and its places in their search; revoking
the share takes it away at the next request. A drive's members read its files' overlays the same way.

## Address and place search

A geocoder is a map provider of a geocoder kind (`MapProviderKind` under `MPK_GEOCODER`):

| Kind | Service | Precision of its points |
| --- | --- | --- |
| `MPK_GEOCODER_CENSUS` | The US Census geocoder's one-line address search (`.../geocoder/locations/onelineaddress`, the current benchmark); US addresses only | An address matched, placed by street-range interpolation (`GPP_STREET_RANGE`): a position along the street's address range, never a rooftop or an entrance |
| `MPK_GEOCODER_NOMINATIM` | A Nominatim server's `/search` (self-hosted: the public OSM endpoint's policy forbids an application's searches beyond one a second and autocomplete) | An address point `GPP_ADDRESS_MATCH`; a postal code `GPP_POSTAL_AREA`; a street or a place `GPP_PLACE_ESTIMATE` |
| `MPK_GEOCODER_PHOTON` | A Photon server's `/api` (self-hosted) | As Nominatim's, by the result's type |
| `MPK_GEOCODER_OVERTURE` | This server's own index of the imported Overture address points ([Overture addresses](#overture-addresses)): no URL, nothing sent | A number and street matched, an address point (`GPP_ADDRESS_MATCH`) whose position kind the data does not state; a number found without its suffix, or a street, `GPP_PLACE_ESTIMATE`; a text it lacks, the postal area of its code (`GPP_POSTAL_AREA`) |

The seed has the US Census geocoder, `CENSUS_GEOCODER`, disabled. It is enabled as every map provider is: its test
(System › Integrations or Maps › Configuration) sends a fixed search, a public address for the Census geocoder and a city
name for the others, never a user's text, and a provider is enabled only after a passing test of its configuration as it
is. While `maps@external-requests` is false nothing is sent. The test and the readiness check name the public demo servers
(`nominatim.openstreetmap.org`, `photon.komoot.io`), whose policies allow an application very little.

`MapServices.find#Places` (`MapGeocoders.js`) asks one provider, the enabled, tested geocoder first by `sequenceNum` that may
be asked now (an outside one while outside requests are on, the local one while a region's addresses are active), for a
text. The local geocoder answers from this server's rows ([Overture addresses](#overture-addresses)); an outside one is asked by
one GET with no credentials, following no redirect, ten seconds, reading at most 1 MiB of the answer whether or
not it says its length (`text(maxBytes)` of the RestClient), to no internal address unless the provider is allowed the
private network, and only when its allowed operations are empty or name geocode or search. Each candidate has its label, point, match kind (address, street, postal,
place), precision in words and source; several candidates are ambiguous and shown as choices. A failure says so beside the
local results (`PROVIDER_FAILED`) and no other outside service is asked; the log names the provider, never the text.
The provider's policy is set beside its URL, and a change of it needs no new test. This server keeps an answer only when
its cache policy is `shared` (`none` or empty keeps nothing, and `private` means the user's browser alone, so nothing on
the server), for its cache seconds or, when empty, the answer's own `Cache-Control` max-age (at most a day; nothing with
`no-store`, `no-cache` or `private`), under a digest of the request, never its text. Its requests per second are enforced
(`RATE_LIMITED`, not queued), 1 for Nominatim and Photon when empty: a request claims its slot atomically, on each server
by itself, so a cluster of n servers may send n times the rate. Its requests per day, when set, are the most searches it is
sent in a calendar day (UTC), refused past that (`DAILY_LIMIT`): a search claims one by locking the provider's
`moqui.maps.MapProviderDayCount` row in a transaction of its own before it is sent, so the servers of a cluster share the
day's count (a cached answer and the connection test claim none). Browse's search asks the geocoder third, after saved places and records and the postal codes, only
when one is enabled, and says which service was asked and its attribution. Opening a map or the local-map dialog never
calls it: an address leaves the server only when the user searches.

## The local-map dialog

An app opens LocalMap beside an address or coordinate field with View map, as a `dynamic-dialog`; its id is
`LocalMapDialog`, or another passed as `dialogId`:

```xml
<dynamic-dialog id="LocalMapDialog" button-text="View map" icon="sym_o_map" title="${partyName}"
        transition="LocalMap" parameter-map="{adapterKey: 'party', recordKey: JSON.stringify({partyId}), addressPurpose: 'PostalPrimary'}"/>
```

Its parameters are a record's `adapterKey`, `recordKey` (its key fields as JSON) and an optional `addressPurpose`, or a
plain `longitude`, `latitude` and `label` (with `pick=true` to let the user choose a point), or a plain `postalCode` and
`countryGeoId` (an address with no point), and `dialogId` when the dialog's id is not `LocalMapDialog` (a list with one
per row). Optional for any of them: `profileId`, a consumer's map profile (HOME's), which the effective configuration's
precedence weighs as any consumer's (a profile the user may not be offered falls back), and `routeApp`, the host app
whose routes the record's [links](#record-links) resolve to; Open in Maps carries both to Browse.

- **A record.** LocalMap reads the record's place in its actions through `get#EffectiveConfig`, which calls the
  registered adapter's `get#RecordMapContext` as the user; the adapter checks access first (no record layer's adapter
  is asked). The place, precision and source show the same in the browser, html, print, text and the terminal; an
  approximate place shows its area (the adapter's extent), and a record that is an area or a line is drawn as it is. A record the user may not read is "not
  available", with nothing of it, and so is an adapter they are not granted.
- **Choose location** shows only when the adapter's `permittedActions` include `chooseLocation`. A click on the map (or
  Enter on it, or a typed latitude and longitude) fills the form, and Save location posts it to `storeLocation`, which
  resolves the adapter by its key (`get#AdapterService`, operation `store`) and calls its `store#RecordLocation` with
  the record's key, the version read and the point. The adapter checks again and writes through its domain's guarded
  services. The form posts in the background in every client: on success the dialog closes (`background-hide-id`, the
  dialog's id) and the page behind it reloads in place (`background-reload-id="_screen"`); a refusal stays in the
  dialog with its reason.
- **An address with no point.** An adapter that has an address but no point answers its `postalCode` and `countryGeoId`
  with no geometry, and a plain address passes them to the dialog: the map shows the code's local postal-area position
  ([Postal lookup](#postal-lookup)), labeled "Approximate location — postal area", with its broad extent, its source and
  edition and the code's place names. Opening it asks no outside service, and nothing is saved to the record.
- **A plain point** grants no record access and posts nothing: with `pick=true` the page takes the point from the
  bubbling `m-map-pick` DOM event (`detail` `{lon, lat}`).
- **The record's links** (an adapter's route keys) show where a registered route resolves them ([Record links](#record-links)).
- **Open in Maps** shows only to a user whose own grants open the Maps app (`isPermittedOwn`: an app's grant on the
  screen that shows the dialog does not count). Keyboard focus returns to View map when the dialog
  closes, and on a phone the dialog fills the screen.

### From a dashboard

A Dashboards geomap ([Monitoring](Monitoring.md)) opens Maps where a country or a point is clicked (or chosen with Enter):
Browse centered on the country's largest land area (`extent=west,south,east,north`, so an overseas territory does not
widen it; west greater than east for one across the antimeridian, such as Russia) or on the point (`longitude`,
`latitude`), and nothing else of the panel's rows. It is offered only to a viewer whose own grants open the Maps app
(`Dashboard.xml` sets `appLinks.maps` by `isPermittedOwn` of `Maps.xml`; an app's grant on the screen that shows the
dashboard does not count); without it the geomap is as it was. The terminal's geomap does the same where the link is
offered: in a dashboard's table mode (`g`) or on a report panel of its own, `M` opens the table's current row in Maps
(its country or its latitude and longitude, the fields the geomap reads, a point south of the drawing's 60°S too) with
the same extent or point, and the listed place of that row is highlighted; the table's own keys (rows, data links,
filters) are unchanged. `M` because `o` is the shell's Open in browser and `m` the dashboard's panel menu. The
countries' extents are a table built from the same outline (`COUNTRY_EXTENTS` in
`crates/moqui-tui/src/components/world.rs`).

### Under an app's route

An app shows the dialog under its own route by the
[thin-wrapper rule](../dev/Practices-App-Dev.md#a-framework-screen-under-an-apps-root): a screen of the app's own that
takes in LocalMap and one `transition-include` for each transition it offers, with its grants naming its own screen,
never `component://tools/screen/Maps/LocalMap.xml`:

```xml
<screen default-menu-include="false" history="false">
    <transition-include name="getConfig" location="component://tools/screen/Maps/LocalMap.xml"/>
    <transition-include name="storeLocation" location="component://tools/screen/Maps/LocalMap.xml"/>
    <widgets><include-screen location="component://tools/screen/Maps/LocalMap.xml" share-scope="true"/></widgets>
</screen>
```

Leave `storeLocation` out where the app offers no Choose location. An app whose routes the record's links should resolve to
sets `routeApp` (a field of its wrapper before the include, or a parameter of its View map). The app's mount narrows what is shown; it never grants
authority, and the dialog does not need the app's other grants or System's.

### Record adapters

A component registers an adapter as a `moqui.maps.MapAdapter` seed row: a key and its services, which implement
`org.moqui.maps.MapServices` `get#RecordMapContext` (label, geometry, precision, source, `recordVersion`,
`permittedActions` of `view` and `chooseLocation`, links; with no geometry, the address's `postalCode` and `countryGeoId`
for the postal fallback), `store#RecordLocation` (answering `applied` false with
`STALE_VERSION`, `PERMISSION_DENIED` or `RECORD_UNAVAILABLE`) and `find#LayerFeatures` for a record layer (a `MapLayer`
with format `MLF_ADAPTER`). The component grants its adapter services to the users who open the dialog; a browser sends
only the key. The framework's test adapter (`framework/test/service/org/moqui/mapstest`) is the example the specs use.

### Record links

An adapter names its record's links by route key (`{label, route, parameters}`: Mantle's `partyDetail`, `facilityDetail`,
`assetDetail`), never a URL. A component registers where a key goes in its app as a `moqui.maps.MapRoute` seed row:
`routeKey`, `routeApp` (the app whose maps it is for, empty for every app's), `urlTemplate` (a path of this site with
`{name}` placeholders of the link's parameters, each URL-encoded: `/apps/myapp/Party/PartyDetail?partyId={partyId}`),
`screenLocation` (the screen whose grant opens it), `priority` (lower first) and `componentName`:

```xml
<moqui.maps.MapRoute mapRouteId="MYAPP_PARTY" routeKey="partyDetail" routeApp="myapp" priority="10" componentName="myapp"
        urlTemplate="/apps/myapp/Party/PartyDetail?partyId={partyId}" screenLocation="component://myapp/screen/MyApp.xml"/>
```

The effective configuration resolves the record's links in the host app's context (`routeApp`) as
`record.resolvedLinks` (`MapRoutes.js`), which LocalMap and Browse show: for each key the enabled rows of the host app by
priority, then those of no app, never another app's (a HOME dialog never links into BOSS for a user of both), the first
whose every placeholder the link's parameters fill (each value URL-encoded, so it stays one path segment or one query
value; a value of `.` or `..` fills none) and whose `screenLocation` the user's own grants open (`isPermittedOwn`, view).
A link to the record the map shows (every parameter one of its key fields, with the same value) is left out, and so is a
key no row resolves, silently; `record.links` stay as the adapter named them. A link is data, never authority: the screen
it opens checks the user again.

Rows of no app resolve wherever no `routeApp` is passed, under an app's wrapper of LocalMap that sets none as much as in
the framework's own LocalMap and Browse, so an app whose maps should link only to its own screens sets `routeApp`. BOSS
resolves Mantle's keys itself under its mount (mr-boss `service/ironstone/shared/MapLinks.js`) and registers no rows yet.
`routeApp` alone does not keep rows of no app out: the app's rows come first and those of no app after them. An app that
resolves its links itself sets `routeLinks` false on its wrapper (a field before the include, or a parameter of View map),
and LocalMap shows none of `record.resolvedLinks`; BOSS's wrapper does so, and sets `routeApp` `boss` for Browse.

## Geo data

No detailed geographic data ships with MR Moqui. Postal codes, counties and cities with their points are imported, when
an administrator asks, from registered public data sets or a file of their own, into the basic Geo model
(`moqui.basic.Geo`, `GeoPostalCode`, `GeoPostalCodePlace`, `GeoPoint`, `GeoPointAssoc`, `GeoAssoc`; the derived IDs are
described above `GeoPostalCode` in `BasicEntities.xml`). Nothing is imported at startup or in a test run.

**The data sets** are `moqui.maps.GeoDataSet` rows (`framework/data/MapsData.xml`, with their URLs, years and priorities in
`MapsSetupData.xml`); an administrator adds a row for a mirror or a later year with the same `sourceKey`:

| Data set | Gives | License |
| --- | --- | --- |
| `CENSUS_ZCTA` | US ZIP Code Tabulation Areas: about 34,000 postal-area points. A ZCTA approximates a ZIP delivery area; it is not a USPS ZIP registry, and not every ZIP has one | Public domain |
| `CENSUS_COUNTY`, `CENSUS_PLACE` | US counties and places with FIPS codes, names, state and points | Public domain |
| `CENSUS_ZCTA_COUNTY` | The county, and through it the state, of each ZCTA | Public domain |
| `GEONAMES_POSTAL` | A country's postal codes (124 countries), each with its place names, admin names and a point whose accuracy sets its precision; in the US the codes Census lacks and the place names. Canada, the Netherlands and Great Britain have only the first part of the code | CC BY 4.0, attribution shown |
| `GEONAMES_COUNTRY`, `GEONAMES_ADMIN1`, `GEONAMES_ADMIN2`, `GEONAMES_CITIES15000` | Countries, first- and second-level divisions and cities | CC BY 4.0 |
| `OPERATOR_CSV` | An operator's CSV (`country, code, place, admin1, admin2, latitude, longitude, precision`): a licensed or national data set they converted, or an installation with no outside access | The operator's |

Where two sources have a code, each keeps its own point and the data set's priority makes the active one: the operator's
file (5), then Census (10), then GeoNames (20).

**Maps › Configuration › Geo data** (`MAPS_ADMIN`; `org.moqui.maps.GeoDataServices`, `GeoData.js`):

- **Acknowledge license**: a data set is imported only after an administrator acknowledged its license and attribution;
  who and when are recorded (`licenseAckUserId`, in the audit log).
- **Import** starts a run of the `GeoDataImport` ServiceJob (`start#GeoDataImport`), never the import in the request, and
  notifies the administrator when it ends (topic `GeoDataImport`). A set read a country at a time asks for the country,
  among those the data set has files for (its `countryCodes`: GeoNames' 124), or offers **Import every country**: after a
  confirmation with an estimate (`everyCountryEstimate`), a queue of runs, one pending run a country with a country Geo
  sharing a `queueId`, run one after another (each run starts the next when it ends, whatever its end), each its own run
  and edition; a run holds one country's records while it loads, so the queue's peak memory is that of its largest
  country (plans/archive/Maps.md §6.3). A refusal every run would meet (the set disabled, its license, outside requests off) cancels
  the waiting runs with its reason. **Imported** shows the queue's progress with **Cancel the rest** (`cancel#GeoDataImport`
  with the `queueId`: the countries waiting are cancelled, the one running stops at its next step; a run with no word for
  an hour is taken as abandoned and ended at once). An operator's file is uploaded (at most `maps@import-max-bytes`, refused before it is
  written when the import could not start) and kept under the administrator's own folder,
  `dbresource://moqui/maps/import/(userId)/`; an operator's file is read only from the caller's own folder there, from
  `runtime://import/` or from their own uploads. A completed import of an upload deletes the uploads of the scope's
  earlier runs, and Remove deletes them all. A download needs `maps@external-requests`. The Import dialog is loaded
  when opened (`GeoData/ImportDialog.xml`).
- A run downloads conditionally (an unchanged source ends as current), reads and checks the whole file writing nothing,
  loads in batches of 2,000 codes, and only after a whole load end-dates what the new edition no longer has. A file with
  far fewer rows than the edition in use is refused unless forced. Rejected rows go to the run's error file. A failed or
  cancelled run leaves the edition in use as it was. The importer never changes a user-confirmed point, a record's own
  location, a code made by hand or another source's rows. A first-level division (a state, a province) is found among
  the country's divisions by its ISO 3166-2 code: GeoNames' first-level codes are FIPS 10-4 for most countries, crossed
  to ISO by `moqui_lib::geo::admin1` (its `admin1_iso.tsv`, GeoNames joined with Wikidata's CC0 codes), so a city or a
  second-level division of `CA.01` links to Mantle's `CAN_AB`; a division with no ISO code is matched by name, else
  `{country}_A1_{code}` is made.
- **Imported** lists each data set and country with its edition in use, its postal codes and points in use and its last
  run, with **Refresh** (that scope again; an operator's file is the one its last run read), **Cancel** (a run stops at
  its next step, retiring nothing) and **Remove** (`remove#GeoData`: the rows nothing refers to are deleted, the rest
  end-dated, a batch of Geo IDs at a time in its own transaction, so one stopped part way is finished by another; a
  batch that fails is rolled back and named in the error while the rest go on; an import of the scope is refused while
  it runs, by its mark on the data set, and an import that starts anyway stops it; not offered for countries and
  divisions, whose rows are shared).
- **Refresh all** runs the `GeoDataRefresh` ServiceJob now (`request#GeoDataRefresh`, also System › Health's refresh
  `maps.geodata.refresh`, offered to holders of `ADMIN_SETTINGS_MANAGE` and run only for those who also hold `MAPS_ADMIN`,
  `ADMIN`'s): each scope in use (one that ever completed a run and still has rows, whatever its last run) is imported
  again but one with a run in progress, and an unchanged source changes nothing.
  **Schedule** sets it every week or every month; a scheduled run has no user, so it runs as the administrator who set it
  (a user of the job who added themselves, and its `runAsUserId` parameter), who is notified of each run.
- **Runs**: the last 50 runs with their counts, failure and error file.

The readiness provider `maps.geodata` (`check#GeoDataReadiness`) says whether postal data is in use for the instance's
default country (`instance@default-locale`), whether the last import of a scope in use failed (a scope that never
completed or was removed is not counted), and whether a postal data set's license is acknowledged while there is none.

### Postal lookup

`org.moqui.maps.MapServices.get#PostalLocation` (`MapLookup.js`) takes a country (a Geo ID or an ISO alpha-2 or alpha-3
code) and a postal code, nothing else, and answers from the local Geo data with no outside request. The code is read by
the importer's own rules (`normalize#PostalCode`): leading zeros kept, a Canadian, British or Dutch code spaced as its
country writes it, a US ZIP+4 read as its ZIP code. Each match carries the code's active representative point, never a
user-confirmed point or a record's own location; its source, edition and attribution; its place names, which are aliases
of one area (a code with several is `ambiguous`, and they are offered as choices, none chosen); and an extent around the
point, which is not the area's boundary: a square of the area's land area where the point's source gave it (a Census
ZCTA's `ALAND`, kept as `GeoPointAssoc.landArea`; half its side between about 0.5 and 55 km), else a fixed box. Puerto Rico, the US Virgin Islands, Guam, American Samoa and the Northern Mariana Islands
have US ZIP codes: a code of one of them not found under its own country is looked up among the US codes.

Precision is said in words, never as a made-up radius:

- A postal-code match is **Approximate location — postal area** (`GPP_POSTAL_AREA`, or `GPP_PLACE_ESTIMATE` where the
  source's point is a place's own or estimated, as GeoNames' accuracy 4 and 1 are). A Census point stands for a ZCTA, not
  a precise postal boundary.
- A full Canadian, British or Dutch code with no row falls back to its declared prefix (the forward sortation area, the
  outward code, the four digits) at `GPP_PLACE_ESTIMATE`. A prefix code, found so or asked for, is said to be only that
  ("only the first part of the code is known here") with a wider extent. No other country's code is ever cut.
- A code with no row finds nothing (`NO_MATCH`): a valid ZIP may have no ZCTA, and a new or PO-box code may be in no data set.

Results are cached in `maps.postal.lookup` (`MoquiDefaultConf.xml`: at most 20,000, a day) as data with no words of a
language, put in the user's words when read. A run that committed rows (a whole load, or the batches of one that failed or
was cancelled after them) and a Remove clear it. Every signed-in user may call it (`MAPS_DIALOG`).

## Overture places and buildings

Optional regional data from an [Overture Maps](https://overturemaps.org/) release (plans/archive/Maps-Overture.md, stage O1 of
plans/archive/Maps.md §4.3): **Places**, named destinations with their categories, for Browse's search, and **Buildings**, footprints
drawn as a map layer. Nothing of it ships and nothing is read until an administrator imports a region; maps, archives and
postal lookup need none of it. It is the `moqui-maps` crate, which the host links under its `overture` feature (Maximal and
Enterprise, not Lite); a server without it keeps what was imported usable and says it cannot import.

**Maps › Configuration › Overture** (`MAPS_ADMIN`; `org.moqui.maps.OvertureServices`, `OvertureData.js`):

- **Themes and licenses.** Overture's licenses are by theme and source, never one: Places are CDLA Permissive 2.0 (Meta,
  Microsoft and others), Apache 2.0 (Foursquare) and CC0 1.0 (AllThePlaces); Buildings are ODbL 1.0 (OpenStreetMap,
  Microsoft) with CC BY 4.0 sources (Esri Community Maps, Google Open Buildings). **Acknowledge license** records who and when
  (`moqui.maps.OvertureTheme`, in the audit log) and the source licenses it covers (`licenses`); nothing of a theme is
  imported before. A run whose sources name a license the acknowledgement does not cover is staged but waits for review
  (**Waiting for license review**, its `newLicenses` named); **Acknowledge and activate** adds them and activates it.
- **Regions.** Add region: a name, a box in degrees of at most `maps@overture-max-area` km² (2,500; west less than east, so
  not across the antimeridian), the release to import (from the catalog, or the latest when imported), the profile its
  Buildings layer is shown in and its themes. Each region theme shows its active release and rows, the previous one and its
  last run, with **Import**, **Cancel**, **Roll back** and **Remove**.
- **Runs**: the last 50 with what each read. A run held for review shows **Acknowledge and activate** and **Discard**.

**A run** (`start#OvertureImport` makes it pending and starts the `OvertureImport` ServiceJob as the administrator, who is
notified when it ends, topic `OvertureImport`; `run#OvertureImport`):

1. **Preflight**, refused with no run: the feature built in, `maps@external-requests`, the theme's license acknowledged, the
   region's size, no run of the region and theme in progress (one with no word for an hour is taken as abandoned), the
   caller's own `MAPS_ADMIN` grant (asked again when the job runs).
2. **Pinned.** The run records its release (`releaseVersion`, explicit before anything is read) and reads its catalog from
   `maps@overture-catalog` (https only; Overture's STAC root, `https://stac.overturemaps.org/catalog.json`; documents only
   on its host): the release's and the collection's `schema:version` must be 2.x (`SCHEMA_UNSUPPORTED` otherwise, before
   any data), then only the items whose extent meets the region (a collection whose items must each be read for their box
   is refused past 1,024 items, `TOO_MANY_ITEMS`).
3. **Read by range** (`read#OvertureTheme`, in Rust: our own GeoParquet reader on `ruzstd`, no C library). For each file,
   through `moqui_core::outbound` (https on the public host its item names, no internal address, 60 s and 64 MiB a
   request): its end, which holds the footer and gives the length, then only the row groups whose `bbox` statistics meet
   the region, and of each only the theme's columns (checked for their types); a row is kept when its own box meets the
   region. Every later range is sent with `If-Match` the first answer's `ETag` (else `If-Unmodified-Since` its
   `Last-Modified`), so a file replaced while it is read fails the run rather than mixing two files. Every count a file
   claims is checked before memory is taken for it: a chunk of at most two million entries, 32 MiB a page and 128 MiB
   decoded, a row group's columns 256 MiB together. A row with no ID, no name (a place)
   or a geometry that does not read is counted as rejected (the first 20 kept with why) and the rest go on. At most 200,000
   places or 250,000 buildings a run (`TOO_MANY_ROWS`: choose a smaller region). The counts are written to the run as it
   goes and a cancel is read between row groups and while tiles are made.
4. **Staged.** Places as `moqui.maps.OverturePlace` rows of the run (2,000 a transaction); Buildings cut into MVT tiles (the
   large overlays' tiler, layer `overlay`, each footprint's name or class as its label and its class, height, floors and
   sources as its description) and written as one PMTiles archive at
   `dbresource://moqui/maps/overture/(regionId)/buildings-(importId).pmtiles`.
5. **Checked**, nothing in use changed: no rows (`NO_ROWS`), fewer than half the active run's rows unless forced
   (`FEWER_ROWS`), a license not covered or none named (`UNKNOWN`: review, above), and the run still going as it was (one
   cancelled, or taken for abandoned, while it read is not activated). A run held for review is activated after its
   licenses are acknowledged (refused when it is older than the active one, `OLDER_THAN_ACTIVE`), or **Discard**ed.
6. **Activated**, in one transaction: the region theme's active run becomes it and the one before it the previous one, kept
   for rollback; for Buildings the region's layer (`OVT_BLD_(regionId)`, `MLF_PMTILES_VECTOR`, the theme's attribution) is
   registered to the new archive (`register#ArchiveLayer`), so a map drawing the old revision is refused it (409) and loads
   its configuration again, and added to the region's profile. The region theme's other runs waiting for review are
   superseded, and they and the runs older than the previous one are then removed.

A run that fails or is cancelled deletes what it staged, and the active release stays as it was. **Roll back** makes the
previous run active again (its rows and archive were kept, the layer registered to its archive) and removes the one rolled
back. **Remove** deletes a region theme's rows, archives and layer, and keeps the run history. Overture deletes a release's
files 60 days after it, so nothing here reads a release again: rollback and search use what was imported.

**What an import never touches.** It writes only these rows, its archives and its own layer: no party, facility, Geo,
point, address or record's location is created or changed, and a place that a later release removes or renumbers changes
nothing outside them. A GERS ID (`gersId`, with the run's release) is an external reference, not a key: Overture does not
promise it never changes. Footprints are not legal property boundaries.

**Search.** Browse's search lists the places of the active Places runs within the view on screen whose name or category
has the text (two characters at least; the names that start with it first, by the `importId` and `nameSearch` index, then
the names and categories that have it anywhere, each query bounded; then the nearest to the view's middle), after the saved places, records and
postal codes and before the geocoder: local rows, no outside request. Each result names its category, locality, source
datasets and licenses, its GERS ID and release, and the search says "Places from Overture Maps Foundation" with the theme's
attribution.

**ODbL.** The Buildings archive is a Derivative Database of ODbL data. Drawing it on a map is a Produced Work, which shows
the layer's attribution ("© OpenStreetMap contributors, Overture Maps Foundation" and the CC BY sources). Giving the
archive, or tiles of it, to anyone outside the installation is a public use of the Derivative Database, which must then be
offered under the ODbL (or with its source data and the means to make it): the archive is outside every drive, so only the
layer guards it, and the import sets no `viewPermission`; give the layer one or keep its profile to the users meant.

The readiness provider `maps.overture` (`check#OvertureReadiness`) says when a run waits for license review, when the last
import of a region theme failed, and when regions are set up on a server built without the feature.

## Overture addresses

Stage O2 ([Maps-Overture §8](../../plans/archive/Maps-Overture.md#8-o2-regional-addresses)): Overture's **Addresses** theme for a
region, imported as Places are (Configuration › Overture: the theme `ADDRESSES`, a run pinned to a release, read by range,
staged, checked, activated, rolled back, removed), and searched by a geocoder of this server, **Overture addresses (local)**
(`MPK_GEOCODER_OVERTURE`, seeded disabled as `OVERTURE_ADDRESSES`). The dataset alone is not a geocoder: the parsing,
normalization, ranking and ambiguity are ours (`moqui-lib` `geo/address.rs`, which `moqui-maps` uses).

**Limits, from the evaluation of release 2026-11-18.0.** The theme is alpha with partial coverage, the US included (downtown
Austin has a tenth of Boston's density); a source may give no postal code (none of Italy's do); the schema does not say what a
point stands for (an entrance, a building or parcel centre, a position on the road); several points may share a number with
nothing to tell them apart (in central Florence 44 % of numbers, 43 % of those more than 50 m apart); and address IDs are not
stable. So a match is never called exact or a rooftop, several points are choices, a missing number is the street's
estimate (no interpolation), the points are no postal-code directory (the [postal lookup](#postal-lookup) stays the
fallback), and a refresh is compared by address, not by ID.

- **Licenses by source.** Sources vary by country and dataset (Italy's `CC-BY-4.0`, the Dutch BAG's `CC0-1.0`, the US
  National Address Database's `LicenseRef-Public-Domain-US`, and Massachusetts' points, which Overture marks
  `LicenseRef-Proprietary`). The theme's acknowledgement covers none at first: a region's first run waits for review naming
  the licenses its sources give, and an administrator acknowledges each after reading its terms. Each address found names
  its source dataset and license.
- **The import.** Rows `moqui.maps.OvertureAddress` (street, number, unit, postal code, postal city, locality, region, point,
  sources; `gersId` and release as an external reference only), 2,000 a transaction, at most 1,000,000 a run
  (`TOO_MANY_ROWS`: about 115 km² at Boston's density, a point per unit). A row with no ISO country, or neither street nor
  number, is rejected. The run records its countries and what it changed against the run active before it, by address:
  added, removed, moved more than 25 m, unchanged (Configuration's runs; a change of the matching's tables between two runs
  shows as addresses removed and added). Activation changes the index as a whole; the run
  before is kept for rollback. An import never reads or writes a record, a Geo or a point outside its rows, so a user's
  confirmed point at an address a release removes stays as it is.
- **The provider.** No URL; its test is a fixed query made from a row of its own data, passed when it comes back as an
  address match within 30 m of that row, so it passes only once a region's addresses are active. It is tested, enabled and
  asked with `maps@external-requests` false, and sends nothing. It is asked only while a region's addresses are active and
  the server has the `overture` feature (on a server without it, it is not asked and its test says why). Readiness says when it is
  enabled with no active addresses (`NO_ADDRESS_DATA`).
- **Matching.** The text is read into number, street, unit, postal code and locality as an address of the country given (a
  Browse search's country) or of each country of the active regions: the number first in a number-first country (the US,
  Canada, Britain, Ireland, Australia, New Zealand, France) and after the street elsewhere, a unit after `Apt`, `Unit`,
  `Suite`, `#`, `Interno` and the like followed by a unit's name (never `Ste` where French is written), the postal code by
  its country's shape; an ordinal (`5th Ave`) is never a number, and where a comma ends the street only the number that ends
  it is the house number (`Piazza 1 Maggio, Firenze` has none). Streets are compared by a key: case and
  diacritics folded, dots dropped, and the words of the country's languages read one way (English with its directionals,
  Italian, French, German, Spanish, Portuguese and Dutch: `St`/`Street`, `Ave`/`Avenue`/`Avenida`, `P.za`/`Piazza`,
  `Bd`/`Boulevard`, `-straße`/`-str.`, `-straat`), with particles (`di`, `de'`, `du`, `von`, `van`) left out of a street of
  more words. The rows of the street and number are ranked: the postal code agreeing, then the locality; points of one
  address within 30 m are one candidate (its units: named without a unit unless it is the one asked for), candidates within
  a margin of the best are choices (`ambiguous`), the nearest the view's middle first. A number and street whose postal
  code or locality differs from the one asked for is an estimate offered as a choice, never an address match. A number's letter that the source keeps as the unit (Italy's `12/A`) is an address match; a
  number found only without its suffix (`51` for `51r`) and a street without the number are estimates. A text the index
  does not have is answered with the postal area of the code it gave (`NO_ADDRESS_MATCH`, approximate), else nothing
  (`NO_MATCH`): choose the point on the map. A search sends at most 24 indexed queries, and one user at most 20 searches a
  second (`RATE_LIMITED`).
- **Browse and the local-map dialog.** Browse asks it third, as any geocoder, each candidate with its precision and its own
  source. A record whose adapter answers an address with no point (`address1`, `address2` when it names a unit, `city`,
  `postalCode`, `countryGeoId`) is matched by it when it is enabled, as the dialog's own read (a user with no Maps app grant
  has it too): one address match whose postal code and locality agree is shown, said to be found from the address and not
  saved; several are shown as choices, none chosen, over the postal area; none is the postal area. The
  record keeps its own point, and Choose location saves through the adapter: the record's point, then an address match,
  then the postal area, then a point chosen by hand.

## Offline packages

A user downloads a region of their map to the Android app or the desktop client and browses it with no network: its
basemap, their own places and the overlays they read, and the region's postal codes, Overture places and addresses. A
place they save meanwhile is sent when the server answers again. The web PWA has no offline maps. The design is
[plans/Maps-Offline.md](../../plans/Maps-Offline.md); how the apps keep and serve packages is in
[Desktop-Client](../ui/Desktop-Client.md#offline-map-packages) and [Mobile-Client](../ui/Mobile-Client.md#offline-map-packages).

**Making one.** Maps › Offline maps (`Maps/Offline.xml`, `MAPS_APP`) lists the user's packages with their state, zooms,
size and built date, with Download, Update, Cancel and Delete. New offline map draws the region as a box on the map
(the one its west, south, east and north parameters name, else the user's map's initial view); the user picks its zooms
(0 to 16), its basemaps among those their profile offers, and whether it holds their places, their overlays, the postal
codes and the Overture rows. Estimate shows the size and the tile count before anything is built; Build starts it, and
the user is told (a notification of topic `MapPackageBuild`, to them alone) when it is ready or failed. Update builds it
again as a new revision from what they may see then, the earlier revision downloadable until the new one is ready;
Delete removes it, its file and its download records. The terminal page (`Offline.tui.json`) lists the packages and
their states with Update, Cancel and Delete; a package is not made from the terminal. A package is its owner's alone:
it is never shared (a household member builds their own), and every service and request checks the owner (another
user's package is, to them, one that is not there).

**What it holds.** A package (`moqui.maps.MapPackage`) is one region (west, south, east, north; a west east of its east
crosses the antimeridian) at a zoom range, cut on the server from what its owner may see when it is built. Its file is a
zip, every member stored as it is (the tiles are compressed already):

| Member | Contents |
| --- | --- |
| `manifest.json` | `schemaVersion` 1; the package (id, name, bbox, zooms, revision, built date); the effective configuration frozen with every URL package-relative (no host and no path of this server: the packaged basemaps, the outline, the places and overlays, `allowedOrigins` empty, `externalRequests` false, the view the region); each source with its revision, attribution and license; each member's path, size and SHA-256; what was left out (`omitted`) and what could not be read (`failures`) |
| `basemap/<layerId>.pmtiles` | The extract of each basemap archive: the light and dark styles of one archive share one |
| `style/<flavor>.json`, `sprites/…`, `fonts/noto-sans/…` | A basemap's style (one under `/libs`) with its `sprite` and `metadata["ol:webfonts"]` made package-relative, its sprites, and the map label fonts' CSS and woff2 files |
| `outline.json` | The Natural Earth outline, for the zooms below the extract |
| `places.geojson`, `overlays/<overlayId>.geojson` | The owner's places, and the overlays they read now that meet the region, as the overlay transition gives them; an overlay simplified into tiles as its tiles' extract, `overlays/<overlayId>.pmtiles` |
| `postal.json` | The postal codes whose representative point is in the region: code, display code, country, place names with their admin names, point and precision, with the data sets' licenses in the manifest |
| `overture-places.json`, `overture-addresses.json` | The rows of the active Overture imports whose point is in the region, each import's licenses in the manifest; an address row carries the keys the server made of it, which the apps' address search reads |

Only the archive basemaps the owner's map profile offers them now are cut, and only the overlays they read now.
**Record layers** (an app's facilities, assets, a party's addresses) are not packaged: their access is read at each
request, and a copy on a device would outlive a revoked grant; the screen says so. **A provider's remote tiles**
(OpenFreeMap) are never packaged: its terms allow a limited offline download, not a mirror, so an offline package needs
a [registered archive](#archives) (`NO_ARCHIVE` without one). A COG layer is not packaged either.

**How it is built.** The estimate reads each archive's directories for the region's tiles, their distinct contents and
their bytes, with no tile read (`MapSourceServices.estimate#PackageExtract`, in Rust, `moqui-tools` `maps_extract.rs`),
and counts the postal and Overture rows. Build (`create#Package`) records the archives' registered revisions and a random
build key, and the `MapPackageBuild` ServiceJob runs it when the installation has room (`maps@package-concurrent-builds`
at once; another waits, "Waiting for its turn", and starts when one ends). The run is signed in as the package's owner,
whoever's request or build's end started it, and cuts only what the owner may see: for each archive and zoom the
region's tiles in tile-ID order, read by range in batches and handed as stored to a streaming writer (`moqui-lib`
`pmtiles.rs` `ArchiveWriter`, the archive's metadata copied whole), then the other members, read as the owner, the
manifest and the zip, `runtime://tmp/map-packages/<packageId>-<revision>-<buildKey>.zip`, with its size and SHA-256 on
the row. Its progress is on the row and Cancel stops it at its next batch of tiles. It writes only while the package
holds its build key, and fails rather than mixing revisions: an archive registered again since Build, or replaced while
it is cut (its tag and size checked at the start and the end), ends it failed, as does a region where no basemap could
be cut or a zip past the size cap. A member that cannot be read is logged and named in the manifest's `failures` in
general words, and the rest goes on. An owner whose account is closed or whose password must change ends it failed; one
locked out for a few minutes after failed sign-ins puts it back to wait. One scheduler runs at a time
(`schedule#PackageBuilds`, after the commit of the request that asked), and it first ends a build whose run is gone, silent
for an hour once started, or not started six hours after it was handed to the job, by the build's own clocks. The hourly
`clean_MapPackages_hourly` job (`clean#MapPackages`) starts a build whose turn was missed, and removes the file of a package
not downloaded for `maps@package-keep-days` since it was built or last downloaded (the package is left Expired; Update
builds it again) and any file under `runtime://tmp/map-packages` no package names.

**Downloading it.** In the app, the shell's drawer has Offline maps (and Maps, below) for an account with the Maps app.
The offline screen lists the account's packages as the server lists them (`/apps/maps/Offline/packageList`,
`find#Packages`: each package's revision, size, strong tag and download path) and as the device holds them, with
Download, Update (a newer revision on the server), Cancel and Delete from this device; on the phone, "Download only on
Wi-Fi". The app downloads through the account's own requests with its device key, by ranges, resuming where a cut left
it; checks the whole file and each member against the SHA-256 the server and the manifest give; and only then puts the
new revision in place of the one it had, which a failed check keeps. The `package` transition
(`/apps/maps/Offline/package?packageId=…&revision=…`) sends the zip by `ec.web.sendPinnedResourceResponse` with its
SHA-256 as the strong `ETag` (`sha256-…`), by range (one `Range`, `If-Range`); it checks the owner on every request
(`get#PackageAccess`: another user's package is 404) and answers 409 for a revision that is no longer the package's (the
app then offers the new one). Each download is recorded (`moqui.maps.MapPackageDownload`: the package, the revision and,
for a request signed in by a device's key, that `UserLoginKey` and its device name). On Android WorkManager runs the
download (`PackageDownloadWorker`), so it goes on with the app in the background and waits for a network (or Wi-Fi); on
the desktop it is a task of the app.

**The offline screen.** The drawer's Maps opens Maps' Browse (the server's page) when the server answers and the shell's
offline Maps screen (`Shell/MapsOffline`, bundled with the apps and precompiled) when it does not, as the shell's offline
chip says, and so does the Maps app among the drawer's Applications while offline; Offline maps opens that screen at any
time, and offline it lists the packages on the device and says so.
Open draws the package's map and says that it is read from the device, whether it is offline and when the package was
built. The map is `m-map` in package mode ([Qvt-Client](../ui/Qvt-Client.md)), with OpenLayers' main part bundled in
the shell (`shell/libs.txt` `openlayers main`, about 0.7 MB; not the raster part): its configuration the manifest's,
every URL resolved under the package's path on the shell's origin (`/__moqui-app/maps/<packageId>/`) and nothing else
asked, not even the shell's own `/libs`, never another host; the label fonts and sprites the package's, each archive
read by range, the outline its `outline.json`. Locate me, the places and the overlays work as on the web. A package's
sources (each archive's attribution and notice, the postal and Overture licenses) are shown as text under the map and
given to OpenLayers' attribution control only as escaped text (`map.attributionHtml`), never as markup: an archive's
metadata is often HTML. **Search** finds, within the package's region, its saved places (`places.geojson`), postal
codes (`postal.json`, by code or place name) and Overture places (`overture-places.json`, by name, brand, category or
locality) in the page, and its Overture addresses through the app (`packages_addresses`), which matches them with the
server's own address matching (`moqui-lib` `geo/address.rs`), at most ten results of each. **A place saved** there (the
result chosen or the map's center, and a name) waits in the app's outbox as a `map-place` item
([Desktop-Client](../ui/Desktop-Client.md#the-outbox-and-drafts)), listed under the map, and is sent when the server
answers to Browse's `savePlace` with its `clientRef`, which keeps it once however many times it is sent (a place is only
ever added, so no change on the server meets it). One the server refuses (1000 places, an unreadable places file) shows
why, with Retry and Discard; one waiting for a sign-in may be discarded. A server without `features.placeRef` takes no
queued place: the screen saves it at once online and needs a connection offline.

**Limits.**

| Limit | Where |
| --- | --- |
| A package's size: 2 GiB, refused at the estimate (`TOO_LARGE`) and enforced while it is cut | `maps@package-max-bytes` |
| A user's packages together: 8 GiB (`TOO_LARGE_FOR_USER`) | `maps@package-user-max-bytes` |
| Builds at once on the installation: 1 | `maps@package-concurrent-builds` |
| A package's file kept after its last build or download: 30 days | `maps@package-keep-days` |
| A region of at most 4,000,000 tiles (`TOO_MANY_TILES`), zooms 0 to 16; 50 packages a user (`TOO_MANY_PACKAGES`), one of them pending or building (`PACKAGE_BUSY`) | fixed |
| 50,000 postal codes and 200,000 rows of each Overture theme a package; a member past 2 GiB is refused by the apps (Android's WebView reads a body's length as 32 bits) | fixed |
| An account's packages on a device: 4096 MB, and a download starts only with room for its rest, its unpacked copy and 64 MB more | `<client-conf packages-max-mb>` ([Desktop-Client](../ui/Desktop-Client.md#device-modes-and-client-conf)) |

**Privacy.** A package holds only what its owner could read when it was built, and a copy on a device stays readable
there until it is deleted there, also after the server revokes access to what is in it: the offline screen says so, and
so does the apps' [privacy notice](../Privacy.md). It is kept in the app's own files, readable only by the app, outside
the account's store (`client/data/<account>.packages/`). A personal device keeps it across sign-outs; a shared device's
go with the account's data at its sign-out, and removing an account always deletes them. The server records which
device downloaded which revision (`MapPackageDownload`); the account's Devices list does not show them yet. Browsing a
package sends nothing: no tile, style or search leaves the device, and only a saved place goes to the server, when it
answers.

## Live location

A person shares where they are, until a time they choose, with people they choose; nothing else starts a share
([plans/Location-Sharing.md](../../plans/Location-Sharing.md)). The server side is built: the services
`org.moqui.maps.LocationServices` (`framework/service/org/moqui/maps/LocationServices.xml`, `LocationShares.js`), the
entities `LocationShare`, `LocationShareAudience`, `LocationShareAudienceMember`, `LocationShareOwner` (the lock of an
owner's share count), `LocationLatest`, `LocationTrailPoint` and `LocationXmppLatest` (the owner's XMPP app's latest,
for its XMPP contacts only) (`moqui.maps`,
`framework/entity/MapLocationEntities.xml`) and the hourly clean, Maps shows them (below, "In Maps"), and the Android
app sends a share from the phone, also in the background (below, "From the Android app"), a chat room's members get
one from Chat's Share live location (below, "In chat"), a share's XMPP contacts follow it in their XMPP apps (below,
"Over XMPP"), and a guest link opens its public page (below, "Guest links").

**In Maps** (Browse; LL2):

- **The live layer.** Browse's configuration carries `locationShares` (`get#EffectiveConfig` with `locationShares`
  true, which only Browse's `getConfig` asks for; the terminal twin reads the same through `MapConfig.js` `liveShares`):
  the user's own active shares and those shared with them, read
  through `find#MyShares`, `find#SharedWithMe` and `get#Share` (a kept trail, its latest 500 points, of at most 10
  shares), so each is checked against the share's current audience. `m-map` draws each share with a position as a
  marker labelled with its label and the position's age, its accuracy as a ring and a kept trail as a line, the user's
  own in green and the others' in teal, grey and dashed once no report has come for five minutes (the page that sent it
  closed, or the device lost its fix); the same shares are listed under the map as text. It follows each share on its
  topic `locationShare:<shareId>` of the page's notification socket while it shows it: a position moves the marker, a
  new end is taken, and `ended` (or `revoked`, to a viewer) takes the share off and lets go of its topic. A position's
  age is kept from when it was first read or pushed (Browse stamps each read once), so a configuration drawn again never
  makes an old position fresh.
- **The Live location panel.** *Shared with me*: each person's label, the age of the position, the end, Show on map, and
  Directions (in the Android app a `geo:` link its `openExternal` hands to the phone's maps app; elsewhere Open in
  OpenStreetMap, the place at openstreetmap.org in a new window, offered only while `maps@external-requests` is on, which
  is the only way a position leaves the server from here, by the user's act). *Who
  sees my location*: each share of the user's own with its audience and end, Stop, Extend (a new end, the audience never
  sent again), Delete trail, a person or group taken out, and for a group Add the group again, the one act that sends a
  group to `update#Share` (a new snapshot: who joined since is let in and told, who left or blocked the owner is left
  out).
- **Share my location** (the panel's button): people the user may message (as Chat finds them), groups the user is in,
  XMPP contacts when the server has XMPP, a guest link (its page's link shown once, in the dialog: below, "Guest links"), until (15 minutes to `location@max-share-hours`), exact or approximate, and a trail or not. Each
  person is told by the `LocationShared` notification.
- **Sending from a browser.** A share started on Browse is sent from that page while it is open: the browser's
  `watchPosition`, at most one report a share's interval (`location@min-interval-seconds`, or
  `location@approximate-interval-seconds` for an approximate share) through the `reportPosition` transition, which
  answers `report#Position`'s outcome as its JSON's `statusCode`: 429 is sent again after `retryAfterSeconds`, 410 and
  404 stop the page sending it, 409 `STALE` and another 400 pass that fix over, and 409 `CLOCK_AHEAD` or 400
  `BEFORE_SHARE` send from then on without the device's time (the server's); a lost sign-in (401, 403, an answer that is
  not the transition's JSON) stops sending. The watch asks for high accuracy while an exact share is sent (started again
  when that changes); a new share waits for a fix taken after it began (one is asked for at once), and with no fix for
  three intervals (a still device) one is asked for again. The shares a tab sends are kept in its session storage under
  the account (`moqui.scopedKey`) with the session it was kept in, so a reload goes on sending them and a sign-in anew does not, pruned to the user's active shares and forgotten at
  a sign-out (`moqui.beforeLogoutHooks`); Send from here takes over one started elsewhere. When the page closes the
  share stays active and goes stale; its audience sees the age. A banner above the map names every active share of the
  user's own and every share the page sends (with whom, until when, whether this page sends it, or the device that does)
  with Stop. Send from here posts the `sendFromHere` transition first (`update#Share` with `sendFromHere`), so a share
  another device sent is sent from this page from then on, and that device is refused (409 `OTHER_DEVICE`) and stops; a
  page that gets `OTHER_DEVICE` stops sending too and offers Send from here again.
- **The device it is sent from.** A share records the device whose request started it (`LocationShare.userLoginKeyId`:
  the request's device key, `ec.user.loginKeyId`, the key's stored hash as the device list names a device, never the
  key), or none for a browser page; `report#Position` takes that device's reports only, and for a share with none no
  device's, so the owner on another device or page gets 409 `OTHER_DEVICE` ("Another device sends this share's
  location") until they take it over (Send from here, Send from this device: `sendFromHere`). The renewal of the
  device's key moves its shares to the new key (`renew_device_key`), keeping the replaced key as `renewedFromKeyId`,
  whose reports are taken while it still signs in (its few minutes' grace; cleared by a takeover). Revoking a device
  key (its sign-out, the device list, an administrator ending it) stops every active share it sends
  (`stop#DeviceShares`, in a transaction of its own, each share in its own, its errors logged and cleared so the revoke
  always goes through), also when the device could not tell the server. A share has an audience version
  (`audienceVersion`) that every change of its audience that can reach someone new raises (an entry added, a group
  added again); a share sent from a device takes reports of that version only (`report#Position`'s `audienceVersion`,
  409 `AUDIENCE_CHANGED` before anything is written or pushed, also for none), so the Android app sends the version
  its person confirmed and pauses on a change, and Browse sends the one it read and reads again on a change; a
  browser's share (no device) sends none and is not checked. The owner's reads say where each share is sent from (`sentFromDevice`, the device's
  `deviceName`, `fromThisDevice`), never the key's id.
- **From the Android app** (LL3; [Mobile-Client, Live location](../ui/Mobile-Client.md#live-location)). Browse in the app
  has `moquiDesktop.location`: Share asks for the phone's location permission first (approximate alone for an
  approximate share), after the app's dialog that says why, and the app asks "Send your location from this phone to
  ... until ...?" in its own dialog, with the server's words (`get#AppShare`, `/qapps/clientLocationShare`), which is
  what lets it send in the background (a page cannot start it alone); once the share is made the app's foreground service of type `location` (`LocationShareService.kt`) sends it
  from the phone, also with the app in the background, under its notification ("Sharing your location with Sam until
  15:30", with Stop) for as long as it does; a share the service could not start (notifications off, the app not in
  front) is sent by the page while it is open, as a browser does. The service reads Android's `LocationManager` (the
  platform's fused provider, else GPS or the network's; no Google Play services) every 5 seconds while the phone moves,
  every 30 while it is still, and at the share's own interval for an approximate one, and hands each fix to the host's
  `report_location` (`moqui_client::location::Reporter`), which keeps only the newest point not yet sent and posts it to
  `/qapps/reportClientLocation` with the account's device key: a transition of the webroot's `qapps` that every
  signed-in user reaches (Browse's own are `MAPS_APP`'s, while every signed-in user may own a share), POST only, which
  calls `report#Position` and answers its outcome as `statusCode`, as `reportPosition` does. It handles the answers as
  the page does (429 waits `retryAfterSeconds`, `STALE` and another 400 are passed over, `CLOCK_AHEAD` and
  `BEFORE_SHARE` send with the server's time), each post within 15 seconds, and stops a share at its end, on a 410 or
  404, on `OTHER_DEVICE` (tried again once across a renewal of the device key), at the account's sign-out or removal
  (stopped on the server first, while the key is held), on the notification's Stop or with notifications turned off
  (both stop the share on the server too, `/qapps/stopClientLocation`, kept and tried until the server confirms) or the
  page's Stop, and when the location permission is taken away (Android ends the process: the share goes stale until
  its end or a Stop). The notification check lists the location permission and the shares the phone sends.
- **The desktop app** shares as a browser does: Browse sends while its window is open (`watchPosition`). A server's page
  in a client window gets `Permissions-Policy: geolocation=(self)` (`moqui_client_tauri::scheme::PAGE_PERMISSIONS_POLICY`),
  which lets Browse, the window's own page, ask and keeps a frame of another origin from it; the permission itself is
  left to CEF (`bridge::on_permission` answers only the camera, the microphone and screen capture), and whether a
  position comes is the platform's (see [Desktop-Client](../ui/Desktop-Client.md)).
- **The app bar's chip.** While the user has any active share of their own, the shell shows "Sharing location" (with the
  count when there are more than one) in the app bar on every screen, a shell indicator ([Qvt-Client, Shell
  indicators](../ui/Qvt-Client.md)) that the framework's `shell-indicator` provider answers for every signed-in user,
  whatever apps they may open (`LocationServices.get#ShellIndicator`, a count of the user's own shares, no audience or
  position read; registered in `MoquiDefaultConf.xml`'s `screen-facade` `shell-providers`, since sharing is for every
  user, not only Maps users). The shell reads it once with its shell data, when its notification socket opens again,
  and on the user's own pushes of a share's start, change or end (which every device of the owner gets, so a share
  started on another device shows too) and at the soonest end. It opens My location shares (`Maps/MyShares.xml`), which
  every signed-in user may open, Maps user or not (`MAPS_SHARES`, as `MAPS_DIALOG` opens LocalMap): the user's own
  active shares with who sees each, its end and its last position's age, Stop, An hour more (the end alone, within
  `location@max-share-hours`) and, for a user who may open the Maps app, Open in Maps (Who sees my location). A Browse page that
  sends a share adds "· sending here" to the chip. With Browse's banner, and the Android notification (LL3), it is how
  the owner sees every active share wherever they are.
- **The terminal.** Browse's twin lists, under its map, the user's own shares (with whom, until, the last position's age
  and coordinates, Stop) and those shared with them (`Maps/LocationSharesTui.xml`).
- **Over XMPP** (LL5; [XMPP, User location](../interfaces/XMPP.md#user-location)). A share with XMPP contacts in its
  audience sends each position it takes, from any device or page, to those contacts' XMPP apps as user location
  (XEP-0080): a `geoloc` event from the owner's JID to each of their sessions that asked for it, with the values the
  audience sees (an approximate share's cell), and an empty `geoloc` item when it ends (at its time too, sent by the
  XMPP session), is stopped or revokes them. A contact is told (`LocationSharedXmpp`), reads nothing of it on the web
  (`find#SharedWithMe`, `get#Share` read people and groups), gets only positions received after they were added, and
  adding one raises the audience version. The owner's own XMPP app's location feeds a share only when the owner turns
  on "Also use my XMPP app's location (sent to my XMPP contacts only)" in Share my location (`takeXmppLocation`, on
  `create#Share` and `update#Share`, audited; turning it on raises the audience version): its location publish is
  `report#XmppPosition`, taken by each such share with its rules, kept apart (`LocationXmppLatest`) and sent to the
  share's XMPP contacts only, never its latest position, trail, people, groups or link. The first one tells the owner's
  XMPP app by a headline, and Who sees my location and My location shares say where the last position came from
  ("from your XMPP app" or "from this page or phone") and the XMPP app's last position's age. With no such share a
  publish is refused, so a client's setting never starts or feeds a share, and its "stopped" (an empty item) ends
  none: the share is the owner's to stop here. A share the phone sends takes it too once opted in; the phone's dialog
  and notification then name its audience with "(your XMPP app's location also goes to your XMPP contacts)", and the
  raised version pauses the phone until its person confirms. Turning it on needs an XMPP contact in the share (else it
  is refused, `XMPP_NO_CONTACT`); turning it off leaves the version; a revoke that takes out the last XMPP contact turns
  it off (its audit row says so), while taking out a person, group or link leaves it. While the phone or page and the
  XMPP app are on different devices, the XMPP contacts may see the marker alternate between the two. The toggle is in
  Share my location, Who sees my location and My location shares; turning it off deletes the XMPP app's last position.
  XMPP contacts are users of this server only: there is no federation.
- **In chat** (LL4; [Comms, Live location](Comms.md#live-location)). Chat's Share a location has Share live location:
  `ChatLocationServices.send#LiveLocation` makes an ordinary share through `create#Share` (no trail, purpose "In chat:
  the room's name") whose audience is the room's members of that moment as `LsaUser` entries, after the same directory
  and block rules, and posts a message carrying only the share's id (`content.location.liveShareId`). Its card reads the
  share through `get#Share` (`get#LiveLocation`, which first reads the message through `get#Timeline`, so only a reader
  of the room asks) and follows its topic; `get#ShareStatus` (internal) tells a reader of the room whom the share does
  not reach whether it has ended, and nothing else. Who joins the room later is not added, who leaves by themselves
  stays in until the owner takes them out here, and who is taken out of the room by another, or blocked by the owner, is
  taken out of the room's live shares (`revoke#ChatAudience`, internal, the `revoke#Audience` path whoever the caller,
  over the room's `moqui.chat.ChatLiveShare` rows); the share is in Shared with me, Who sees my location and the app
  bar's chip like any other. The
  Android app's dialog and service, and a browser's sending while its page is open (`moqui.chat.live.sender`, Chat's
  `reportLiveLocation`), are as for a share started in Maps. Not in an encrypted room, where the server would see each
  position (the decision and its later step are in Comms).
- **Guest links** (LL6; plans/Location-Sharing.md §3.7). A link's token is a selector and a secret; its page is
  `/location/<selector>#<secret>` (`Maps/SharedLocation.xml`, the token route `location` of tools' `MoquiConf.xml`, in the
  public shell, no sign-in). The secret is the URL's fragment, which a browser sends to no server: the public shell
  (`WebrootPublic.qvt.js`) takes it as it loads, before the page does, sets the address bar to the URL without it (kept
  for the tab, so a reload still reads it), and the page posts it in the body of its read (`get#LinkShare`), so no
  request line, server log or `Referer` holds it. Two places can still hold the whole link: the browser's history and its
  sync keep the URL first opened, fragment and all, and a mail or chat service's link scanner may rewrite or fetch the
  whole URL; and the mail that carries it holds it until its share ends (below). The page shows the share's label, its
  purpose text (an ETA its owner wrote) and its latest position with its age, on the country outline or the one basemap
  an administrator marks public, and as text where no map draws; it reads again every 20 seconds and answers nothing
  else: no position received before the link was added, an approximate share's cell, at most
  `location@link-max-views-per-minute` reads a minute for the link. A wrong secret is counted on the link and against the
  client, and the link is refused from `location@link-max-failed-tries` (10) of them, and once its share ends. The page is
  sent with `Referrer-Policy: strict-origin` (a tile host that needs a Referer gets this site's origin, never the path
  with its selector), `Cache-Control: no-store`, and a `Content-Security-Policy` that lets it reach this server, the
  public basemap's origins and the theme's own outside files only.
- **A public basemap.** A `MapLayer` with `isPublic` Y and `isBasemap` Y, enabled, with no `viewPermission`, no
  registered resource (`resourceLocation`), and no provider that is proxied (`proxyRequests` Y) or on a private network
  (`allowPrivateNetwork` Y); the first by `layerId` is drawn, else the country outline. Configuration does not set
  `isPublic`: an administrator sets it on the layer by a data load (`<moqui.maps.MapLayer layerId="..." isPublic="Y"/>`)
  or with the entity tools. Outside requests and the provider's test hold as for any map.
- **A share for a record.** A component's service that has checked the caller may act on a record (Mantle's
  `create#RecordGuestShare`, a technician's visit) names the record on the share with its end (`store#ShareRecord`,
  `recordThruDate`); the share's end is no later, also when `update#Share` extends it (`AFTER_RECORD_END`), and the
  record's end stops it (`stop#RecordShares`). Mantle mails the link only once the technician's app has confirmed the
  share (or, in a browser, at the press), and keeps the mail's `EmailMessage` row for the mail queue with the link's
  secret blanked from its body when the share ends or is stopped (mr-mantle `LocationShareServices.xml`).
- **The owner's acts.** `create#Share` (the audience, the end, exact or approximate, a trail or not), `update#Share`
  (extend or shorten, add to the audience, change the label or purpose), `stop#Share`, `revoke#Audience` (one person,
  group, XMPP contact or the link), `report#Position` and `delete#Trail` are the owner's alone; another person's share,
  an administrator's request included, answers `SHARE_NOT_FOUND`. Every signed-in user may call them for their own
  shares (`LocationShareServices`, framework `data/MapsLocationData.xml`).
- **Who a share is for.** A user the owner may message (the chat directory's rule, `ChatServices.get#RefusedUsers`: a
  user it refuses, or one who blocked the owner, answers as one that does not exist), a group the owner is in now (never
  `ALL_USERS` or a group of external users), a contact of the owner's XMPP roster (`username@xmpp-server@domain`, a
  person of one of the owner's direct rooms, a user of this server since XMPP has no federation; the entry keeps the
  contact's user, whose XMPP sessions get the share's positions, "Over XMPP" above), or one guest link a share, whose
  token is answered once by the call that makes it and kept only as its digest (as `ResourceShare`'s link). Each person
  added is told who shares with them and until when (the `LocationShared` notification, in their own time zone). A
  person has at most `location@max-active-shares` shares on at once, and a share at most `location@max-audience` people
  (the members its groups reach, counted each), contacts and links; a group with more than four times that many current
  members is refused before its members are written down. A share whose owner's lock row cannot be made or locked is
  refused (`SHARE_LOCK_FAILED`) rather than counted without it.
- **Who reads it.** `find#MyShares` (the owner's shares with their audiences), `find#SharedWithMe` and `get#Share` (the
  latest position, and a kept trail in pages of up to 1000 points) answer the owner and the share's current audience
  only: a user whose entry is not revoked, or a member of a group whose entry is not revoked who is in the entry's
  snapshot and whose membership row of the snapshot is still current, checked again at every read and every push. A
  viewer sees the latest position and a kept trail the server received from when the audience first reached them only
  (by the server's receive time, `receivedDate`, never the device's; from their entry's `fromDate`, or their group
  snapshot's `reachedDate`, kept when the group is added again only for the same membership row), so no position
  reported before a person was added, or while they were out of the group, reaches them (but for the snapshot member
  whose own row is ended and reopened, or made again with the same `fromDate`, who keeps their first reach and sees
  what was received while out, which plans/Location-Sharing.md §3.1 accepts); the owner sees everything. The
  snapshot (`LocationShareAudienceMember`) is the group's members when the owner adds the group, each with the
  membership row that makes them one, less every one the chat directory refuses the owner at that act (its hooks, the
  external-caller rule, the people who blocked the owner; a block made later applies at the next re-add, as for a direct
  user), written only by `create#Share` and by `update#Share` when the owner adds the group again. A person taken out of
  a group sees nothing of it from then on, and one put into it later, by anyone and with whatever dates (an
  administrator, an app or a household organizer; a backdated, reopened or new membership, a member removed and given a
  new row) sees nothing and gets no push until the owner adds the group again, which lets in and tells its new members,
  names them to the owner, and tells anyone it leaves out. The one way back without the owner: a snapshot member's own
  row, ended and reopened, or deleted and made again with the same `fromDate` (an administrator can do either in System
  › Access), lets that same person back. No grant of these services reads another person's shares, and the generic
  entity tools, entity REST, entity-auto services and the agents' `data` tool refuse these seven entities, an
  administrator's included (`moqui-entity` `sensitive.rs`).
- **What the server's operator can still reach.** These services keep a share from other users, not from whoever runs
  the server: an administrator who signs in as the person (`ADMIN_LOGIN_AS`) sees what that person sees; a holder of the
  Tools SQL runner reads the tables; any server script can turn on the `NotificationCapture` tool (meant for specs, not
  gated) and see pushes in transit; and the database, its backups and its own logs hold what the tables hold. Who shared
  with whom and until when (never a position) stays in the entity audit log and in the `LocationShared` notification
  rows of the people told, under their own retention.
- **Reports.** From the owner only, for an active share: the values checked (latitude and longitude in range, accuracy,
  heading and speed, a time not before the share began and later than the latest), at most one a
  `location@min-interval-seconds`, or `location@approximate-interval-seconds` for an approximate share (a sooner one is
  dropped with 429), 410 for a share that has ended or was stopped, which stops a device, 409 `CLOCK_AHEAD` for a
  time more than a minute ahead of the server's clock, and 409 `OTHER_DEVICE` for a request not from the device the
  share is sent from (above). An approximate share's position is made here, before it is stored
  or sent: the centre of a cell of about 1 km on one fixed grid that every share uses (so two shares of one person at
  one place show the same cell), the last cell kept until a fix is more than a quarter of a cell inside another one, for
  the person rather than the share (a new share starts from the cell the person's other approximate shares hold, so they
  show one cell; once none is active the next starts afresh), an accuracy of 1000 m (a device's coarser one rounded up
  to the next 1000), no heading or speed. Keeping the cell stops jitter at a line flipping it, but a change of cell
  still shows that the person crossed a line 0.75 cell from the old centre, to within what they move in one
  `location@approximate-interval-seconds` (not to within the cell). The stored and answered `observedDate` is rounded
  down to the minute; the push itself goes out when the report comes, so its arrival tells the time. Each report
  replaces the share's latest position, adds a trail point when the share keeps a trail, and goes out on the topic
  `locationShare:<shareId>` to the audience's current users only, never saved as a notification or pushed to a phone. On
  the same topic the owner's other devices are told when a share starts, and the owner and the audience when it changes
  or ends; a revoked entry's people (and the owner) are told it was revoked. The audience learns of a start by the
  `LocationShared` notification.
- **Retention.** A share ends at its end (at most `location@max-share-hours` from its start or last extension) or when
  stopped: reads stop then, and its latest position is deleted at once by a stop, and up to an hour after an end by time
  (the hourly job). A trail is kept only when the owner chose it for that share, for its days after the share ends (at
  most `location@trail-max-days`), and the owner deletes it at any time. The hourly job `clean_LocationShares_hourly`
  (`clean#LocationShares`) ends the shares past their end, deletes the trails whose purge date has come
  (`trailPurgeDate`, set when the share ends) and removes the shares ended more than `location@trail-max-days` ago.
  Start, extension, stop and an audience added or revoked are in the entity audit log (who, when, the audience), never a
  position. Positions never go to a log, a telemetry attribute, an error message or a cache.
- **Settings**, on the conf's `location` element (`moqui_conf__location__max_share_hours` and the rest):
  `location@max-share-hours` (24), `location@trail-max-days` (30), `location@min-interval-seconds` (5),
  `location@approximate-interval-seconds` (60), `location@max-active-shares` (20), `location@max-audience` (50) and
  `location@link-max-views-per-minute` (30, the reads a minute of a guest link's page) and
  `location@link-max-failed-tries` (10, the wrong secrets after which a guest link is refused).

## Tests

The regression spec is `tools/test/MapsScreens.spec.js` (Browse in tui, its terminal's overlays, postal search and the
view it is opened on, and its live location list with Stop; LocalMap, a plain XML screen, once in qvt for a writer and
once in html for a user it refuses the record; the access rules, Save location, the profile rules), with Browse's one
qvue render in a browser in the client spec `tools/test/client/MapsBrowseExtent.spec.js` (the MapsBrowse component and
its map, opened on an extent across the antimeridian at the first load, and the extent check's refusals; a share started
with a guest link, sent from the page, drawn on the live layer and stopped). The live layer's pushes, stale style, end and
escaped labels are `webroot/test/client/pages/WebrootMap.page.js`; the share services' rules, with who sees what in the
effective configuration's `locationShares` and a share's device (another device or a browser refused, Send from here
and Send from this device, the app's `reportClientLocation` and `stopClientLocation`, a renewed key, and over XMPP an
XMPP position taken by the share with XMPP contacts only and each report pushed to those contacts alone),
`framework/test/LocationShares.spec.js` (with the XMPP opt-in: none refused, the contacts alone reached, the version
raised), with the XMPP session's side in `moqui-xmpp` (`session::tests`, user location); the phone's sending, `moqui-client` `location_tests.rs` (`report_location`: the
newest point only, 429, stale, the clock, a 410, 404, another device or a sign-out stopping it), `moqui-mobile`
`location_tests.rs` and the Kotlin `LocationRulesTest` (the interval, the stop rules, the start only from the app in
front). The configuration services' rules are `framework/test/MapsConfig.spec.js` (with a record link resolved to the host
app's route, else one of no app, and left out for a non-holder, and a profile's view projection: a registered one handed
to the map with its definition, refused for a vector-style basemap and for an unknown code), and the archives'
(registering, the map-source transition's access, ranges, revocation and stale revisions, and the tile transition's tile,
304, 204, non-holder and stale revision, and readiness's room to replace an archive) `framework/test/MapsSource.spec.js`, with the header's, the tile IDs', the
directories' and the archive writer's in Rust (`moqui-lib` `pmtiles_tests.rs`) and the tile reader's (`moqui-tools`
`maps_tiles_tests.rs`: gzip or plain by the client, a missing tile, `If-None-Match`, a changed archive, the cache's
bound). Overlays and places: `framework/test/MapsOverlays.spec.js`
(a file refused before it is kept, an upload kept with its bad feature counted, a delimited text and a binary WKB upload
read, a GeoPackage refused, an edit saved by the owner beside its KML file and refused to another user, for a geometry
that is not valid and for a stale version, the overlay transition for the owner, a
share holder, a non-holder and a revoked share, saved places, and a file past `maps@max-features` offered simplifying and
then cut into tiles beside it that its owner and a holder of its share read and a non-holder and a revoked share do not,
`TILES_NOT_WRITABLE` for a reader, `TILES_RUNNING` while its run goes on, and a run that ended without saying so ended
by Stop simplifying), with the converter's rules in Rust
(`moqui-lib` `geo/overlay_tests.rs`: geometry, CRS, caps, KML sanitizing, GPX; `geo/overlay/formats_tests.rs`: each
other format's reader, its CRS and axis order, and its malformed and oversized inputs: shared TopoJSON arcs, claimed WKB
and shapefile counts, a zip that inflates too far, nested collections, a geometry of thousands of rings, the positions
bound, code pages and CRS names; and registered CRSs converted: a NAD83 UTM shapefile by ESRI's name, a GeoJSON file in
the British National Grid, LAEA Europe's GML URN northing first, an EWKT and EsriJSON one, an unregistered SRID refused);
the projections (`geo/proj_tests.rs`: the EPSG guidance examples of transverse Mercator, Lambert conformal conic, LAEA,
polar stereographic, the Helmert and geocentric conversions, Snyder's Albers, the definitions refused by name, a `.prj`
found by authority, name or parameters); a COG's header (`geo/tiff_tests.rs`: classic and BigTIFF in both byte orders,
what a map cannot draw, and hostile files: a loop, a directory past the end, a count that overflows, too many directories
or entries); `MapsSource` registers a COG of a drive, refuses one in strips and reads its ranges for a holder of its share
and not another user, with its projection in the holder's configuration and the holder's own band settings kept (a band
it lacks refused), and a real public Sentinel-2 COG (the bucket OpenLayers' COG example reads) registered from its
provider's URL (the provider tested by a range of that COG), its header recorded and its full image's first tile read by
range is the acceptance test `MapsCogPublic`; `MapsOverlays` converts a UTM and a British
National Grid overlay and the tiler's (`geo/tiles_tests.rs`:
the budgets against a pathological file, a stop within a zoom, points, the poles and the antimeridian;
`geo/simplify_tests.rs`, `geo/mvt_tests.rs`); the geocoder: `framework/test/MapsGeocoder.spec.js`
(nothing sent untested or with outside requests off, the test's fixed address, the Census answer through `HttpFixtures`,
a failure with nothing else asked, the rate and the requests a day, the dialog's reads sending nothing); a provider through
this server: `framework/test/MapsProviderProxy.spec.js` (the test with the key and the recorded paths, the proxy's path in
the configuration, the style and TileJSON rewritten, a tile with the key added, a path not recorded, outside requests off and
a user not offered the profile refused); OGC services: `framework/test/MapsOgc.spec.js` (a DOCTYPE, an entity
declaration and an unknown entity refused; WMS, WMTS and WFS capabilities recorded through `HttpFixtures`, a user@host
address not the provider's; the effective configuration for a permitted user and not another; the proxy's OGC requests for
a registered layer the caller may be offered and no other, by KVP and by a WMTS template; Add OGC layer's services), with the reader's
rules in Rust (`moqui-lib` `geo/ogc_tests.rs`), `m-map`'s WMS, WMTS and WFS sources a page test, and the USGS topographic
WMTS and NASA GIBS's WMS tested, added and asked for an image, the acceptance test `MapsOgcPublic`; Configuration refused
to an app's own allow of `Maps.xml`: `MapsScreens`. The
archive backends, local-only browsing and an installation
without Mantle are the acceptance tests `MapsArchiveBackends`, `MapsLocalOnly` and `MapsFrameworkOnly`
(`./moq accept framework --spec MapsArchiveBackends`), and a real Protomaps extract (a box of Florence, zooms 0 to 14, read
by ranges from a recent daily build, about 3 MB) registered for both local layers and drawn through the map-source
transition, its tiles' MVT layers checked, `MapsProtomapsExtract`. Geo data: the importer's rules are
`framework/test/GeoDataImport.spec.js`, the postal lookup's `framework/test/MapsPostalLookup.spec.js`, and the Geo data
screen and its Import `tools/test/MapsGeoData.spec.js`, with the parsers, the crosswalk and the land area in Rust
(`moqui-lib` `geo/tests.rs`); the real US import, with its times and counts, is the acceptance test `GeoDataUsaImport`
(`./moq accept framework --spec GeoDataUsaImport`), a queue of three GeoNames countries with the largest, Portugal, `GeoDataEveryCountry`, and a real
import cancelled between batches and run again `GeoDataImportCancel`.
m-map's feature selection (plain-text details, the feature nearest the center), its editing helpers (the edited
FeatureCollection, a keyboard shape, which overlays are editable) and the raster part (loaded only for a COG layer or a
registered projection, a COG's band settings and WebGL style, projections registered from the configuration and a lookup
refused with no outside request) are a page test,
`webroot/test/client/pages/WebrootMap.page.js`, a dashboard geomap's link into Maps `WebrootReport.page.js`, and the terminal's postal search and overlays `moqui-tui`
`components/map_tests.rs`. The acceptance walks are `MapsBrowseWalk` and `MapsDialogWalk` (browser, desktop and phone width) and
`MapsTerminalWalk`, with their fixture `MapsWalkFixtures`: `./moq accept tools --spec MapsBrowseWalk`.
Offline packages: `framework/test/MapsPackages.spec.js` (a package cut from a small fixture archive by the build job and
served to its owner by range with its strong tag, refused another user and 409 for another revision; a basemap the owner
may not see left out; the manifest listing every member with its size and SHA-256 and no URL of this server; a package
past `maps@package-max-bytes` refused at its estimate; a user's second build refused and another user's waiting its turn
and started when the build ends; a cancel; a run whose key the package does not hold writing nothing; an archive
registered again since the build was asked failing it; the owner's places and the overlays they read packaged, another
user's places and an unshared overlay never, a shared one until its share is revoked; the Offline page once in qvt and once in the terminal), with the box's tile set (across the antimeridian, at
zoom 0, a tile's edge), the extract's bounds, a shared content cut once, a replaced archive and a cancel leaving no file in
Rust (`moqui-tools` `maps_extract_tests.rs`), and the streaming writer giving the bytes the whole-archive writer gave
(`moqui-lib` `pmtiles_tests.rs`); the `package` transition asked by GET with its parameters in the URL, as the apps ask
it, `tools/test/client/MapsOfflineDownload.spec.js`. The whole of it on the Android emulator, a package built, downloaded,
browsed and searched with the network cut, and a place saved offline sent when the network is back, is the acceptance
walk `MapsOfflinePackage`: the tools acceptance spec of that name serves its data and package, and
`scripts/android-maps-offline.sh` drives the app ([Mobile-Client, Tests](../ui/Mobile-Client.md#tests)).
Overture: the services' rules are `framework/test/MapsOverture.spec.js` (a non-holder, outside requests off, a license not
acknowledged, a region too large and a release of schema 3 refused; a Places import read by range from the one item and row
group that meet the region, active and searched, the later ranges pinned to the file's ETag; a failed update and a file
replaced while it is read leaving the release before it active; a new license held for review, activated and rolled back;
a run cancelled while it reads and a run held for review discarded; Buildings as the region's layer, removed; Configuration › Overture rendered once
for an administrator and refused to a non-holder), over `HttpFixtures` with `ranges`, its
Parquet fixtures (`framework/test/fixtures/overture`) goldens of the reader's tests in Rust (`moqui-maps`: the reader on the
`parquet` crate's files, v1 and v2 pages, dictionary, delta and byte-stream-split encodings, Snappy, ZSTD and GZIP, bounds,
corrupted pages and crafted counts (delta block sizes, width-0 runs, row counts, deep schemas, a zero type length); the STAC catalog; WKB; the themes' rows). The real latest release for central Florence, with its
times, bytes, rows and memory, is the acceptance test `OvertureRegionImport` (`./moq accept framework --spec
OvertureRegionImport`). Overture addresses: `framework/test/MapsOvertureAddresses.spec.js` (a non-holder's import refused,
a first run held for its sources' licenses and activated; the local geocoder tested by its own data and asked with outside
requests off, sending nothing: an exact match, two points of one number as choices, `12/A`, an estimate for `51r` and for a
street, the postal area of a code it lacks, nothing else; a record's address matched, offered as choices, or its postal
area, nothing saved; a refresh that moves one address and removes one, counted by address, rolled back, a confirmed point
untouched), with the parsing, keys and ranking in Rust (`moqui-maps` `address_tests.rs`, `address_service_tests.rs`), and
the real release for central Florence and part of Boston's Back Bay with sample lookups, the acceptance test
`OvertureAddressLookup`.

Live location: `framework/test/LocationShares.spec.js` (by the real grants, with authorization on: a share read and
pushed to its audience alone, another user and an administrator refused; a blocked user, an audience past `max-audience`
(on create, on update and on a group added again) and a share past `max-active-shares` refused; a group member taken out
losing it at the next read and the pushes, and a membership given a caller's dates and stamp, a backdated add, an ended
membership reopened, an ended one with a new row and a member removed and added again getting nothing until the owner
adds the group again, which names and tells only those it lets in and is audited; a member who blocked the owner never
reached; the rate and its 429, a clock ahead and a time before the share; an approximate share's cell made before it is
stored or sent, the same for two shares at one place, kept through jitter at a cell line, its accuracy 1000 m or the
device's rounded up and its time to the minute; an ended share hidden, 410 to a report and its latest position cleaned;
a trail kept only when chosen, deleted by its owner and purged past its days; the audit rows with no position; a link's
token answered once and only its digest kept), with the generic tools' refusal of the seven entities in Rust
(`moqui-entity` `sensitive.rs`) and the agents' `data` tool's (`moqui-llm` `core_tests.rs`).
