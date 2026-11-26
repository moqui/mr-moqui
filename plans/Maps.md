# Maps

November 1, 2026 — initial design. Built P0 to P3 on November 25, 2026 (each stage's row in §9 says what was built; the HOME mount, P4 and the optional Overture stages remain). Reviewed November 25, 2026 against the code, the sibling plans
and the outside projects it relies on: paths, settings and existing mechanisms are named as they are today, and
what is still to build is marked proposed. The selected browser renderer is OpenLayers. This plan owns
the framework Maps app, reusable map interface, configuration, resource delivery and geographic lookup work.
[BOSS shared maps](../runtime/components/addon/mr-boss/plan/design-crosscut.md#332-maps-and-record-location-dialogs)
and [HOME Maps](../runtime/components/addon/mr-home/plan/initial-design.md#39-maps-places-and-geographic-information)
own their application contexts. 3D layers over this app (terrain, extruded buildings, a facility on the map) are
[mr-three's](../runtime/components/addon/mr-three/plan/design-technical.md#63-3d-for-maps), after P1, and Maps
needs nothing from it; its extruded buildings need the optional O1 data (§4.3), and a terrain elevation source is
its own to supply. No pre-release data migration plan is required.

## 1. Decisions and scope

- Add **Maps** to `runtime/components/base/tools`, alongside Dashboards, System and Tools: one framework-owned app
  available without BOSS, HOME, PASS or Mantle. Those components reuse and contribute to it rather than fork it.
- Use **OpenLayers** for browser rendering, its geographic format readers, coordinate systems and editing
  interactions. Its broader GIS support is useful for business applications and the expected HOME audience.
  Its BSD-2-Clause license permits distribution with MR Moqui; preserve notices for it and its dependencies.
- Offer **OpenFreeMap** as an initial hosted basemap option, plus **self-hosted PMTiles through ResourceFacade**.
  These are independently selectable configurations; an installation can operate with only local resources.
- **A fresh install makes no outside request.** The OpenFreeMap provider and profile are seeded disabled. Until an
  administrator enables a provider or registers an archive, a map shows the Natural Earth country outline the
  dashboard geomap already vendors (`libs/naturalearth/countries-110m.json`), with a notice that links an
  administrator to Configuration.
- Add **Overture** as an optional regional data source: Places and Buildings first, then regional address lookup
  after coverage and matching are evaluated. OpenFreeMap remains the initial hosted basemap; Overture is not a
  prerequisite for Maps, self-hosted archives or the postal-code fallback (§4.3).
- Provide a full-screen map browser and a reusable local-map dialog for an address, party, facility, asset,
  shipment endpoint or another permitted location. Both use the same map component and source configuration.
- Support installation-wide metadata/defaults and personal map preferences, saved views and permitted layers.
- Include local postal-code-to-coordinate lookup as an inexpensive, offline fallback. Approximate postal-area
  coordinates must remain distinguishable from a confirmed location or address-level geocode.
- **Ship no detailed Geo data.** Postal codes, counties, cities and their points are derived from other publishers'
  data sets, so no repository holds them as data files. An administrator runs the **Geo data importer** (§6.4) from
  the Maps settings: the server downloads a registered data set, or takes an operator's file, and imports it.
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
| Tools app registration | `runtime/components/base/tools/MoquiConf.xml` mounts `dashboards` (menu-index 96), `system` (98) and `tools` (99) under the apps screen; Monitor is a section of System (`System/Monitor.xml`), not an app. The three roots are ADMIN-only (`ToolsSecurityData.xml`, `MonitoringSecurityData.xml`); Maps receives its own grants. Two precedents there split a capability from its app root: `SYSTEM_VAULT_DIALOGS` (two dialog screens and their transitions, `inheritAuthz="N"`, granted to ALL_USERS) and `CALC_GRID` (a screen and its services that apps grant to their own groups). |
| XML screens and browser widgets | Follow the [app practices](../docs/dev/Practices-App-Dev.md): XML owns parameters/transitions/access; a `.qvue` can own the browsing layout, beside its XML screen and a `.tui.json` twin. Repeated map use becomes a shared widget, with TUI and print fallbacks; `code-view`/`m-code` is the precedent for a widget with a lazily loaded library (§3.2). |
| Geographic reference data | `moqui.basic.Geo` (`BasicEntities.xml`) already has `GEOT_POSTAL_CODE`, `GEOT_COUNTY`, `GEOT_CITY`, localized names, codes and `wellKnownText`; `GeoAssoc` records geographic relationships (types group member, regions, county seat). `GeoPoint` stores coordinates, elevation, `dataSourceId` and a type whose enumeration has no values yet. No Geo has a point, and no postal-code, county or city Geo is loaded anywhere: seed data has the countries, Mantle's install data the states and provinces (`USA_CA`). The extensions are in §6. |
| Domain locations | Mantle `PostalAddress` has `postalCodeGeoId` (read by sales tax and checkout, set by nothing) and `geoPointId`; `Facility`, `FacilityLocation`, `Asset`, `Container` and `PartyGeoPoint` already reference GeoPoint. Mantle owns their access and location updates; Maps knows no business entity names. Mantle's `GooglePartyServices` geocoder (called by nothing, its key a user preference) is superseded by the provider interface in P3. |
| Resource delivery | Built: `ByteSource` (`crates/moqui-entity/src/resource/source.rs`: length, a strong tag, `read_at`) behind `ResourceFacade::open`, and one response path (`respond_opened` in `crates/moqui-core/src/screen/static_files.rs`, reached by `ec.web.sendResourceResponse` and static files) that answers a single `Range`, `If-Range`, `If-None-Match` and HEAD and compresses no binary or partial response ([Revision management](../docs/data/Revision-Management.md), [Git hosting](../docs/interfaces/Git-Hosting.md)). What remains is in §5; that work is shared with [HOME Media](../runtime/components/addon/mr-home/plan/initial-design.md#32-media-photos-music-videos-ebooks-and-audiobooks). Drives and `ResourceShare` (`org.moqui.resource.FileServices`) are the sharing of files. |
| Saved items and preferences | `moqui.database.GridView` (owner, `isShared`, JSON columns) is the model for a saved view. `UserPreference` holds personal settings, with `UserGroupPreference` rows of ALL_USERS as the installation default and JSON values as Dashboards' `REPORT_PINNED_*`. |
| Connections and settings | A provider with an endpoint and a secret is a registered connection kind: `moqui.server.ConnectionAdapter`, `org.moqui.admin.ConfigurationServices` (`store#Connection`, `test#Connection`), `tools/template/ConnectionParts.xml` and System › Integrations give write-only secrets, a cleared secret and a new test when the endpoint changes, and enabling only after a passing test. `ReadinessProvider`, `RefreshOperation` and `SettingsEntry` rows register a component's checks, refreshes and settings screens ([app practices](../docs/dev/Practices-App-Dev.md)). |
| Outbound requests | `ec.service.rest()` and `moqui_core::outbound`, with the internal-address guard (`denyInternal`, a record's `allowPrivateNetwork`) and `HttpFixtures` standing in for the outside host in specs. |
| Imports and jobs | `ServiceJob` runs and schedules; ETL runs (`crates/moqui-tools/src/etl`) for the run record, counts, error file and rerun conventions; `org.moqui.admin.DataTransferServices` for a staged import of an uploaded file. The workspace already has the `zip` and `csv` crates. |
| Dashboard geomap | The [existing geomap](../docs/tools/Monitoring.md) draws Natural Earth country outlines and point summaries as SVG (`WebrootReport.js`), with a braille land mask in the terminal. Keep its reporting behavior. Maps reuses its outline file as the zero-request basemap, its ISO code mapping and the terminal mask; an optional detailed Maps drill-down is a later consumer of this interface. |

Ownership is explicit:

- **Framework:** OpenLayers integration, generic map widget/screens/services, provider and layer configuration,
  the Geo model and its importer, coordinate/format utilities, resource streaming and reusable server operations.
  Screens are in the tools component; services (`service/org/moqui/maps/`) and entities (a new
  `entity/MapEntities.xml`, package `moqui.maps`; Geo extensions in `BasicEntities.xml`) are in the framework
  component, as Dashboards' are.
- **Mantle:** party/address/facility/asset adapters, permission-scoped queries, provenance of record locations,
  and writes through the domain's guarded services. No new business location registry beside these entities.
- **BOSS/HOME:** contextual navigation, authorized layers and links, vocabulary and workflow-specific actions.
  HOME has no screens or conf yet, so its Maps mount is its first; a photo-location layer from its Media is a
  registered layer adapter of HOME's (§3.2).
- **PASS/other consumers:** the same component with their own publication and access rules. A public map receives
  explicitly published features and permitted basemaps, never a staff dataset with client-side filtering.

## 3. Screens and reusable interface

Names and routes in this section are proposed contracts, not built artifacts.

### 3.1 Tools Maps app

Propose `component://tools/screen/Maps.xml`, registered in Tools' conf as `subscreens-item name="maps"` at
menu-index 97 (between Dashboards and System) with a `menu-image` for the app launcher, with:

| Screen | Purpose and layout |
| --- | --- |
| Browse (`Maps/Browse.xml`, `Browse.qvue`, `Browse.tui.json`) | Map fills available screen space, with optional browser full-screen mode; collapsible search/results and layers panels, basemap selector, scale, attribution, coordinate readout, saved places/views and selected-feature details. Phone layout uses a bottom sheet. |
| Saved views and layers (`Maps/Views.xml`) | Personal bookmarks, saved extents, selected layers/styles and source status; authorized shared collections appear alongside personal ones. Reuse ResourceFacade/drives for geographic files and collections. |
| Preferences (`Maps/Preferences.xml`) | Personal default basemap, initial extent, display units, language, layer visibility and optional restoration of last view. No automatic location or browsing-history collection. |
| Configuration (`Maps/Configuration.xml`, route `/qapps/maps/Configuration`) | Authorized installation administration of providers, profiles, layer resources, external-request policy, Geo data sets and their imports (§6.4), and readiness. Registered as a `SettingsEntry`, which is the path BOSS Admin's settings home waits for. Ordinary users cannot change these defaults or secret references. |
| Local map (`Maps/LocalMap.xml`) | The shared dialog of §3.2. |

Three artifact groups, in a `MapsSecurityData.xml` of the tools component: `MAPS_APP` for the root (a user group
of its own, not ADMIN alone), `MAPS_DIALOG` for LocalMap and its transitions (ALL_USERS, `inheritAuthz="N"`, as
`SYSTEM_VAULT_DIALOGS` is) and `MAPS_ADMIN` for Configuration and the import services.

Search distinguishes local saved places/permitted records, postal-code reference lookup and a configured external
place/address service. Show source, precision and match ambiguity. Navigation to a result does not save a location
to its source record. Failed layers show a named, retryable status while other layers and the record list remain usable.

### 3.2 Map widget and local-map dialog

Propose an XML `map-view` widget backed by `m-map`, with a shared dialog screen
`component://tools/screen/Maps/LocalMap.xml`. Develop its parts together, by the element checklist of
[the BOSS style guide](../runtime/components/addon/mr-boss/plan/style-guide.md) §8.1 and as `code-view`/`m-code`
was added:

- the element in `framework/xsd/mr-xml-screen.xsd`;
- its renderers in `framework/template/screen/`: `ScreenQvt.js` and its mirror `ScreenQvtStatic.js`,
  `ScreenHtml.js`, `ScreenHtmlPrint.js`, `ScreenText.js` and `ScreenTui.js` (JavaScript, not macros), and the
  `STAMPED` list of `crates/moqui-core/src/screen/stamp.rs`;
- `m-map` in a file of its own, `webroot/screen/webroot/js/WebrootMap.js`, loaded on first use as
  `WebrootCalc.js` and `WebrootCode.js` are, with its registration in `WebrootVue.qvt.js`, the regenerated
  `WebrootVue.qvt.render.js` and `m-*` styles in `WebrootVue.qvt.css`;
- a terminal component in `crates/moqui-tui/src/components`, registered by name;
- the qvt render goldens and the client's node tests; a Gallery example in
  `runtime/components/dev/example/screen/Gallery/Widgets.xml`; a row in [Qvt-Client](../docs/ui/Qvt-Client.md).

The browser libraries are the framework's own, vendored as the webroot's others are
([Build-Test](../docs/Build-Test.md)): exact pins in `webroot/build/libs/package.json` and its lock, an entry each
in `build/libs/refresh.mjs` with its license, `./moq update-js` for the lock, the generated `NOTICES.md` and
`MANIFEST.json`, then `./moq build-js` and `./moq verify-js`. OpenLayers, `ol-mapbox-style`, `ol-pmtiles`
and `pmtiles` are ES modules, so they are bundled into one global with esbuild, as CodeMirror 6 is, from an entry
file that imports only what `m-map` uses. There is no per-widget chunking: the bundle loads with
`moqui.loadScript` when the first map mounts, so a screen without a map never fetches it. No mandatory runtime CDN
or Node test dependency. P0 fixes the versions; at this review they are OpenLayers 10.x, ol-mapbox-style 13.x,
ol-pmtiles 2.0.x (which needs OpenLayers 9 or later) and pmtiles 4.x.

The dialog opens beside a permitted address or coordinate field with **View map**. It shows the selected place,
nearby geographic context, source/precision, coordinate copy and **Open in Maps**. Read-only view is the default;
**Choose location** is offered only when the owning record service permits a write. On phones it can occupy the
screen while retaining the same close/return behavior and form state. Keyboard focus returns to the launch control.

**Network contracts of `m-map`** (built in P1, `js/WebrootMap.js`; the reviewed hosts of the bundle are
`OPENLAYERS_HOSTS` in `build/libs/refresh.mjs`). A map asks only the page's origin and the hosts its profile and layers
name:

- A `mapbox://` URL, and any scheme but http and https, is refused before it reaches the library.
- A style document is loaded by `m-map`, its sources, tiles, GeoJSON data and sprite checked against those hosts, each
  source's attribution made escaped text, and its `metadata['ol:webfonts']` replaced with the local fonts' template;
  the same template is the `webfonts` option. The template is absolute (the page's origin and
  `/libs/mapfonts/{font-family}/{fontweight}{-fontstyle}.css`): `ol-mapbox-style` resolves a root-relative one against
  the style's URL, so a hosted style would have its fonts asked of the provider's host.
- What `ol-mapbox-style` fetches itself (TileJSON, tiles, sprites) passes a `transformRequest` allow-list of those hosts;
  a TileJSON is fetched by `m-map` and handed on with its attribution escaped, since the attribution control sets HTML.
- KML is read with an `iconUrlFunction` that answers a blank `data:` icon for every href (and the default pushpin), as
  an IconStyle's image loads while the file is parsed whatever `extractStyles` is; the layer is styled by `m-map`.
- A PMTiles archive must be of this server; its header is read first, so an archive that cannot be read by range is a
  named, retryable failure. A registered archive's URL is the map-source transition's, naming its revision (§5); a 409
  from it loads the configuration again, so a replaced archive is drawn whole at its new revision.
- A hillshade layer's raster operation runs in a Blob worker: a page with a Content-Security-Policy stricter than the
  default needs `worker-src blob:` for it, beside the provider's hosts in `connect-src`, `img-src` and `font-src`.

The common interface accepts:

- A profile ID, initial center/extent/zoom and a list of registered layer/source IDs; an explicit projection where
  needed. Default interchange coordinates are WGS84 longitude/latitude with documented axis order.
- An optional registered record-adapter ID and canonical record identity/address purpose. The service resolves
  labels, geometry, precision, permitted actions and drill-down links after checking source-record access.
- For a plain coordinate display, a validated point/extent and escaped label. Such input grants no record access.
- Selection/view-change events and an optional proposed point/geometry result. The owner rechecks permission,
  record version and address identity when applying it; the widget cannot issue arbitrary entity updates.

**Choosing a location.** No dialog today hands a value back to the form that opened it, and this one does not
need to. **Choose location** posts the dialog's own form to a LocalMap transition, which calls the registered
adapter's `store#RecordLocation` with the proposed point, the record identity and the version read; the adapter
is the owning domain's guarded service. The dialog then closes (`submit-hide-id`) and the host screen reloads with
the saved location. A plain coordinate field, with no record behind it, takes the picked point from an `m-map`
event in the page, as `m-emoji-picker` gives its pick.

The shared dialog can be used without granting access to the standalone Maps launcher. Split map-view capability
(`MAPS_DIALOG`) from app-root access (`MAPS_APP`), configuration administration (`MAPS_ADMIN`) and source-record
writes (the adapter's own check). An app shows the dialog under its own route by the
[thin-wrapper rule](../docs/dev/Practices-App-Dev.md#a-framework-screen-under-an-apps-root): `include-screen` and
one `transition-include` for each transition it offers, with its grants naming its own screen. Mount/app context
narrows data and actions; it never grants authority. Do not copy System's grants or require BOSS app access to
open a HOME map.

Expose proposed typed services in `org.moqui.maps.MapServices`
(`framework/service/org/moqui/maps/MapServices.xml`) for effective configuration, saved views, source
metadata, postal lookup and geocoding. Record and layer adapters implement framework interface services:
`get#RecordMapContext` (label, geometry, precision, permitted actions and links, after the adapter's own access
check), `store#RecordLocation` and a bounded `find#LayerFeatures` for a viewport. A component registers an
adapter as a seed data row (a proposed `moqui.maps.MapAdapter`: a key and the implementing service's name), as
`TransferTarget` and `ConnectionAdapter` rows register theirs; a row naming a service that does not exist is
logged and left out. A browser sends only the adapter's key: never a service name, entity name, query,
filesystem path or proxy URL. Optional full-screen record layers
must reapply domain permissions and organization scope to every bounded viewport query.

The TUI presents places/features as an accessible list with address, coordinates, precision and available actions;
it supports lookup and numeric location selection. HTML/print includes that information and attribution; an image
snapshot is optional when supported, not a prerequisite for reading the record. Missing WebGL or a failed renderer
must still leave the textual result and record actions usable. Browse is a qvue-only screen: asked for in html
under `/apps` it redirects to the same path under `/qapps`, so the html and print renderings are the `map-view`
widget's and LocalMap's, and Browse's terminal form is `Browse.tui.json` with the terminal component, which can
draw the geomap's braille land mask as an overview.

## 4. Sources, formats and configuration

### 4.1 Initial source choices

**OpenFreeMap:** configure a hosted profile, initially one of its supplied styles. It currently permits commercial
use without an API key or published view/request caps and offers no SLA. Preserve attribution and provider-specific
usage policy. Its limited offline-download allowance is not permission to mirror the planet; use published archives
for self-hosting. The style uses the OpenMapTiles schema. [OpenFreeMap](https://openfreemap.org/)

Use `ol-mapbox-style` for compatible MapLibre/OpenMapTiles style documents. Its font handling differs from MapLibre:
it uses web fonts rather than PBF glyph assets, and for a font the browser lacks it fetches Fontsource CSS from
jsDelivr unless the style's `ol:webfonts` metadata or the `webfonts` option of `apply` names another place.
Vendor the fonts each packaged style uses and set that location to them, so a local map does
not unexpectedly contact the adapter's default font host. Validate each profile's style/tiles/icons/fonts as a
compatible set. [OpenLayers style integration](https://openlayers.org/ol-mapbox-style/index.html)

**PMTiles:** use the OpenLayers PMTiles source integration against authenticated MR Moqui resource URLs. Support
raster and MVT vector archives initially. Each archive has compatible style metadata, extent, min/max zoom,
attribution, source edition and update policy. Regional extracts are useful for households; whole-world downloads
are optional. Protomaps is one available downloadable basemap, not the archive format's only supplier. Its schema
and style differ from OpenMapTiles, so changing suppliers can mean changing the whole profile. OpenFreeMap
publishes no PMTiles, so the local profile P2 packages and validates is a regional Protomaps extract with its own
style, sprites and fonts. Settled by P0's spike (§9): `m-map` binds the archive itself, a `PMTilesVectorSource` on the
archive's URL with `applyStyle(layer, style, {source, updateSource: false})`, since `ol-mapbox-style` does not know a
style's `pmtiles://` source; that source is not checked as a URL to ask.
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
| MapProvider | Installation-managed kind, endpoint templates/resource root, attribution/terms, allowed operations, quotas, cache policy and credential. A provider is a registered connection kind (a `ConnectionAdapter` row, `mapProvider`; a geocoder the same way) whose adapter service is built on the engine of `ConfigurationConnections.js`, with MapProvider as its native record. So its secret is an `encrypt="true"` field that is write-only, never in map JSON, an exported row or a browser; an endpoint change clears it and asks for a new test; and a provider is enabled only by a passing test, which is how the seeded OpenFreeMap row starts disabled. Configuration takes the shared parts in by `transition-include` of `ConnectionParts.xml`, and providers also list under System › Integrations. |
| MapLayer | Provider/resource reference, format, CRS, tile grid, bounds/zoom, style and asset references, source edition/revision and visibility policy. Dynamic layers name a registered access-checking adapter. |
| MapProfile | Basemap plus compatible defaults/layers, region/extent, display preferences, external-access policy and permitted user overrides. One installation default; consumers may select another authorized profile. |
| User preferences and saved views | Use UserPreference for small personal defaults (a JSON value where one key holds several), with a `UserGroupPreference` of ALL_USERS as the installation's default; a proposed MapSavedView stores named extent/projection/layer selections with an owner and `isShared`, as `GridView` does. Personal collections use existing resource sharing rather than a new ACL system. |
| Dataset/import metadata | The proposed GeoDataSet and GeoDataImport of §6.4: source, license, edition/checksum, coverage, import status/counts and history for Geo data, and later for optional Overture extracts and derived archives. Retain schema version and upstream attribution/provenance. Reuse DataSource and resource revisions where applicable. |

Settings follow the conf rule. Installation-wide switches and hard limits are attributes of a proposed `maps`
element in `MoquiDefaultConf.xml`, each with its XSD attribute and its entry in
`crates/moqui-core/src/context/conf_schema.rs`: the outbound switch (`external-requests`), the largest download
and import (`import-max-bytes`) and the most features one overlay or viewport query returns (`max-features`). The
environment sets them as `moqui_conf__...` and specs with `setConf`. Providers, layers and profiles are
configuration entity rows, with the OpenFreeMap provider and profile and the outline profile as seed data.

An installation upgraded from before Maps, or one whose default profile row or `maps.profile` preference is gone,
draws the outline profile (`OUTLINE`) and says so to an administrator; P1 builds that fallback with the effective
configuration service.

Precedence: installation policy constrains all choices; consumer profile supplies context; allowed personal
preferences override defaults; an explicit dialog target overrides the initial viewport. Removed or inaccessible
layers fall back to a permitted profile with an explanation. Preferences cannot enable an unapproved external
provider or reveal another user's private resource. Household and business contexts do not share record-layer state
unless explicitly chosen; global basemap metadata may be common to both.

Configuration/readiness checks cover the selected style/schema, fonts/icons, archive revision and range access,
source grants, attribution, Geo data availability and optional geocoder; they register as `ReadinessProvider`
rows, and a data refresh as a `RefreshOperation`. Administration can disable outgoing
map/search requests while retaining local maps and postal lookup. Public basemap requests may be direct to the
provider when configured; explain that its server sees the requested area and client connection. A proxy is an
explicit provider option, not a claim of complete location privacy.

The outbound switch acts on the server: it stops the server's own requests (a geocoder, a data download) and
stops it handing a browser a profile with outside URLs. What a browser may then contact is the page's
Content-Security-Policy. The default policy sets only `frame-ancestors` and `form-action`, so a hosted basemap
loads as configured; an installation with a stricter policy adds the provider's hosts to its `connect-src`,
`img-src` and `font-src` with `webapp@csp-append`, and readiness names a provider the policy would block.

### 4.3 Optional Overture data

Use Overture as an additional source for local search and geographic overlays, with operator-selected regions and
themes. Keep OpenFreeMap as the initial hosted basemap and the existing PMTiles profile as the self-hosted option.
Overture imports do not automatically create Mantle parties/facilities or replace user-maintained record locations.

| Order | Scope | Use and boundary |
| --- | --- | --- |
| O1 — Places and Buildings | Regional place records and building footprints | Places supplies named destinations/categories for search and saved-place selection; Buildings supplies contextual footprints around homes and facilities. Keep imported reference data separate from private household/business records, with explicit links when useful. Footprints do not establish legal property boundaries. |
| O2 — regional address lookup | Optional address-point extract and local matching index | Follow O1 with evaluation of the target region's coverage, fields and position quality. Implement parsing, normalization, candidate ranking and ambiguity handling through the shared provider interface; the dataset alone is not a geocoder service. Keep Census/GeoNames postal lookup as the lightweight fallback. |

**Access and rendering.** Overture publishes monthly releases of GeoParquet data, with a STAC catalog for discovering
release assets. Import only the selected themes/region and needed properties; preserve originals and derived files
through ResourceFacade. Serve modest overlays as GeoJSON and larger ones as generated MVT/PMTiles with a controlled
layer schema and style. Index Places for local search without loading a global dataset into basic Geo tables.
Overture's supplied PMTiles are for data inspection: its documentation advises against production use and gives no
stable layer/property/zoom or availability contract. They can help evaluate a region, but production overlays use
our validated, versioned outputs from source data. [Catalog and tile limitations](https://docs.overturemaps.org/getting-data/cloud-sources/)

**Licensing.** Record terms and attribution by theme and contributing source for the selected release. Places
includes CDLA-Permissive-2.0, Apache-2.0 and CC0 sources; Buildings is distributed under ODbL; Addresses has varied
source licenses. Do not apply one blanket Overture license. Carry source notices into displayed layers, search
results and distributed extracts/tiles as required, and account for ODbL obligations when publishing derived
databases. A newly encountered license or changed terms requires review before activating the affected source.
[Overture attribution and licensing](https://docs.overturemaps.org/attribution/)

**Versions and identity.** Discover updates through the catalog but pin each import to an explicit release and
schema version (the October 2026 release moved to schema 2.0.0, renaming fields). Stage, validate and compare coverage/counts/schema/licenses before activation; an incompatible or
incomplete update leaves the previous edition active. Activate matching indexes and tiles together, invalidate
revision-keyed caches and retain rollback/provenance. Update schedules are optional; map browsing does not require
live catalog access. Changelogs and bridge files can assist reconciliation, but removals or identity changes must
not delete linked Mantle records or overwrite user-confirmed coordinates. Keep internal IDs plus an external
reference containing theme/type, feature or GERS ID and source edition. GERS stability varies; it is not a promise
that an identifier never changes. [GERS identity and change behavior](https://docs.overturemaps.org/gers/stability/)

**Address limitations.** At the November 2026 research date, Addresses remains alpha, has partial coverage in some
countries including the US, and may lack postal codes. Points can represent entrances, building/parcel centers or
road positions. Address IDs are not stable enough for the GERS registry. O2 therefore needs its own reconciliation
and region-quality checks, and must retain the precision/source rules in §6 rather than label every match exact or
derive a supposedly complete postal-code directory from incomplete address points.
[Addresses guide](https://docs.overturemaps.org/guides/addresses/),
[Places guide](https://docs.overturemaps.org/guides/places/)

## 5. ResourceFacade and tile streaming

One byte-access contract serves Media and Maps. Support **git on Postgres, git on filesystem, DbResource and plain
local filesystem**; Maps must not open paths, git objects or SQL blobs directly. A provider needing full archive
materialization is not considered ready for large PMTiles; diagnose it or use an explicit managed staging job.

The contract is the framework's, and most of it is built. `ResourceFacade::open` returns a `ByteSource`: a length,
a strong tag where the storage has one, and `read_at(offset, max)` with no reader state, so readers are
independent. One response path (`respond_opened`) serves it for `ec.web.sendResourceResponse` and static files.
What each backend does with a range today:

| Backend | A range reads only its bytes | Validator |
| --- | --- | --- |
| Plain file (`file`, `runtime`, `component`) | Yes, by seek | Weak (`mtime` and size); a registered archive's SHA-256 is read once when it is registered, and the map-source transition sends it as the strong tag (a change of its size or modification time is 409 until it is registered again) |
| DbResource over `dbresource-blob-threshold` (64 KiB) | Yes, the 1 MiB chunks it needs of the content blob | Strong (SHA-256) |
| Git on Postgres, an object over `db-large-object` (8 MiB) | Yes, from its content blob; a smaller object is read whole from its row | Strong |
| Git LFS, in either kind of repository | Yes, but the bytes are Postgres content blobs whatever the repository's storage | Strong |
| Git on filesystem, an object not in LFS | **No: the whole object is read for every request**, and a deflated object cannot be read by range | Strong (blob id) |
| Any `location#version` | Built in stage R: a version of an LFS object, a content blob or a large object opens by range with its strong tag; a small row or a filesystem git object is read whole | Strong (its content address) |

This work is **stage R** (§9): it has no dependency on the map widget, and
[HOME Media](../runtime/components/addon/mr-home/plan/initial-design.md#32-media-photos-music-videos-ebooks-and-audiobooks)
needs the same contract, so whichever of the two starts first builds it here rather than in its own plan. Built (stage R,
then the registration, the transition and the LFS rule with P2's archives, November 25, 2026: `org.moqui.maps.MapSourceServices`,
`MapSources.js`, `LocalMap.xml` `source`; [Maps › Archives](../docs/tools/Maps.md#archives)); the items were:

- **Git archives are LFS objects.** A map source registered in a git repository must be an LFS object, and
  readiness refuses another by name; the 512 MiB cap on a pushed non-LFS object (`git-server@max-object-size`)
  points the same way. Until the LFS placement item below is done, a filesystem repository's archive is read by
  range but from Postgres.
- **Pinned revisions by range.** A reader stays on its revision by the content address the registered source
  recorded (an LFS oid, a blob's SHA-256), or `open` returns a ranged source for a `#version`; never a whole
  version in memory.
- **A map-source transition** on the Maps screens, in the URL the registered source's key and its revision. It
  resolves the source and checks the caller's access on every request with no write (the Files download of BOSS
  records a `ResourceRecent` row each time, which hundreds of tile requests must not), then answers through
  `respond_opened`.
- **A strong validator for every backend.** The transition sends the checksum recorded with the registered
  revision as the `ETag`, since a plain file has only a weak one, and pmtiles.js disregards a weak tag when it checks
  for a replaced archive (confirmed in P0's spike against the vendored pmtiles 4.5.0: the Florence archive, served with
  a weak tag by the static handler, drew, and its tag was ignored), so only a pinned strong tag detects a replacement.
- **`Cache-Control: private` with the revision in the URL**, in place of the `no-store` user files carry, so a
  browser reuses the tile bytes it has, for `resource-facade` `pinned-max-age` (a day by default). After a share is
  revoked, the ranges a browser already has stay readable in that browser until the max-age passes; any range it
  does not have is a request to the server, refused at once. The transition answers through
  `ec.web.sendPinnedResourceResponse` with the `tag` and `size` `ec.resource.getContentAddress` gave when the source
  was registered, and content that is no longer that (a stale record) is refused with 409, never sent under the old
  tag.
- **`If-Range` by strong comparison** (it is compared as a string today, so a weak tag can satisfy it), and
  **cancellation** of a streamed read when the client goes away.

The authenticated HTTP resource adapter must:

1. Resolve an authorized registered source, then authorize each HEAD/GET/range request, including requests after
   a source/share is revoked. Do not expose arbitrary server paths or grant access to an entire drive/repository.
2. Return exact lengths and byte ranges, `206`, `Content-Range`, `Accept-Ranges`, validators and `416` for a range
   that starts past the end (all built). A request with several ranges, or a `Range` that does not parse, is
   answered with the whole file and `200`, as RFC 9110 allows; the PMTiles client asks for one range and treats a
   whole-file answer as an error. Bound parsing, concurrent requests, buffers and archive-directory decompression.
   Test the actual OpenLayers PMTiles integration against it.
3. Preserve archive byte offsets: no transparent HTTP recompression of the PMTiles payload (the host compresses
   only text types and never a partial response). Internal tile/archive compression is handled by the reader. Keep immutable revisions/ETags consistent across directory and tile reads;
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

Archive ingestion must expose each backend's practical size/quota limits. Current defaults are an upload of 2 GiB
(`webapp-list/webapp[webroot]/web-protection/requests@max-upload`), a git LFS object of 5 GiB
(`git-server@lfs-max-object-size`), a pushed non-LFS git object of 512 MiB (`git-server@max-object-size`) and a
drive's own `quotaBytes`; a large regional/planet archive can exceed them. Where the per-client web protection is
on, the `all` rate limit (50 requests a second, a burst of 500) also bounds the tile requests of several people
panning behind one address; measure a pan against it in P2 rather than raising it by default (measured by `MapsLocalOnly`, November 25, 2026: a pan of six steps and two zooms over a small archive asked 17 ranges in about 4 s, far under the limit; an archive with leaf directories asks a few more, and the browser keeps each range it read). Support operator-registered local files in place and bounded server-side download/import jobs, with explicit
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
| US Census Gazetteer | Initial US source: downloadable ZCTA records with representative/internal-point latitude and longitude, public domain. The yearly files (2026 at this review: `2026_Gaz_zcta_national.zip`, `2026_Gaz_counties_national.zip`, `2026_Gaz_place_national.zip` under `www2.census.gov/geo/docs/maps-data/data/gazetteer/2026_Gazetteer/`) are pipe-delimited text with a header line; the ZCTA file has `GEOID`, areas, `INTPTLAT` and `INTPTLONG` and no name or state, while Counties and Places have the state's USPS code, FIPS `GEOID` and `NAME`. A ZCTA approximates a ZIP delivery area for statistical purposes; it is not a USPS ZIP registry, and not every valid ZIP has a ZCTA. Preserve leading zeros, edition and this approximation. Missing/PO-box-only/new codes can remain unmatched. The 2020 ZCTA-to-county relationship file gives a ZCTA's county, and through it its state. |
| GeoNames postal downloads | Initial international source, and the US source of names: one file a country (124 at this review; `US.zip` is 619 KB) with postal codes, place name, three levels of admin name and code, WGS84 coordinates and an accuracy of 1 (estimated), 4 (the place's own) or 6 (centroid of addresses or shape); tab-delimited UTF-8, no header. `allCountries.zip` (19 MB) holds every country but only the first part of Canadian, Dutch and British codes, whose full codes are `CA_full`, `NL_full` and `GB_full` (the last under a Royal Mail notice). CC BY 4.0 with attribution; Chile, China, Ireland, Malta, Argentina and Brazil are partial codes or partial coverage, and other upstream notices vary by country. Some coordinates are inferred from neighboring places/codes. Import per country with its documented granularity, not a blanket worldwide accuracy claim. |
| GeoNames gazetteer | Global source of administrative divisions and cities: `countryInfo.txt`, `admin1CodesASCII.txt`, `admin2Codes.txt` and `cities15000.zip` (about 25,000 cities; `cities5000`, `cities1000` and `cities500` reach about 185,000), tab-delimited, CC BY 4.0. Its first-level codes are FIPS except for the US, Switzerland, Belgium and Montenegro, which are ISO, so a code is kept as the source's own and not written to an ISO field. |
| Other national datasets | Register country-specific import adapters for openly licensed or operator-supplied postal datasets, with explicit CRS, code granularity, update source and terms. For example, Ordnance Survey Code-Point Open provides postcode locations for Great Britain; evaluate its format and attribution for a national adapter. A non-WGS84 source needs a verified transformation before use. |
| US Census geocoder | Optional address-level provider, separate from downloads: returns coordinates calculated along address ranges in supported US territories. Do not label these rooftop/entrance positions. |
| Self-hosted Nominatim / Photon | Candidates for full address/place search and reverse geocoding. Photon also supplies autocomplete. Configure independently of basemap choice; operating these services is optional. |
| Optional Overture Addresses | Later regional import and local matching provider after Places/Buildings (§4.3, O2). Evaluate coverage, source precision and identifier changes; it supplements the postal fallback and is not an initial address-resolution dependency. |

Sources: [USPS Addresses](https://developers.usps.com/addressesv3),
[USPS City State Product](https://postalpro.usps.com/address-quality/city-state-product),
[Census Gazetteer downloads](https://www.census.gov/geographies/reference-files/time-series/geo/gazetteer-files.html),
[ZCTA definition and coverage](https://www.census.gov/programs-surveys/geography/guidance/geo-areas/zctas.html),
[ZCTA relationship file layout](https://www.census.gov/programs-surveys/geography/technical-documentation/records-layout/2020-zcta-record-layout.html),
[GeoNames postal dataset](https://download.geonames.org/export/zip/readme.txt),
[GeoNames gazetteer](https://download.geonames.org/export/dump/readme.txt),
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
- Seed a precision enumeration (user-confirmed, address match, street-range interpolation, postal-area
  representative point, place estimate), used by GeoPointAssoc and by Mantle's record-location provenance;
  `GeoPointType` has no values today.
- Derive IDs from the country and the code, as Mantle's states are (`USA_CA`): a postal code, a county and a city
  each get an ID an import can compute, and a point's ID adds its source. A re-import is then a create-or-update
  with no lookup. Confirmed in P0 (the comment above `GeoPostalCode` in `BasicEntities.xml`): a postal code
  `{country}_Z_{code}` (`USA_Z_02134`, the code upper case without spaces or hyphens, leading zeros kept) and a prefix
  code `{country}_ZP_{code}`; a US county `USA_C_{FIPS}` (`USA_C_25025`), a US place `USA_P_{FIPS}` and a GeoNames
  city `{country}_G_{geonameid}`; a point the Geo's ID, `_` and its data set's `sourceKey` (at most 12 characters,
  shared by a mirror or a later year: `USA_Z_02134_CENSUS_ZCTA`). A first- or second-level division is found by
  lookup, not computed: Mantle's install data has more than one form (`USA_CA`, `VNM_VN-SG`, `KOR_KR-11`, and
  `ITA_AG` a province) and GeoNames' first-level codes are FIPS for most countries, so the importer looks among the
  country's `GAT_REGIONS` children by `geoCodeAlpha2` or `geoCodeNumeric` (a GeoNames code through a crosswalk to ISO
  3166-2) and makes `{country}_A1_{code}` or `{country}_A2_{admin1}_{admin2}` only when nothing matches. A state the
  importer meets is then the row Mantle's install data already has, not a second one.
- A code's place names are `GeoPostalCodePlace` rows (code, source, name, its order in the file, admin names, the
  source's accuracy and an optional point of its own): GeoNames has a row a code and place, and no name wins. The
  code's one representative point from a source is its row with the highest accuracy (GeoNames 6, then 4, then 1),
  the first in the file among equals.

This is four rows a postal code (Geo, GeoPostalCode, GeoPoint, GeoPointAssoc) and one a place name
(GeoPostalCodePlace): about 135,000 for the US by the first estimate, about 318,500 as measured below (each code has a
point from each source that has it, and the county links come too), and
several million for every GeoNames country. The model is kept as it is for the US. P2 measures the US import, the
lookup and a refresh before the importer offers every country at once, and the international shape is decided on
those numbers.

**Measured November 25, 2026** (the acceptance test `GeoDataUsaImport`, release build, embedded Postgres, a 2026 Apple
laptop shared with other builds; the Census 2026 Gazetteer and 2020 relationship files and GeoNames `US.zip` as published
that day). The first import, downloads included, took 61 s: Counties 1.5 s (3,222 rows), ZCTAs 29.3 s (33,791), the
ZCTA-to-county file 16.4 s (47,863 rows, 33,486 links, 305 county parts outside every ZCTA passed over) and GeoNames US
13.7 s (41,490 rows, 41,488 written). In use after it, about 318,500 rows: 44,925 Geos (codes, counties), 41,637
GeoPostalCode, 78,501 GeoPointAssoc and as many GeoPoint (a point an association; a GeoNames place's own point beyond
its code's was not counted), 41,490 GeoPostalCodePlace and 33,486 county-to-code GeoAssoc links. A refresh that changes nothing took 1.3 s when the sources answered
conditionally (Counties, ZCTAs and GeoNames ended as current; the relationship file, which sends no validator, was read
and compared whole in 0.75 s) and 3.5 s forced (every file downloaded, read and compared: 0 created, updated or retired).
The postal lookup took 0.46 ms a code with its cache cleared and 0.11 ms cached (1,041 codes, every one found). Memory:
the spec records the status map's `Process MemoryUsed` and `MemoryPeak`, which are read from `/proc` and so are Linux's
only (null on macOS, where `ru_maxrss` or `/usr/bin/time -l` would give the peak of the whole test process, its data
load included). The figures here are the test server's resident size read with `ps -o rss` once a second by a sampler
beside the run, so a shorter peak can be missed: about 445 MB before the first import, at most about 520 MB during it,
and about 745 MB at the end of the forced GeoNames refresh.

**Every country.** One run reads one country, so the time, the rows and the memory of a run grow with that country's
codes, not with the number of countries: at the US rate (about 3,000 GeoNames rows a second), the 124 GeoNames countries (about 1.3 million rows, judged by `allCountries.zip`'s 19 MB against `US.zip`'s 619 KB
and 41,490 rows; with the US's about 7.7 database rows a code, about 10 million database rows) would load in roughly 8 minutes of runs. Importing
each country by itself stays as it is (Geo data's Import, one country a run). Every country at once may open as a queued
sequence of per-country runs, each its own run and edition, with the confirmation of §6.4 giving that estimate, once the
memory growth of a forced refresh (about 225 MB above the first import's peak for 41,000 codes) is explained: a run that
holds a country's records in memory would need GBs for the largest files (`GB_full` has well over a million codes),
which stay a separate, deliberate import. Until then the screen offers one country a run.

A framework postal lookup accepts country and postal code only; it does not need a street address or party ID.

### 6.4 Geo data importer

No reference data ships. A proposed importer, framework work in P2, brings a data set in when an administrator
asks for it from Maps › Configuration › Geo data: the server downloads the source, or takes a file the operator
supplies, validates it and loads it. A data set is never loaded at startup or in a test run. Which sets an
installation imports, how often it refreshes them and how long it keeps them remain its choices, subject to each
set's terms.

**Data sets and formats to support first:**

| Coverage | Data set | Gives | Format | Terms |
| --- | --- | --- | --- | --- |
| US | Census Gazetteer ZCTA | About 34,000 postal-area points | Zip; pipe-delimited text with a header | Public domain |
| US | Census Gazetteer Counties, and Places by choice | County and city Geos with FIPS codes, names, state and points | The same | Public domain |
| US | Census 2020 ZCTA-to-county relationship | The county, and through it the state, of each ZCTA (the largest land share where it spans several) | Pipe-delimited text with a header (it starts with a byte order mark), not zipped: `rel2020/zcta520/tab20_zcta520_county20_natl.txt`; a county's part outside every ZCTA has the ZCTA columns empty | Public domain |
| US and 123 other countries | GeoNames postal, a country at a time | Code, place name, state and county names and codes, point and accuracy; the ZIP codes that have no ZCTA | Zip; tab-delimited text, no header | CC BY 4.0, attribution shown |
| Global | GeoNames gazetteer: `countryInfo`, `admin1CodesASCII`, `admin2Codes`, `cities15000` (larger sets by choice) | States and provinces, second-level divisions and cities with points | Tab-delimited text, zipped or bare | CC BY 4.0 |
| Any | An operator's file: CSV with a header of our own (country, code, place, admin codes, latitude, longitude, precision) | A licensed or national data set the operator converted; an installation with no outside access | CSV from an upload or a resource location | The operator's |

That is three parsers (Census pipe-delimited, GeoNames tab-delimited, the operator CSV) over one delimited-text
reader that reads a member of a zip or a bare file. In the US the Census point is a code's active one where both
sources have it, and GeoNames adds the place names and the codes Census lacks; elsewhere GeoNames is the source
and its accuracy value sets the precision. Later, each with a parser of its own when a workflow needs it:
boundaries as GeoJSON into `Geo.wellKnownText` (Natural Earth, public domain;
[geoBoundaries](https://www.geoboundaries.org/api.html), whose API record names each boundary's own license),
GeoNames alternate names for localized Geo names, a country's national files (§6.2) and Overture Divisions with
O1 (§4.3).

**The registry.** A proposed GeoDataSet row for each source, as seed data: publisher, parser, URL template (with
`{country}` and `{year}`), coverage, what it yields, license, attribution and priority among sources. An
administrator can add a row for a mirror or a later year. A download goes only to the host of a registered row,
through `moqui_core::outbound` and its internal-address guard; a source that needs a credential is a connection
(§4.2).

**A run.** A proposed GeoDataImport row for each run: data set, country, edition (the source's year or its
`Last-Modified`), checksum, bytes, status, the counts (read, created, updated, unchanged, retired, rejected), who
ran it and when. Rejected rows go to an error file, as an ETL run's do, with the first of them kept as rows.

1. **Preflight.** Outbound requests are allowed (`maps@external-requests`), the data set's license has been
   acknowledged once by an administrator, the source is within `maps@import-max-bytes`, and no run of the same set
   and country is in progress.
2. **Download** to a file under `runtime/tmp` as it arrives, with the size cap, and conditionally, so a source
   that has not changed ends the run as current. An operator's file skips this step.
3. **Validate** the whole file, writing nothing: a bounded unzip, the columns, the country, the shape of each
   code (leading zeros kept), coordinate ranges and duplicates. A file with far fewer rows than the edition in
   use is refused unless the administrator forces it.
4. **Load** in batches of a few thousand codes to a transaction, skipping rows that have not changed. The counts
   are written after each batch and a cancel request is read between batches. A row that fails is counted and
   named in the error file, and the rest go on.
5. **Finish**, only after a whole load: end-date the codes the new edition no longer has, record the edition in
   use, clear the lookup cache and notify the user.

A run that fails or is cancelled leaves every earlier row in use and can be run again; a malformed or incomplete
download therefore never retires the edition in use. An import does not delete a Geo another record refers to,
and never touches a user-confirmed point, a record's own location or a Geo row another source owns.

**Where it runs.** As a `ServiceJob`, so a run has a job run, a notification and an optional schedule for
refreshes. The services are `framework/service/org/moqui/maps/GeoDataServices.xml`, declared `type="rust"`, with
the importer in `crates/moqui-tools` beside `etl` and the parsers in `moqui-lib` (§7). The ETL mapping engine is
not the vehicle: a mapping has one target and writes a row at a time as its user, with no download, no four-row
write and no retiring step. Its run record, error file and rerun conventions are followed, and cancel, which ETL
runs lack, is new here.

**Settings.** Maps › Configuration › Geo data, for `MAPS_ADMIN`: each data set with its license and attribution;
the countries imported, each with its edition, counts and last run; and Import, Refresh, Cancel, Schedule and
Remove (rows nothing refers to are deleted, the rest end-dated). Importing every GeoNames country at once asks
for confirmation with an estimate of the rows, and is not offered until the P2 measurement. Built: Import and a scope's
Refresh start a run of the `GeoDataImport` ServiceJob; Refresh all and the schedule are one `GeoDataRefresh` ServiceJob
that imports every scope again (an unchanged source costs a conditional request), scheduled weekly or monthly and run
as the administrator who scheduled it, rather than a job a data set (docs/tools/Maps.md "Geo data").

## 7. Useful Rust server work

Keep interactive rendering and ordinary small-format display in OpenLayers. The following work benefits from Rust
for streaming, bounded processing and shared use by browser, TUI, imports and APIs:

| Work | Scope and proposed placement |
| --- | --- |
| Resource range access and HTTP delivery | Stage R (§5): extend the existing `ByteSource` providers (`moqui-entity`, `moqui-git`), `ResourceFacade::open` and `respond_opened` in `moqui-core`, and the transport in `moqui-host`; all consumers share them. No map-specific bypass of provider storage or authentication. |
| Metadata, Geo data imports and lookup | Framework services for effective configuration, the Geo data importer (§6.4: download, bounded zip and delimited-text parsing, validation, batched load, retiring) and indexed Geo lookups. The importer is a module of `moqui-tools` beside `etl`, which is in every build; the three parsers, postal-code normalization and coordinate helpers belong in `moqui-lib`, as they need no ExecutionContext. |
| Provider adapters | Typed Rust clients for geocoding/normalization, timeouts, quotas, cancellation and allowed caching; configuration and authorization services remain the public boundary. Send through `moqui_core::outbound` (or `ec.service.rest()` from a script), with its internal-address guard, and let specs answer for the provider with `HttpFixtures`. Avoid separate JS proxies per app. |
| Archive metadata and optional tile extraction | Validate PMTiles headers/indexes and read metadata through ResourceFacade; a later Z/X/Y endpoint can extract a tile without loading the archive. Evaluate the [Rust pmtiles crate](https://docs.rs/pmtiles/latest/pmtiles/) against the facade's range-reader contract before choosing it. No server decoder is required just to stream the original archive. |
| Geometry/import jobs | Later, shared coordinate validation, extent calculation, reprojection, simplification and large-file normalization to GeoJSON/vector tiles. Evaluate Rust libraries and license/build costs against actual formats; no mandatory GDAL/PROJ or PostGIS dependency in the initial delivery. |
| Optional Overture ingestion | Bounded regional GeoParquet reads, schema/provenance validation, Places indexing, Buildings overlays and release reconciliation; later address candidate matching. Evaluate Rust Parquet/geometry tooling and prepared-data import options before adding dependencies. ResourceFacade owns input/derived storage; reuse or integrate tile-generation tooling rather than requiring a new Rust cartography engine. |
| Spatial data queries | Begin with bounded, indexed latitude/longitude queries and domain scope filters. Add spatial indexes/PostGIS or dynamic vector tiles when dataset size/query needs justify them; no unbounded download of all parties/assets to the client. |

Follow [Rust practices](../docs/dev/Practices-Rust.md) and [crate seams](archive/Crate-Split.md): reuse existing
lower-level modules and extension hooks. No new crate through P3: the work above goes in `moqui-lib`, `moqui-core`
and `moqui-tools`, with services declared `type="rust"`. A feature-gated `moqui-maps` extension crate, modelled on
`moqui-calc` under the host's `xlsx` feature, is decided when Overture's Parquet dependency arrives with O1, since
that weight should not be in every build; a new crate then has its own plan and the crate checklist
(`scripts/check-packages.sh`, `cargo lint-min`). `moqui-host` owns async/process edges and core must not depend
upward on it. Domain rules stay in Mantle, with XML Actions first for normal entity workflows, and Rust only for
the loops over thousands of rows and the binary formats. A new crate is not needed merely to serve a resource's
bytes.

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
| P0 — contracts and a spike | Confirm widget/dialog, profile/layer/provider schema, the adapter interfaces and permission boundaries, Geo extensions and ID scheme, and the `maps` conf element. Before the widget contract is fixed, a spike on a scratch page proves the two rendering paths on a desktop and a phone: an OpenFreeMap style through `ol-mapbox-style` with fonts served locally, and `PMTilesVectorSource` with a local style against a ranged resource URL (which also settles the weak-tag question of §5). Then vendor the selected OpenLayers/style/PMTiles versions and their assets/notices. Spike outcome (November 25, 2026): OpenFreeMap liberty and a Florence PMTiles extract both drew on desktop and at phone width, with fonts only from the page's origin once the webfonts template was absolute; `m-map` binds the archive's source itself (`applyStyle`, `updateSource: false`; §4.1); pmtiles ignored the static handler's weak tag, so stage R's pinned strong tag is required to detect a replaced archive (§5). |
| R — byte access | The to-build list of §5: LFS-only git archives with their readiness check, pinned revisions by range, the map-source transition with a strong validator and private caching, strong `If-Range`, cancellation. Independent of P1, and built by whichever of Maps and HOME Media starts first. |
| P1 — map and shared UI | Tools Maps full-screen browser, local dialog, personal preferences/saved views, explicit current-location action, the outline map of a fresh install and the OpenFreeMap provider an administrator enables. A user can open a record dialog without System, BOSS or standalone Maps app access, but cannot read an unauthorized record; with no Mantle in this stage, that rule is tested with a test adapter of the framework's own test services (`framework/test/MoquiConf.xml` and `framework/test/service/org/moqui/mapstest/`, loaded in `moqui test` runs only). Text/TUI/phone fallbacks work. |
| P2 — local data and archives | PMTiles through stage R on all four backends, packaged local styles/fonts/icons, the Geo data importer (§6.4) with its Census, GeoNames and operator-CSV parsers and its settings screen, a measured US import, provenance/precision and data refresh. Local browsing and postal lookup work with external requests disabled. Other GeoNames countries open after the measurement. Built (A7a, November 25, 2026): the postal lookup (`get#PostalLocation`) in Browse's search and as the local-map dialog's fallback for an address with no point, Maps › Configuration › Geo data (licenses, Import as a ServiceJob run, Refresh and its schedule, Cancel, Remove, run history), the `maps.geodata` readiness and refresh; measured US import (§6.3): 61 s first, 1.3 s conditional and 3.5 s forced refresh changing nothing, 41,637 codes and about 318,500 rows, lookup 0.46 ms cold; every country at once waits on the forced refresh's memory (§6.3), one country a run is offered. Built (A7b, November 25, 2026): PMTiles archives registered from a plain file, a drive (DbResource) or git as a Git LFS object (`register#ArchiveLayer`: header checked in Rust, revision, version, drive and size recorded; a non-LFS git archive refused, `NOT_LFS`), the map-source transition (`LocalMap.xml` `source`: access on every range with no write, 409 for a stale revision, pinned strong ETag and private caching) and the same-origin URL in the effective configuration, `m-map` loading its configuration again on a 409, Maps › Configuration › Archives, the `maps.archives` readiness, and the packaged local Protomaps profile (`LOCAL_PROTOMAPS`: the `@protomaps/basemaps` light and dark styles generated at build time, the basemaps-assets sprites, Noto Sans); acceptance `MapsArchiveBackends`, `MapsLocalOnly` and `MapsFrameworkOnly`. |
| P3 — consumer integration | Mantle adapters and BOSS/HOME mounts, basic geographic file overlays, optional configured street/place search, private/shared collections. HOME-only and dual-suite users see only permitted places; a viewer cannot save record coordinates. P0–P3 comprise the initial Maps scope. Built, the framework part (A8, November 25, 2026): personal overlays (`MapServices.store#Overlay`, `MapOverlays.js`; a GeoJSON, GPX or KML upload kept in `Maps/overlays` of the personal drive, or a file of a drive the user reads, read first by `MapSourceServices.convert#Overlay` in Rust, `moqui-lib` `geo/overlay.rs`: `maps@overlay-max-bytes` (10 MiB) within `maps@import-max-bytes`, `maps@max-features` (5000 when 0), 4 MiB of GeoJSON, WGS84 or a declared EPSG:3857, each geometry checked by itself, plain-text labels and descriptions, no network link, icon or reference followed), recorded as `moqui.maps.MapOverlay` and drawn from Browse's `overlay` transition with the reader's drive access checked on every request; saved places as `Maps/places.geojson` (`store#Place`); collections shared only by sharing the file in Files (`ResourceShare`), listed read-only in Browse's Layers, Saved views and search; the geocoder kinds `MPK_GEOCODER_CENSUS`, `MPK_GEOCODER_NOMINATIM` and `MPK_GEOCODER_PHOTON` with their fixed-query test, `find#Places` (one tested provider, never another on failure, outside requests, allowed operations, a shared cache policy and the rate (1 a second for Nominatim and Photon by default, per server) honoured, the answer read to 1 MiB with the RestClient's new `text(maxBytes)`) and its candidates third in Browse's search with source, precision and ambiguity; the disabled seed `CENSUS_GEOCODER`. Regression: `MapsOverlays`, `MapsGeocoder`, `geo/overlay_tests.rs`. Built, the Mantle part (A9): the record adapters `postalAddress`, `party`, `facility` and `asset` (mr-mantle `mantle.other.MapPlaceServices`, its `docs/MapAdapters.md`), each checking access itself (a party's business addresses only to others, a home address only to the person or their personal contact's owner; `PARTY_LOCATION_EDIT`, `FACILITY_EDIT`, `MAINTENANCE_RESOURCE_EDIT` to choose a location); the postal fallback never written to a record; saves as new points with `GeoPointProvenance`, refusing a stale version and an estimate over a recorded point; `PostalAddress.postalCodeGeoId` derived from the local Geo data; the bounded, organization-scoped record layers `MANTLE_FACILITIES` and `MANTLE_ASSETS`; `GooglePartyServices` superseded (regression: mr-mantle `MapPlaces`). Built, the BOSS part (A10): mr-boss `screen/Shared/LocalMap.xml`, LocalMap by the thin-wrapper rule, mounted under its app roots; View map on contact addresses, FacilityDetail and Maintenance's EquipmentDetail; the adapters' route keys resolved to BOSS screens; `PARTY_LOCATION_EDIT` for `BOSS_CONTACT_ADMIN`, while an internal organization's own places stay its settings manager's (`ADMIN_SETTINGS_MANAGE`), and Maps configuration rights read from the user's own grants (`isPermittedOwn`), so a mount's inherited root grant confers none (regression: mr-boss `RecordMap`). Not built: the HOME mount and the HOME-only and dual-suite visibility walks, which wait for mr-home's first screens. |
| P4 — richer GIS | Configured WMS/WMTS/WFS and additional imports/editing, COG/raster controls, advanced CRS workflows, large-data processing and optional detailed dashboard drill-downs. Prioritize concrete household/business scenarios. |
| Optional O1 — Overture Places and Buildings | After the shared source/import foundation, add selected regional Places search and Buildings overlays with attribution, pinned releases and staged updates. Initial Maps remains usable without these datasets. |
| Optional O2 — Overture regional addresses | After O1, evaluate and index regional address points; exercise ambiguous, missing and approximate matches plus refresh reconciliation. Retain postal/manual fallback and the existing record-save authority. |
| Later research | Routing, Google/other external maps, device integrations and “find my XYZ”, explicitly shared person-location feeds and offline mobile packages. Reuse the interface without treating approximate postal positions as navigation or tracking evidence. |

The checks below go in two suites, by [the testing rules](../docs/Build-Test.md#the-two-suites). A stage is done
when the regression tests of the rules it added pass within the component's budget and `moqui lint --screens` is
clean; its scenarios are acceptance tests to write, named here, and running them is not part of done.

- **Regression** (rules, each on a small fixture): range arithmetic, validators and `If-Range` in Rust beside
  `respond_opened`; the map-source transition for one holder, one non-holder and a revoked share; a non-LFS git
  archive refused by readiness; each parser on about ten rows (leading zeros kept, a bad row passed over, a
  partial-code country, a re-import that changes nothing, retiring only after a whole load, a download refused
  with outbound off and for a non-administrator), with `HttpFixtures` answering for the source; postal lookup and
  precision; an adapter's refusal of a non-holder and of another organization's record; preference precedence and
  a stale layer ID. Browse and LocalMap are each rendered once per mode with all their checks on that render;
  there is no screen sweep. A Mantle adapter's rules are tested in Mantle, not again through an app.
- **Acceptance**, each a named test run when asked: `MapsArchiveBackends` (the same viewport from an archive on
  each of the four backends, then its replacement and a revoked share), `MapsLocalOnly` (browsing and postal
  lookup with external requests disabled, and no outside request from opening a locally resolvable address),
  `MapsBrowseWalk` and `MapsDialogWalk` in a browser at desktop and phone width, the terminal walk,
  `MapsFrameworkOnly` (an installation without Mantle), `GeoDataUsaImport` (the real Census and GeoNames US
  downloads, recording time and memory), and with P3 the HOME-only and dual-suite visibility walks in their
  components.

Focused implementation checks should cover:

- Equivalent PMTiles viewport reads on Postgres git, filesystem git/LFS, DbResource and ordinary files; partial reads
  stay bounded, a range past the end is rejected, archive replacement invalidates readers, and revoked shares deny
  later requests. Use small fixtures, not a planet download.
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
- For optional Overture imports, use small regional fixtures to check Places/Buildings display and attribution,
  pinned schema/release handling, failed-update rollback, removed/changed external IDs and preservation of linked
  private records. O2 additionally checks incomplete address/postcode coverage, precision and ambiguous matches;
  refreshed indexes/tiles must agree on their active edition, with no production dependency on inspection tiles.

These are planned checks, not tests run for this document. Keep normal fixtures/specs well below the slow-test
threshold (a regression spec over 15 seconds fails the run); real-provider, whole-dataset or load scenarios are
acceptance tests, run only when asked.
