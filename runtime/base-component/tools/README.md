# tools

The administration apps, mounted under `/apps` (and `/qapps`) by `MoquiConf.xml`: Dashboards, Monitor
([docs/tools/Monitoring.md](../../../docs/tools/Monitoring.md)), System and Tools. Their access is the artifact groups
of `data/ToolsSecurityData.xml` and `data/MonitoringSecurityData.xml`, granted to `ADMIN`.

## Tools

`screen/Tools.xml`, grouped by section. The guide to Data, SQL, Schema, ETL and Database is
[docs/tools/Database.md](../../../docs/tools/Database.md); the design is
[plans/archive/Tools-Database.md](../../../plans/archive/Tools-Database.md).

| Screen | What it is |
| --- | --- |
| `Data/` | Grid (`Grid.qvue`: the data grid on an entity, a view, a data document, a SQL result or an uploaded CSV or spreadsheet, edited and saved in place, with saved views; a file has Load into…) and Record (one record's form), Browse (a record and its related records), Views (`DbViewEntity` views), Load, Export (entity data and data documents; the home of `DocumentExport.json`), Snapshots |
| `Sql.xml`, `Sql.qvue` | The SQL editor: statements run read only or with changes confirmed, results in a grid, explain, history and saved queries. `SqlTui.xml` is its terminal and plain-page form, `SqlDownload.xml` a result as XLSX. Needs `SQL_RUNNER_WEB` |
| `Schema/` | Entities and one Entity (with its table's differences), Check Tables, Indexes and keys (the model against the database, planned as jobs), Download DDL |
| `Etl.xml`, `Etl/` | ETL: Mappings, a Mapping (`Mapping.qvue`: source, target, fields with sample values, test, run, schedule), Runs and a Run (errors, error file, rerun), Value Maps; `Etl.xml` holds the JSON transitions the mapping form and the grid's Load into… use |
| `Database/` | Overview, Activity (sessions, blockers, index health), Queries, Tables (with maintenance), Jobs and a job's page, Settings, Speed Test, and Connections (external databases: `DATABASE_EXTERNAL`, never on production) |
| `Service/`, `StatusFlows.xml`, `ArtifactStats.xml` | Services, status flows, artifact statistics |

Data › Load and Snapshots can drop the foreign keys of the loaded tables first and make them again after: the load
runs on a worker as the `DatabaseLoadWrap` ServiceJob (`service/moqui/tools/DatabaseLoadServices.xml`, `data/ToolsDatabaseData.xml`).

The Data, SQL, Schema, ETL and Database screens take a `db` parameter, `group:<groupName>` (or `ext:<connectionId>` where an external database may be used), kept as the user
preference `database.selected`; `template/DatabaseParts.xml` has the picker.

## Tests

```sh
./moq test tools                      # the regression specs in test/ (budget in test/budget)
./moq accept tools                    # the acceptance specs in acceptance/
```
