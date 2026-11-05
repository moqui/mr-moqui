# tools

The administration apps, mounted under `/apps` (and `/qapps`) by `MoquiConf.xml`: Dashboards, Monitor
([docs/tools/Monitoring.md](../../../docs/tools/Monitoring.md)), System and Tools. Their access is the artifact groups
of `data/ToolsSecurityData.xml` and `data/MonitoringSecurityData.xml`, granted to `ADMIN`.

## Tools

`screen/Tools.xml`, grouped by section ([plans/Tools-Database.md](../../../plans/Tools-Database.md) §3):

| Screen | What it is |
| --- | --- |
| `Data/` | Grid (an entity's records) and Record (one record's form), Browse (a record and its related records), Views (`DbViewEntity` views), Load, Export (entity data and data documents; the home of `DocumentExport.json`), Snapshots |
| `Sql.xml` | SQL on a database: one statement with its results, or a script. Needs `SQL_RUNNER_WEB` |
| `Schema/` | Entities and one Entity, with Check Tables |
| `Etl.xml` | Data mappings; not in the menu yet |
| `Database/` | Overview, Tables, Queries, Speed Test |
| `Service/`, `StatusFlows.xml`, `ArtifactStats.xml` | Services, status flows, artifact statistics |

The Data, SQL, Schema, ETL and Database screens take a `db` parameter, `group:<groupName>`, kept as the user
preference `database.selected`; `template/DatabaseParts.xml` has the picker.

## Tests

```sh
./moq test tools                      # the regression specs in test/ (budget in test/budget)
./moq accept tools                    # the acceptance specs in acceptance/
```
