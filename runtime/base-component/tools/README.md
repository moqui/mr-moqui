# tools

The administration apps, mounted under `/apps` (and `/qapps`) by `MoquiConf.xml`: Dashboards, Monitor
([docs/tools/Monitoring.md](../../../docs/tools/Monitoring.md)), System and Tools. Their access is the artifact groups
of `data/ToolsSecurityData.xml` and `data/MonitoringSecurityData.xml`, granted to `ADMIN`.

## Tools

`screen/Tools.xml`, grouped by section ([plans/Tools-Database.md](../../../plans/Tools-Database.md) §3):

| Screen | What it is |
| --- | --- |
| `Data/` | Grid (`Grid.qvue`: the data grid on an entity, a view, a data document or a SQL result, edited and saved in place, with saved views) and Record (one record's form), Browse (a record and its related records), Views (`DbViewEntity` views), Load, Export (entity data and data documents; the home of `DocumentExport.json`), Snapshots |
| `Sql.xml`, `Sql.qvue` | The SQL editor: statements run read only or with changes confirmed, results in a grid, explain, history and saved queries. `SqlTui.xml` is its terminal and plain-page form, `SqlDownload.xml` a result as XLSX. Needs `SQL_RUNNER_WEB` |
| `Schema/` | Entities and one Entity (with its table's differences), Check Tables, Indexes and keys (the model against the database, planned as jobs), Download DDL |
| `Etl.xml` | Data mappings; not in the menu yet |
| `Database/` | Overview, Activity (sessions, blockers, index health), Queries, Tables (with maintenance), Jobs and a job's page, Speed Test |
| `Service/`, `StatusFlows.xml`, `ArtifactStats.xml` | Services, status flows, artifact statistics |

Data › Load and Snapshots can drop the foreign keys of the loaded tables first and make them again after: the load
runs on a worker as the `DatabaseLoadWrap` ServiceJob (`service/moqui/tools/DatabaseLoadServices.xml`, `data/ToolsDatabaseData.xml`).

The Data, SQL, Schema, ETL and Database screens take a `db` parameter, `group:<groupName>`, kept as the user
preference `database.selected`; `template/DatabaseParts.xml` has the picker.

## Tests

```sh
./moq test tools                      # the regression specs in test/ (budget in test/budget)
./moq accept tools                    # the acceptance specs in acceptance/
```
