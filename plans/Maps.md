# Maps

November 1, 2026 — initial design, not implemented. The selected browser renderer is OpenLayers. This plan owns
the framework Maps app, reusable map interface, configuration, resource delivery and geographic lookup work.
[BOSS shared maps](../runtime/component/mr-boss/plan/design-crosscut.md#332-maps-and-record-location-dialogs)
and [HOME Maps](../runtime/component/mr-home/plan/initial-design.md#39-maps-places-and-geographic-information)
own their application contexts. No pre-release data migration plan is required.

## 1. Decisions and scope

- Add **Maps** to `runtime/base-component/tools`, alongside Monitor and Dashboards: one framework-owned app
  available without BOSS, HOME, PASS or Mantle. Those components reuse and contribute to it rather than fork it.
- Use **OpenLayers** for browser rendering, its geographic format readers, coordinate systems and editing
  interactions. Its broader GIS support is useful for business applications and the expected HOME audience.
  Its BSD-2-Clause license permits distribution with MR Moqui; preserve notices for it and its dependencies.
- Offer **OpenFreeMap** as an initial hosted basemap option, plus **self-hosted PMTiles through ResourceFacade**.
  These are independently selectable configurations; an installation can operate with only local resources.
- Provide a full-screen map browser and a reusable local-map dialog for an address, party, facility, asset,
  shipment endpoint or another permitted location. Both use the same map component and source configuration.
- Support installation-wide metadata/defaults and personal map preferences, saved views and permitted layers.
- Include local postal-code-to-coordinate lookup as an inexpensive, offline fallback. Approximate postal-area
  coordinates must remain distinguishable from a confirmed location or address-level geocode.
- Keep address standardization, geocoding, basemap tiles, routing and device-location ingestion separate.
  Routing, background tracking and external Google Maps integration are later capabilities, not prerequisites.

The initial delivery covers browsing, search through configured providers, saved views/places, basic geographic
file overlays, record dialogs, local postal lookup, hosted tiles and self-hosted archives. Advanced raster analysis,
large dataset publication and tracking have separate later stages. Useful imports are not limited to business GIS:
GPX walks, KML places, property boundaries and regional overlays are household use cases too.

## 2. Existing foundations and ownership

Verified starting points, rather than claims that Maps already exists:

| Foundation | Reuse and required extension |
| --- | --- |
| Tools app registration | `runtime/base-component/tools/MoquiConf.xml` mounts Monitor and Dashboards under the apps screen; their XML roots and separate security data show the registration pattern. Maps receives its own grants, not Monitor's administrator-only policy. |
| XML screens and browser widgets | Follow the [app practices](../docs/dev/Practices-App-Dev.md): XML owns parameters/transitions/access; a `.qvue` can own the browsing layout. Repeated map use becomes a shared widget, with TUI and print fallbacks. |
| Geographic reference data | `moqui.basic.Geo` already has `GEOT_POSTAL_CODE`, localized names, codes and `wellKnownText`; `GeoAssoc` records geographic relationships. `GeoPoint` stores coordinates, type and `dataSourceId`. Geo-to-point and country/postal-code lookup metadata need extensions (§6). |
| Domain locations | Mantle `PostalAddress` has `postalCodeGeoId` and `geoPointId`; `Facility`, `FacilityLocation`, `Asset` and `PartyGeoPoint` already reference GeoPoint. Mantle owns their access and location updates; Maps knows no business entity names. |
| Resource delivery | [Revision management](../docs/data/Revision-Management.md) and [Git hosting](../docs/interfaces/Git-Hosting.md) describe resource providers, streamed file responses, ranges and validators. Consistent efficient random access across providers remains work; share that work with [HOME Media](../runtime/component/mr-home/plan/initial-design.md#32-media-photos-music-videos-ebooks-and-audiobooks). |
| Dashboard geomap | The [existing geomap](../docs/tools/Monitoring.md) uses Natural Earth country outlines and point summaries. Keep its current reporting behavior; an optional detailed Maps drill-down is a later consumer of this interface. |

Ownership is explicit:

- **Framework:** OpenLayers integration, generic map widget/screens/services, provider and layer configuration,
  postal reference data, coordinate/format utilities, resource streaming and reusable server operations.
- **Mantle:** party/address/facility/asset adapters, permission-scoped queries, provenance of record locations,
  and writes through the domain's guarded services. No new business location registry beside these entities.
- **BOSS/HOME:** contextual navigation, authorized layers and links, vocabulary and workflow-specific actions.
- **PASS/other consumers:** the same component with their own publication and access rules. A public map receives
  explicitly published features and permitted basemaps, never a staff dataset with client-side filtering.

## 3. Screens and reusable interface

Names and routes in this section are proposed contracts, not built artifacts.

### 3.1 Tools Maps app

Propose `component://tools/screen/Maps.xml`, registered as `maps` beside `monitor` in Tools' conf, with:

| Screen | Purpose and layout |
| --- | --- |
| Browse | Map fills available screen space, with optional browser full-screen mode; collapsible search/results and layers panels, basemap selector, scale, attribution, coordinate readout, saved places/views and selected-feature details. Phone layout uses a bottom sheet. |
| Saved views and layers | Personal bookmarks, saved extents, selected layers/styles and source status; authorized shared collections appear alongside personal ones. Reuse ResourceFacade/drives for geographic files and collections. |
| Preferences | Personal default basemap, initial extent, display units, language, layer visibility and optional restoration of last view. No automatic location or browsing-history collection. |
| Configuration | Authorized installation administration of providers, profiles, layer resources, external-request policy, postal datasets, import results and readiness. Ordinary users cannot change these defaults or secret references. |

Search distinguishes local saved places/permitted records, postal-code reference lookup and a configured external
place/address service. Show source, precision and match ambiguity. Navigation to a result does not save a location
to its source record. Failed layers show a named, retryable status while other layers and the record list remain usable.

### 3.2 Map widget and local-map dialog

Propose an XML `map-view` widget backed by `m-map`, with a shared dialog screen
`component://tools/screen/Maps/LocalMap.xml`. Develop the XSD, qvt/static macros, wrapper, gallery example,
HTML/print fallback and TUI rendering together. The browser assets are vendored through the established library
workflow and loaded only on screens using maps; no mandatory runtime CDN or Node test dependency.

The dialog opens beside a permitted address or coordinate field with **View map**. It shows the selected place,
nearby geographic context, source/precision, coordinate copy and **Open in Maps**. Read-only view is the default;
**Choose location** is offered only when the owning record service permits a write. On phones it can occupy the
screen while retaining the same close/return behavior and form state. Keyboard focus returns to the launch control.

The common interface accepts:

- A profile ID, initial center/extent/zoom and a list of registered layer/source IDs; an explicit projection where
  needed. Default interchange coordinates are WGS84 longitude/latitude with documented axis order.
- An optional registered record-adapter ID and canonical record identity/address purpose. The service resolves
  labels, geometry, precision, permitted actions and drill-down links after checking source-record access.
- For a plain coordinate display, a validated point/extent and escaped label. Such input grants no record access.
- Selection/view-change events and an optional proposed point/geometry result. The owner rechecks permission,
  record version and address identity when applying it; the widget cannot issue arbitrary entity updates.

The shared dialog can be used without granting access to the standalone Maps launcher. Split map-view capability
from app-root access, configuration administration and source-record writes. Mount/app context narrows data and
actions; it never grants authority. Do not copy Monitor's grants or require BOSS app access to open a HOME map.

Expose proposed typed services in `org.moqui.maps.MapServices` for effective configuration, saved views, source
metadata, postal lookup and geocoding. Register record/layer adapter services by ID; do not accept an arbitrary
service name, entity name, query, filesystem path or proxy URL from a browser. Optional full-screen record layers
must reapply domain permissions and organization scope to every bounded viewport query.

The TUI presents places/features as an accessible list with address, coordinates, precision and available actions;
it supports lookup and numeric location selection. HTML/print includes that information and attribution; an image
snapshot is optional when supported, not a prerequisite for reading the record. Missing WebGL or a failed renderer
must still leave the textual result and record actions usable.

## 4. Sources, formats and configuration

### 4.1 Initial source choices

**OpenFreeMap:** configure a hosted profile, initially one of its supplied styles. It currently permits commercial
use without an API key or published view/request caps and offers no SLA. Preserve attribution and provider-specific
usage policy. Its limited offline-download allowance is not permission to mirror the planet; use published archives
for self-hosting. The style uses the OpenMapTiles schema. [OpenFreeMap](https://openfreemap.org/)

Use `ol-mapbox-style` for compatible MapLibre/OpenMapTiles style documents. Its font handling differs from MapLibre:
it uses web fonts rather than PBF glyph assets. Configure and package those fonts explicitly so a local map does
not unexpectedly contact the adapter's default font host. Validate each profile's style/tiles/icons/fonts as a
compatible set. [OpenLayers style integration](https://openlayers.org/ol-mapbox-style/index.html)

**PMTiles:** use the OpenLayers PMTiles source integration against authenticated MR Moqui resource URLs. Support
raster and MVT vector archives initially. Each archive has compatible style metadata, extent, min/max zoom,
attribution, source edition and update policy. Regional extracts are useful for households; whole-world downloads
are optional. Protomaps is one available downloadable basemap, not the archive format's only supplier. Its schema
and style differ from OpenMapTiles, so changing suppliers can mean changing the whole profile.
[PMTiles for OpenLayers](https://docs.protomaps.com/pmtiles/openlayers),
[Protomaps downloads](https://docs.protomaps.com/basemaps/downloads)

Initial local overlays support GeoJSON, GPX and KML through OpenLayers, with bounded imports and original files kept
in resources. Add WMS/WMTS service configuration and additional readers (GML, WKT/WKB, EsriJSON, TopoJSON) as the
corresponding workflow is exposed. Library capability alone does not promise an application editor for every format.
GeoTIFF/COG band controls, WFS editing and arbitrary projections are later UI increments; shapefile/GeoPackage
ingestion requires additional tooling. Conversion must preserve source CRS/provenance and report unsupported data.
[OpenLayers formats](https://github.com/openlayers/openlayers/blob/main/src/ol/format.js),
[reprojection](https://openlayers.org/doc/tutorials/raster-reprojection.html)

### 4.2 Metadata and effective settings

Proposed schema, refined during implementation rather than duplicated in each app:

| Metadata | Contents and ownership |
| --- | --- |
| MapProvider | Installation-managed kind, endpoint templates/resource root, attribution/terms, allowed operations, quotas, cache policy and credential/configuration reference. Secret values stay in the established secret/configuration mechanism, not map JSON or exported rows. |
| MapLayer | Provider/resource reference, format, CRS, tile grid, bounds/zoom, style and asset references, source edition/revision and visibility policy. Dynamic layers name a registered access-checking adapter. |
| MapProfile | Basemap plus compatible defaults/layers, region/extent, display preferences, external-access policy and permitted user overrides. One installation default; consumers may select another authorized profile. |
| User preferences and saved views | Use UserPreference for small personal defaults; a proposed MapSavedView stores named extent/projection/layer selections with user ownership. Personal collections use existing resource sharing rather than a new ACL system. |
| Dataset/import metadata | Source, license, edition/checksum, coverage, import status/counts and activation history for postal data and archives. Reuse DataSource and resource revisions where applicable. |

Precedence: installation policy constrains all choices; consumer profile supplies context; allowed personal
preferences override defaults; an explicit dialog target overrides the initial viewport. Removed or inaccessible
layers fall back to a permitted profile with an explanation. Preferences cannot enable an unapproved external
provider or reveal another user's private resource. Household and business contexts do not share record-layer state
unless explicitly chosen; global basemap metadata may be common to both.

Configuration/readiness checks cover the selected style/schema, fonts/icons, archive revision and range access,
source grants, attribution, postal dataset availability and optional geocoder. Administration can disable outgoing
map/search requests while retaining local maps and postal lookup. Public basemap requests may be direct to the
provider when configured; explain that its server sees the requested area and client connection. A proxy is an
explicit provider option, not a claim of complete location privacy.

## 5. ResourceFacade and tile streaming

One byte-access contract serves Media and Maps. Support **git on Postgres, git on filesystem, DbResource and plain
local filesystem**; Maps must not open paths, git objects or SQL blobs directly. Expose provider capability, length,
revision and bounded random reads with independent reader state and cancellation. A provider needing full archive
materialization is not considered ready for large PMTiles; diagnose it or use an explicit managed staging job.

The authenticated HTTP resource adapter must:

1. Resolve an authorized registered source, then authorize each HEAD/GET/range request, including requests after
   a source/share is revoked. Do not expose arbitrary server paths or grant access to an entire drive/repository.
2. Return exact lengths and byte ranges, `206`, `Content-Range`, `Accept-Ranges`, validators and `416` for invalid
   ranges. Bound parsing, concurrent requests, buffers and archive-directory decompression. A single byte range
   per request is sufficient for the initial client; test the actual OpenLayers PMTiles integration against it.
3. Preserve archive byte offsets: no transparent HTTP recompression of the PMTiles payload. Internal tile/archive
   compression is handled by the reader. Keep immutable revisions/ETags consistent across directory and tile reads;
   changed content triggers reader invalidation/retry, never a mixed-revision map.
4. Use same-origin delivery for private resources where possible; configure CORS/header exposure deliberately
   when a separate origin is needed. Shared caches must not retain private responses across authorization scopes.
5. Cache archive metadata/index blocks by source and revision; bound caches and invalidate on replacement. Publish
   archive updates atomically as new revisions, retaining active readers' old revision where the backend supports it.

PMTiles is read-only: changing an archive entails replacement, not individual in-place tile edits. Mutable saved
places and business overlays stay separate. Initial delivery streams archive ranges directly to the browser and
does not require a tile server. A later Rust endpoint returning individual Z/X/Y tiles can serve clients without a
PMTiles reader, using exactly the same facade and authorization boundary.
[PMTiles format and access model](https://docs.protomaps.com/pmtiles/)

The [existing LFS placement issue](Remaining-Work.md#git-and-git-hosting) remains a shared prerequisite for genuinely
disk-backed git/LFS archives. Local git metadata currently does not imply local LFS bytes. Default LFS payload
placement should follow the repository backend with explicit overrides; Maps introduces no separate LFS store.

Archive ingestion must expose each backend's practical size/quota limits. Current documented defaults include
`webapp_max_upload` at 2 GiB and git LFS `lfs-max-object-size` at 5 GiB; a large regional/planet archive can exceed
them. Support operator-registered local files in place and bounded server-side download/import jobs, with explicit
configured limits and disk/database capacity checks. Do not imply that range playback removes upload/storage caps,
or globally raise those caps just to install a map. Include backup and archive-replacement space in readiness.

## 6. Address resolution and postal-code fallback

### 6.1 Resolution and precision

Start with an authorized record's current confirmed coordinate. Otherwise show a locally resolved postal-area
position when available, labeled **Approximate location — postal area**, with an appropriately broad extent.
Offer a configured address/place lookup or manual point selection for a better position. Opening an address dialog
does not silently send the full address to an outside service. An installation can explicitly enable that workflow;
the default useful fallback is local and requires no external request.

Results carry source/edition, match kind, matched address or postal code, precision, timestamp and bounds/accuracy
where actually known. Distinguish user-confirmed point, address match, street-range interpolation, postal-area
representative point and broader place estimate. Do not manufacture a numeric accuracy radius or treat a provider's
confidence score as meters. Ambiguous results remain choices rather than silently selecting the first match.

Reads do not rewrite addresses or canonical record coordinates. Saving a selected point calls the owning Mantle
service with provenance and an address fingerprint/version; changing the address marks its prior derived match
stale. Explicitly recorded facility/asset coordinates are not replaced by a postal fallback. A confirmed navigation
point, delivery entrance and mailing address can differ; preserve their purpose rather than merging them.

### 6.2 Sources researched

| Source | Finding and proposed use |
| --- | --- |
| USPS Addresses API | Validates/standardizes US mailing addresses. The documented API is not a general address-to-latitude/longitude service. Keep normalization separate from geocoding. |
| USPS downloadable products | City State Product remains available through Electronic Product Fulfillment for ZIP/city/county validation under AIS product terms. Its published description does not establish an open coordinate dump. No current open USPS ZIP-to-coordinate download was verified; do not promise or bundle one. Revisit a specific licensed product if it offers useful coordinates and permits this use. |
| US Census Gazetteer | Initial US offline candidate: downloadable ZCTA records with representative/internal-point latitude and longitude. A ZCTA approximates a ZIP delivery area for statistical purposes; it is not a USPS ZIP registry, and not every valid ZIP has a ZCTA. Preserve leading zeros, edition and this approximation. Missing/PO-box-only/new codes can remain unmatched. |
| GeoNames postal downloads | Initial international candidate: country-specific files with postal codes, place/admin names, WGS84 estimated coordinates and an accuracy classification. CC BY 4.0 with attribution; coverage, full-code versus prefix availability and upstream notices vary by country. Some coordinates are inferred from neighboring places/codes. Import per country with its documented granularity, not a blanket worldwide accuracy claim. |
| Other national datasets | Register country-specific import adapters for openly licensed or operator-supplied postal datasets, with explicit CRS, code granularity, update source and terms. For example, Ordnance Survey Code-Point Open provides postcode locations for Great Britain; evaluate its format and attribution for a national adapter. A non-WGS84 source needs a verified transformation before use. |
| US Census geocoder | Optional address-level provider, separate from downloads: returns coordinates calculated along address ranges in supported US territories. Do not label these rooftop/entrance positions. |
| Self-hosted Nominatim / Photon | Candidates for full address/place search and reverse geocoding. Photon also supplies autocomplete. Configure independently of basemap choice; operating these services is optional. |

Sources: [USPS Addresses](https://developers.usps.com/addressesv3),
[USPS City State Product](https://postalpro.usps.com/address-quality/city-state-product),
[Census Gazetteer downloads](https://www.census.gov/geographies/reference-files/time-series/geo/gazetteer-files.html),
[ZCTA definition and coverage](https://www.census.gov/programs-surveys/geography/guidance/geo-areas/zctas.html),
[GeoNames postal dataset](https://download.geonames.org/export/zip/readme.txt),
[OS Code-Point Open](https://www.ordnancesurvey.co.uk/products/code-point-open),
[Census geocoder](https://geocoding.geo.census.gov/geocoder/Geocoding_Services_API.html/),
[Nominatim](https://nominatim.org/), [Photon](https://github.com/komoot/photon).

The public OSM Nominatim endpoint is not the application's default: its policy caps aggregate application traffic
at one request/second and prohibits autocomplete. Photon demo hosting also has limits and no availability guarantee.
Provider configuration must describe permitted caching, retention, rate limits and uses; normalization/search
failure leaves manual selection and postal lookup working.
[Nominatim policy](https://operations.osmfoundation.org/policies/nominatim/)

### 6.3 Extend the basic Geo model

Keep reference geography in `moqui.basic`, usable with only the framework installed:

- Reuse `Geo` of type `GEOT_POSTAL_CODE` and `GeoAssoc` for country and administrative relationships. Names are
  labels; do not squeeze a full postal code into the alpha-2/alpha-3 fields or identify a code by its number alone.
- Propose a small **GeoPostalCode** companion indexed by country Geo ID, normalized postal-code text and code
  granularity, linked to Geo. Preserve original display spelling, spaces where meaningful and leading zeros.
  Country normalization rules are explicit; prefix fallback is a declared lower precision, never a global rule
  that truncates every country's codes. Multiple locality names for one code are aliases, not arbitrary winners.
- Reuse **GeoPoint** and **DataSource** for representative coordinates and their source; propose a **GeoPointAssoc**
  linking Geo to a point with role/precision, dataset edition, validity and source priority. Geo currently has no
  direct point relationship, so this is new work. Support more than one source and choose a documented active
  representative point; do not average unrelated source candidates. Census-derived matches retain the ZCTA basis
  rather than asserting a precise postal boundary. Optional polygons can use Geo's existing `wellKnownText`.
- Resolve `PostalAddress.postalCodeGeoId` in Mantle using country plus code. Return a postal fallback from the
  related reference point without copying it into the address's exact `geoPointId`. Add record-location provenance
  fields/associations where needed rather than overloading an unstructured description.
- Import files through ResourceFacade using a staged, idempotent job keyed by dataset/edition and source record.
  Validate schema, codes, coordinate ranges, CRS, country and source granularity. Report a named bad row and
  continue with independent rows. A malformed/incomplete download never retires the prior active edition.
- Activate a validated edition explicitly/atomically; report rejected and missing records. Keep old provenance and
  referenced Geo IDs, retire removed codes instead of cascading deletion, and invalidate lookup caches after
  activation. Do not overwrite user-confirmed points or unrelated geographic data on refresh.

A framework postal lookup accepts country and postal code only; it does not need a street address or party ID.
Country packages are optional imports, not multi-country datasets loaded on every startup or test run. Update
schedules, distribution of bundled data and retention remain operator choices subject to the dataset's terms.

## 7. Useful Rust server work

Keep interactive rendering and ordinary small-format display in OpenLayers. The following work benefits from Rust
for streaming, bounded processing and shared use by browser, TUI, imports and APIs:

| Work | Scope and proposed placement |
| --- | --- |
| Resource range access and HTTP delivery | Extend existing resource primitives in `moqui-core` and transport in `moqui-host`; all consumers share them. No map-specific bypass of provider storage or authentication. |
| Metadata, postal imports and lookup | Framework services for effective configuration and bounded ZIP/CSV/TSV parsing, validation, dataset activation and indexed Geo lookups. Pure parsers/coordinate helpers belong in `moqui-lib` where they need no ExecutionContext. |
| Provider adapters | Typed Rust clients for geocoding/normalization, timeouts, quotas, cancellation and allowed caching; configuration and authorization services remain the public boundary. Use existing outbound HTTP/simulation infrastructure. Avoid separate JS proxies per app. |
| Archive metadata and optional tile extraction | Validate PMTiles headers/indexes and read metadata through ResourceFacade; a later Z/X/Y endpoint can extract a tile without loading the archive. Evaluate the [Rust pmtiles crate](https://docs.rs/pmtiles/latest/pmtiles/) against the facade's range-reader contract before choosing it. No server decoder is required just to stream the original archive. |
| Geometry/import jobs | Later, shared coordinate validation, extent calculation, reprojection, simplification and large-file normalization to GeoJSON/vector tiles. Evaluate Rust libraries and license/build costs against actual formats; no mandatory GDAL/PROJ or PostGIS dependency in the initial delivery. |
| Spatial data queries | Begin with bounded, indexed latitude/longitude queries and domain scope filters. Add spatial indexes/PostGIS or dynamic vector tiles when dataset size/query needs justify them; no unbounded download of all parties/assets to the client. |

Follow [Rust practices](../docs/dev/Practices-Rust.md) and [crate seams](Crate-Split.md): reuse existing lower-level
modules and extension hooks. If map services warrant a `moqui-maps` framework extension crate, decide its dependency
and feature boundary during P0; `moqui-host` owns async/process edges and core must not depend upward on it. Domain
rules stay in Mantle, with XML Actions first for normal entity workflows. A new crate is not needed merely to serve
a resource's bytes.

Large imports run as bounded jobs with progress, cancellation and per-item failures. A full geocoder/search engine,
planet basemap generator or routing engine is not an initial Rust rewrite; integrate established services or use
prepared downloads. This keeps a HOME installation practical without a large geographic database.

## 8. Access, publication and reliability

- Source-record permissions govern labels, addresses, coordinates, feature properties, counts, exports, cached
  results and selection links. Readable party identity does not imply readable private/home address. Recheck at
  drill-down and save; never send hidden attributes to the browser and rely on a hidden layer.
- Personal saved views/places are private by default; existing resource sharing handles explicit shared files.
  Users can clear optional recent searches/last view. No child-location history or adult monitoring is inferred
  from Maps use; future person-location sharing must be explicit and visible.
- Escape imported feature labels and sanitize descriptions; do not execute KML HTML, scripts, remote icons or
  network links automatically. Limit file/archive expansion and features, and validate geometry before storage.
  Style/source documents and their nested font/icon URLs follow the same configured external-resource policy;
  importing a style does not authorize arbitrary remote requests or bypass a local-only profile.
- Outbound proxy/geocoder destinations come from registered configuration. Enforce configured hosts/schemes,
  redirect and response limits, and SSRF protection; explicitly authorized LAN map servers remain possible.
  Do not put private addresses, exact user locations or credentials in routine telemetry or shared cache keys.
- A failed external provider never silently switches to another external service. Offer a configured local profile,
  an approximate postal view or the readable record. Logs identify the failed source without logging private queries.
- PASS public maps require their own published data adapter and rate/resource limits. Public caches contain only
  public data. Authentication or a Maps grant never substitutes for household/business/publication boundaries.
- Ordinary map browsing does not request device location. **Locate me** is an explicit browser permission action;
  displaying that position neither persists it nor starts background tracking.

## 9. Delivery and acceptance

| Stage | Deliverable and acceptance |
| --- | --- |
| P0 — contracts | Confirm widget/dialog, profile/layer/provider schema, record adapter and permission boundaries, Geo extensions and crate placement. Vendor selected OpenLayers/style/PMTiles versions and their assets/notices. |
| P1 — hosted map and shared UI | Tools Maps full-screen browser, local dialog, personal preferences/saved views, explicit current-location action and OpenFreeMap profile. A user can open a record dialog without Monitor/BOSS or standalone Maps app access, but cannot read an unauthorized record. Text/TUI/phone fallbacks work. |
| P2 — local data and archives | ResourceFacade PMTiles on all four backends, packaged local styles/fonts/icons, US Census postal fallback and an international GeoNames import path, provenance/precision and dataset refresh. Local browsing and postal lookup work with external requests disabled. |
| P3 — consumer integration | Mantle adapters and BOSS/HOME mounts, basic geographic file overlays, optional configured street/place search, private/shared collections. HOME-only and dual-suite users see only permitted places; a viewer cannot save record coordinates. P0–P3 comprise the initial Maps scope. |
| P4 — richer GIS | Configured WMS/WMTS/WFS and additional imports/editing, COG/raster controls, advanced CRS workflows, large-data processing and optional detailed dashboard drill-downs. Prioritize concrete household/business scenarios. |
| Later research | Routing, Google/other external maps, device integrations and “find my XYZ”, explicitly shared person-location feeds and offline mobile packages. Reuse the interface without treating approximate postal positions as navigation or tracking evidence. |

Focused implementation checks should cover:

- Equivalent PMTiles viewport reads on Postgres git, filesystem git/LFS, DbResource and ordinary files; partial reads
  stay bounded, invalid ranges are rejected, archive replacement invalidates readers, and revoked shares deny later
  requests. Use small fixtures, not a planet download.
- Global defaults/personal overrides, disabled providers, private saved views, stale layer IDs and style/font assets
  loaded entirely locally. Hosted styling must be checked with OpenLayers' actual font behavior.
- Zero coordinates, antimeridian extents, latitude/longitude axis order and explicit source CRS. Imported invalid
  geometry or one corrupt dataset row fails independently while valid records remain usable.
- Leading-zero US ZIPs, country collisions, alphanumeric codes, prefix-only datasets, absent ZCTAs, ambiguous places,
  edition updates and preserved manual points. Postal fallbacks show approximate precision at appropriate zoom.
- Public versus private address visibility, cross-organization requests, unauthorized location saves, PASS publication,
  provider failures and no external call merely from opening a locally resolvable address dialog.
- One full-screen browser flow and the same selected location in the shared dialog, keyboard/phone/TUI behavior,
  source attribution, failed-layer isolation and a generic framework-only installation without Mantle.

These are planned checks, not tests run for this document. Keep normal fixtures/specs well below the slow-test
threshold; real-provider, whole-dataset or load scenarios require explicit opt-in and user permission when slow.
