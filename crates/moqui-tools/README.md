# moqui-tools

The server side of the Tools app's Data, SQL, Schema, ETL and Database sections: the Rust services of
`org.moqui.impl.DatabaseServices` (`runtime/base-component/framework/service/org/moqui/impl/DatabaseServices.xml`),
on the entities of `framework/entity/DatabaseEntities.xml`. A framework crate on `moqui-core`, registered through
`moqui_core::extension` and linked in every host build; the browser build leaves it out.

- `src/capabilities.rs`: `get#Capabilities`, what a database allows and why the rest is not available, cached five
  minutes per database.
- `src/catalog.rs`: the probe and catalog SQL, a constant per dialect. The schema comparison's queries are in
  `crates/moqui-db/src/sql/` instead.

Test: `cargo test -p moqui-tools` (the capabilities test starts an embedded cluster).

Design and the steps still to come: [plans/Tools-Database.md](../../plans/Tools-Database.md).
