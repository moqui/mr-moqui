# Moqui Java Migration Notes

Notes for moving a Moqui Java deployment, or a component written for it, to MR Moqui. This guide covers names and
locations. The rest is documented elsewhere:

- The deliberate redesigns (JavaScript for scripts and templates, SearchFacade, html-print, database cluster
  coordination and sessions, keys as configuration): the root [README](../../README.md#significant-redesigns-vs-moqui-java).
- Smaller deliberate differences inside ported features: [Parity-Diffs](../../plans/Parity-Diffs.md). Features left out
  on purpose: [Parity-Mapping, Excluded on purpose](../../plans/Parity-Mapping.md#excluded-on-purpose).
- Converting a component's Groovy to JavaScript (`moqui convert --groovy`, `moqui lint --groovy`, hand ports): the Mantle
  record in [ConvertProcess](../../plans/archive/ConvertProcess.md) and the Marble one in
  [MarbleERPMigration](../../plans/archive/MarbleERPMigration.md). Each converted component keeps its own
  `MIGRATION-STATUS.md`.
- A component's native Rust code, which takes the place of a Gradle build and jars: [Native-Components](Native-Components.md).
- `./moq` in place of the Gradle tasks: the root [README](../../README.md#build-test-and-run).

## Component and repository renames

The Java components that MR Moqui has ported were first forked under `mr-`/`Mr` names and kept their Java
component.xml names. On 2026-10-25 they were renamed and merged so that each repository, its directory under
`runtime/component`, and its component.xml name are the same:

| Moqui Java component(s) | Fork repositories (until 2026-10-25) | MR Moqui component and repository |
|---|---|---|
| `mantle-udm` + `mantle-usl` | `mr-mantle-udm` + `mr-mantle-usl` | `mr-mantle` |
| `SimpleScreens` + `MarbleERP` | `MrSimpleScreens` + `MrMarbleERP` | `mr-marble` |
| (none; new) | `ironstone-boss` | `mr-boss` (the IronStone BOSS application) |

`mr-mantle` is the renamed `mr-mantle-usl` repository with `mr-mantle-udm` merged in. `mr-marble` is the renamed
`MrSimpleScreens` repository with `MrMarbleERP` merged in. Both merges keep the commit history of the two repositories
(`git log --follow` works on the merged-in files, whose paths did not change). The two merged-in repositories are
retired. On GitHub the renamed repositories redirect from their old names. The framework repository is `mr-moqui`.

### What changes in a component that uses them

- **`depends-on`**: `mantle-udm` and `mantle-usl` become one `<depends-on name="mr-mantle"/>`. `SimpleScreens` and
  `MarbleERP` become `<depends-on name="mr-marble"/>`.
- **Locations**: `component://mantle-udm/…` and `component://mantle-usl/…` become `component://mr-mantle/…`. The two
  had no paths in common, so the rest of the location is unchanged. `component://SimpleScreens/…` and
  `component://MarbleERP/…` become `component://mr-marble/…`, as in
  `component://mr-marble/screen/SimpleScreens/Order/FindOrder.xml`. This applies wherever a location is written: screen
  `location` and `extends` attributes, `subscreens-item`, `include-screen`, `widget-template-include`, form
  `extends`, script and template locations, and data files.
- **Component names in commands and files**: `./moq component-get mr-mantle mr-marble mr-boss`, `./moq test mr-mantle
  mr-marble`, `moqui load components=mr-mantle`, a `myaddons.xml` entry, and the `components` of a
  `test/client/suite.js` server.
- **Test dependencies**: a spec that needed `mantle-usl` and `SimpleScreens` selected together now needs `mr-mantle` and
  `mr-marble`. One component, `mr-mantle`, now covers what took two selections before.

### What stays the same

These are names inside the components, not component names:

- **Screen paths and mounts.** The Simple Screens tree is still `screen/SimpleScreens/` with its root
  `SimpleScreens.xml`. The test mount is `/apps/SimpleScreens`. Marble ERP is still `/apps/marble` and `/qapps/marble`,
  and My Account is still `/apps/my`.
- **Data and security IDs.** These include the artifact groups `SimpleScreensAccounting`, `SimpleScreensOrder` and the
  rest, the `MarbleErp*` records, and the setup file `SimpleScreensSetupData.xml` (Marble's is now `ZmeMarbleErpSetupData.xml`, so that it loads after the Simple Screens artifact groups it uses).
- **Entities and services.** These are `mantle.*` entities and services, and the `ironstone.*` services of mr-boss.
- **Spec names.** For example, `SimpleScreensRender` is still `SimpleScreensRender`.

### Existing databases

Some records hold component locations, so a database loaded before the rename still names the old components. Examples:

- `moqui.security.ArtifactGroupMember.artifactName`
- `moqui.screen.ScreenDocument.screenLocation`
- `moqui.screen.SubscreensItem.subscreenLocation`
- `moqui.basic.email.EmailTemplate` screen locations
- any location your own data stores

On a development or test database, run `./moq clean-db` and load again. On a database worth keeping, update the
location prefixes with SQL, as in this example:

```sql
UPDATE artifact_group_member
   SET artifact_name = regexp_replace(artifact_name, '^component://(SimpleScreens|MarbleERP)/', 'component://mr-marble/')
 WHERE artifact_name ~ '^component://(SimpleScreens|MarbleERP)/';
UPDATE artifact_group_member
   SET artifact_name = regexp_replace(artifact_name, '^component://mantle-u(sl|dm)/', 'component://mr-mantle/')
 WHERE artifact_name ~ '^component://mantle-u(sl|dm)/';
```

Use the same kind of update for `component://ironstone-boss/` to `component://mr-boss/`, and run it on each table and
column that holds a location. Then clear the caches (or restart).

A database moved from Moqui Java also keeps its `artifact_hit` and `artifact_hit_bin` tables. They are no longer
entities here: OpenTelemetry spans and metrics replace them ([Monitoring](../tools/Monitoring.md)). The tables are left as they
are, unused. Once you no longer need the old rows, and after a backup, drop them:

```sql
DROP TABLE IF EXISTS artifact_hit;
DROP TABLE IF EXISTS artifact_hit_bin;
```

The old `clean_ArtifactData_daily` and `clean_ElasticSearch*Messages_daily` job registrations are paused at start;
their past runs stay.
