# Maps

Maps is the framework's map app, beside Dashboards, System and Tools (`runtime/components/base/tools`, mounted as
`/apps/maps` and `/qapps/maps`). It needs no BOSS, HOME, PASS or Mantle: those components add record adapters and
mount its dialog under their own routes. The design is [plans/Maps.md](../../plans/Maps.md); the map widget
(`map-view`, `m-map`) is in [Qvt-Client](../ui/Qvt-Client.md) and the terminal's `map` component in
[TUI](../ui/TUI.md#components).

A fresh installation makes no outside request: maps draw the Natural Earth country outline from this server until an
administrator enables a map provider.

## The screens

| Screen | What it is |
| --- | --- |
| Browse (`Maps/Browse.xml`, `Browse.qvue`, `Browse.tui.json`) | The map filling the page, with optional browser full screen. One panel at a time beside it, a bottom sheet on a phone: Search (saved places and the permitted records of the record layers shown), Layers (the basemap, overlays and record layers; a layer that is not shown is named with Retry; a record layer cut to `maps@max-features` says so), Saved views (and Save view, which keeps where the map is, its basemap and its layers) and the selected feature's details. The scale, attribution, coordinate readout and Locate me are the map's. A qvue screen: under `/apps` it opens at the same path under `/qapps`. In the terminal, the `map` component over `terminalMap`: the saved places and records as a list over the braille land mask, the notices and the attribution |
| Saved views (`Maps/Views.xml`) | The user's saved views and those others shared, each with its place, owner and whether its layers are all available; Share, Stop sharing and Delete for one's own, View map for any |
| Preferences (`Maps/Preferences.xml`) | The user's map profile, basemap, initial area, units, the layers shown and Restore my last view. Only what an administrator enabled is offered, and a profile's `allowedOverrides` decide what a preference changes. No location is asked for unless Locate me is pressed, and the last view is kept only while its restoring is on |
| Configuration (`Maps/Configuration.xml`, route key `mapsConfiguration`) | For `MAPS_ADMIN`: readiness, the outside-request policy and limits (read from the conf), map providers (connections of the `mapProvider` kind: a provider's page is `Configuration/ConnectionDetail` over `tools/template/ConnectionParts.xml`), profiles (enable, disable, make the installation default) and layers; Archives (`Configuration/Archives.xml`, [below](#archives)); and Geo data (`Configuration/GeoData.xml`, [below](#geo-data)). Its settings index entry is `maps.configuration` |
| Local map (`Maps/LocalMap.xml`) | The dialog below |

Search reads saved places, the records of the record layers shown within the view on screen (each read through its
adapter) and the postal codes of the local Geo data ([Postal lookup](#postal-lookup)): a text that looks like a postal
code (a digit, letters, spaces and hyphens), of the country chosen beside the search (a list of the countries with postal
data, which then shows the country searched), or one named in the text (`US 02134`, `02134, US`) when it has postal data
and its rules read the rest as a code (so `MA 02134` is not Morocco while only US data is there). Otherwise the user's
country is tried first and then every other country with postal data whose rules read the text as a code; a code found
in more than one country lists each, labeled with its country, and says so. A postal result goes to the code's broad
area, not a close zoom, and a code with several places lists each as a choice. A configured place or address service joins the `search` transition
later. Going to a result changes nothing in its record.

## Access

Three artifact groups in `tools/data/MapsSecurityData.xml`:

- `MAPS_APP`: the app's root, for `MAPS_USERS` and `ADMIN`. `MAPS_APP_SERVICES` gives the same groups the preference and
  saved-view services, each acting on the signed-in user's own.
- `MAPS_DIALOG`: LocalMap and its transitions, the effective configuration, the user's own preferences
  (`get#Preferences`), the postal lookup (`get#PostalLocation`, public reference data only), the map-source check (`MapSourceServices.get#SourceAccess`, [Archives](#archives)) and the adapter registry, for every signed-in user (`ALL_USERS`, always-allow, not inherited). Opening a record's map needs neither the Maps app, System
  nor any app's access.
- `MAPS_ADMIN`: Configuration, its `find#Configuration` and `store#ProfileSettings`, the archive services (`read#ArchiveHeader`,
  `register#ArchiveLayer`, `find#ArchiveLayers`) and the Geo data services, `ADMIN`'s;
  `MAPS_USERS` is denied it. A profile setting changes only with Configuration's update (view alone reads it), and a
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
maps), `maps@max-features` (the most features one record layer or viewport sends) and `maps@import-max-bytes`. A page
`Content-Security-Policy` stricter than the default needs each provider's host in `connect-src`, `img-src` and
`font-src`; the readiness checks name what to add.

## Archives

A PMTiles archive on this server draws a map with no outside request (plans/Maps.md §4.1, §5). The seed has the local
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
modification time, which also makes the map-source transition answer 409 until it is registered again).

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

**Limits.** An upload is at most 2 GiB (`webapp-list/webapp[webroot]/web-protection/requests@max-upload`), a Git LFS object
5 GiB (`git-server@lfs-max-object-size`), and a drive has its own `quotaBytes`, which a replacement needs room in for both
revisions until the old content is collected; Archives shows each against the archive's size. A larger regional or
planet archive is placed on the server's disk and registered there, not uploaded. Where the per-client web protection is
on, its `all` rate limit (50 requests a second, a burst of 500) counts every range a map asks for, for everyone behind one
address together. A pan of six steps and two zooms asked 17 ranges in about four seconds (`MapsLocalOnly`), and a
range the browser has read is not asked again while it keeps it.

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
per row).

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
- **Open in Maps** shows only to a user whose own grants open the Maps app (`isPermittedOwn`: an app's grant on the
  screen that shows the dialog does not count). Keyboard focus returns to View map when the dialog
  closes, and on a phone the dialog fills the screen.

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

Leave `storeLocation` out where the app offers no Choose location. The app's mount narrows what is shown; it never grants
authority, and the dialog does not need the app's other grants or System's.

### Record adapters

A component registers an adapter as a `moqui.maps.MapAdapter` seed row: a key and its services, which implement
`org.moqui.maps.MapServices` `get#RecordMapContext` (label, geometry, precision, source, `recordVersion`,
`permittedActions` of `view` and `chooseLocation`, links; with no geometry, the address's `postalCode` and `countryGeoId`
for the postal fallback), `store#RecordLocation` (answering `applied` false with
`STALE_VERSION`, `PERMISSION_DENIED` or `RECORD_UNAVAILABLE`) and `find#LayerFeatures` for a record layer (a `MapLayer`
with format `MLF_ADAPTER`). The component grants its adapter services to the users who open the dialog; a browser sends
only the key. The framework's test adapter (`framework/test/service/org/moqui/mapstest`) is the example the specs use.

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
  notifies the administrator when it ends (topic `GeoDataImport`). A set read a country at a time asks for the country:
  one country a run, among those the data set has files for (its `countryCodes`: GeoNames' 124), and every GeoNames
  country at once is not offered. An operator's file is uploaded (at most `maps@import-max-bytes`, refused before it is
  written when the import could not start) and kept under the administrator's own folder,
  `dbresource://moqui/maps/import/(userId)/`; an operator's file is read only from the caller's own folder there, from
  `runtime://import/` or from their own uploads. A completed import of an upload deletes the uploads of the scope's
  earlier runs, and Remove deletes them all. A download needs `maps@external-requests`. The Import dialog is loaded
  when opened (`GeoData/ImportDialog.xml`).
- A run downloads conditionally (an unchanged source ends as current), reads and checks the whole file writing nothing,
  loads in batches of 2,000 codes, and only after a whole load end-dates what the new edition no longer has. A file with
  far fewer rows than the edition in use is refused unless forced. Rejected rows go to the run's error file. A failed or
  cancelled run leaves the edition in use as it was. The importer never changes a user-confirmed point, a record's own
  location, a code made by hand or another source's rows.
- **Imported** lists each data set and country with its edition in use, its postal codes and points in use and its last
  run, with **Refresh** (that scope again; an operator's file is the one its last run read), **Cancel** (a run stops at
  its next step, retiring nothing) and **Remove** (`remove#GeoData`: the rows nothing refers to are deleted, the rest
  end-dated; not offered for countries and divisions, whose rows are shared).
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
of one area (a code with several is `ambiguous`, and they are offered as choices, none chosen); and a broad extent around
the point, which is not the area's boundary (sizing it from the Census ZCTA's land area, which the importer does not keep
yet, is a later improvement). Puerto Rico, the US Virgin Islands, Guam, American Samoa and the Northern Mariana Islands
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

## Tests

The regression spec is `tools/test/MapsScreens.spec.js` (Browse in qvue and tui; LocalMap, a plain XML screen, once in
qvt for a writer and once in html for a user it refuses the record; the access rules, Save location, the profile
rules); the configuration services' rules are `framework/test/MapsConfig.spec.js`, and the archives'
(registering, the map-source transition's access, ranges, revocation and stale revisions) `framework/test/MapsSource.spec.js`,
with the header's in Rust (`moqui-lib` `pmtiles_tests.rs`). The archive backends, local-only browsing and an installation
without Mantle are the acceptance tests `MapsArchiveBackends`, `MapsLocalOnly` and `MapsFrameworkOnly`
(`./moq accept framework --spec MapsArchiveBackends`). Geo data: the importer's rules are
`framework/test/GeoDataImport.spec.js`, the postal lookup's `framework/test/MapsPostalLookup.spec.js`, and the Geo data
screen and its Import `tools/test/MapsGeoData.spec.js`; the real US import, with its times and counts, is the acceptance
test `GeoDataUsaImport` (`./moq accept framework --spec GeoDataUsaImport`).
The acceptance walks are `MapsBrowseWalk` and `MapsDialogWalk` (browser, desktop and phone width) and
`MapsTerminalWalk`, with their fixture `MapsWalkFixtures`: `./moq accept tools --spec MapsBrowseWalk`.
