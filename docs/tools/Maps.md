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
| Configuration (`Maps/Configuration.xml`, route key `mapsConfiguration`) | For `MAPS_ADMIN`: readiness, the outside-request policy and limits (read from the conf), map providers (connections of the `mapProvider` kind: a provider's page is `Configuration/ConnectionDetail` over `tools/template/ConnectionParts.xml`), profiles (enable, disable, make the installation default) and layers. Its settings index entry is `maps.configuration` |
| Local map (`Maps/LocalMap.xml`) | The dialog below |

Search reads saved places, and the records of the record layers shown within the view on screen (each read through its
adapter); postal-code lookup of the local Geo data and a configured place or address service join its `search`
transition as further sources. Going to a result changes nothing in its record.

## Access

Three artifact groups in `tools/data/MapsSecurityData.xml`:

- `MAPS_APP`: the app's root, for `MAPS_USERS` and `ADMIN`. `MAPS_APP_SERVICES` gives the same groups the preference and
  saved-view services, each acting on the signed-in user's own.
- `MAPS_DIALOG`: LocalMap and its transitions, the effective configuration, the user's own preferences
  (`get#Preferences`) and the adapter registry, for every signed-in user (`ALL_USERS`, always-allow, not inherited). Opening a record's map needs neither the Maps app, System
  nor any app's access.
- `MAPS_ADMIN`: Configuration, its `find#Configuration` and `store#ProfileSettings` and the Geo data services, `ADMIN`'s;
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

## The local-map dialog

An app opens LocalMap beside an address or coordinate field with View map, as a `dynamic-dialog`; its id is
`LocalMapDialog`, or another passed as `dialogId`:

```xml
<dynamic-dialog id="LocalMapDialog" button-text="View map" icon="sym_o_map" title="${partyName}"
        transition="LocalMap" parameter-map="{adapterKey: 'party', recordKey: JSON.stringify({partyId}), addressPurpose: 'PostalPrimary'}"/>
```

Its parameters are a record's `adapterKey`, `recordKey` (its key fields as JSON) and an optional `addressPurpose`, or a
plain `longitude`, `latitude` and `label` (with `pick=true` to let the user choose a point), and `dialogId` when the
dialog's id is not `LocalMapDialog` (a list with one per row).

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
`permittedActions` of `view` and `chooseLocation`, links), `store#RecordLocation` (answering `applied` false with
`STALE_VERSION`, `PERMISSION_DENIED` or `RECORD_UNAVAILABLE`) and `find#LayerFeatures` for a record layer (a `MapLayer`
with format `MLF_ADAPTER`). The component grants its adapter services to the users who open the dialog; a browser sends
only the key. The framework's test adapter (`framework/test/service/org/moqui/mapstest`) is the example the specs use.

## Tests

The regression spec is `tools/test/MapsScreens.spec.js` (Browse in qvue and tui; LocalMap, a plain XML screen, once in
qvt for a writer and once in html for a user it refuses the record; the access rules, Save location, the profile
rules); the configuration services' rules are `framework/test/MapsConfig.spec.js`.
The acceptance walks are `MapsBrowseWalk` and `MapsDialogWalk` (browser, desktop and phone width) and
`MapsTerminalWalk`, with their fixture `MapsWalkFixtures`: `./moq accept tools --spec MapsBrowseWalk`.
