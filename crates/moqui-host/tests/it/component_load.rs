/*
 * This software is in the public domain under CC0 1.0 Universal plus a
 * Grant of Patent License.
 *
 * To the extent possible under law, the author(s) have dedicated all
 * copyright and related and neighboring rights to this software to the
 * public domain worldwide. This software is distributed without any
 * warranty.
 *
 * You should have received a copy of the CC0 Public Domain Dedication
 * along with this software (see the LICENSE.md file). If not, see
 * <http://creativecommons.org/publicdomain/zero/1.0/>.
 */

//! A component's entities, rules, services, and data load as Java loads Mantle's: the entity directory joins
//! the model (an `extend-entity` of a framework entity too) and its `seed-data` loads before the data
//! directory; entity ECA rules there run, with `entityValue` shared with a service found by its file path and
//! `set-results` setting fields; a data file calls a service in its combined camel-case form; a nested record
//! gets a `fromDate` key of now and a secondary sequenced id; a row may name itself in a foreign key; and
//! number text may have grouping commas.

#![cfg(all(unix, feature = "embedded-postgres"))]

use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use moqui_core::driver::DbDriver;
use moqui_host::db::embedded::EmbeddedCluster;
use moqui_host::{load_data, LoadFlags};
use moqui_lib::object::{to_plain_string, Value};

const ENTITIES: &str = r#"<entities>
    <entity entity-name="GateOrg" package="gate.comp">
        <field name="orgId" type="id" is-pk="true"/>
        <field name="ownerOrgId" type="id"/>
        <field name="limitAmount" type="currency-amount"/>
        <field name="flag" type="text-indicator"/>
        <field name="location" type="text-short"/>
        <relationship type="one" title="Owner" related="gate.comp.GateOrg"><key-map field-name="ownerOrgId" related="orgId"/></relationship>
    </entity>
    <entity entity-name="GateOrgContact" package="gate.comp">
        <field name="orgId" type="id" is-pk="true"/>
        <field name="contactId" type="id" is-pk="true"/>
        <field name="fromDate" type="date-time" is-pk="true"/>
        <relationship type="one" related="gate.comp.GateOrg"/>
    </entity>
    <entity entity-name="GateOrgNote" package="gate.comp">
        <field name="orgId" type="id" is-pk="true"/>
        <field name="noteSeqId" type="id" is-pk="true"/>
        <field name="noteText" type="text-medium"/>
        <relationship type="one" related="gate.comp.GateOrg"/>
    </entity>
    <extend-entity entity-name="Enumeration" package="moqui.basic">
        <field name="gateExtra" type="text-short"/>
    </extend-entity>
    <seed-data>
        <moqui.basic.EnumerationType enumTypeId="GateType" description="Gate type"/>
        <moqui.basic.Enumeration enumId="GateEnum" enumTypeId="GateType" description="Gate enum" gateExtra="extended"/>
    </seed-data>
</entities>"#;

const RULES: &str = r#"<eecas>
    <eeca id="GateOrgFlag" entity="gate.comp.GateOrg" on-create="true" run-before="true">
        <actions><service-call name="gate.comp.GateServices.set#Flag" in-map="{entityValue: entityValue}"/></actions>
    </eeca>
    <eeca id="GateOrgLocation" entity="gate.comp.GateOrg" on-create="true" run-before="true" set-results="true">
        <actions><set field="location" value="HQ"/></actions>
    </eeca>
</eecas>"#;

const SERVICES: &str = r#"<services>
    <service verb="set" noun="Flag">
        <in-parameters><parameter name="entityValue" type="Map"/></in-parameters>
        <actions><set field="entityValue.flag" value="Y"/></actions>
    </service>
    <service verb="init" noun="Org">
        <in-parameters><parameter name="orgId" required="true"/></in-parameters>
        <actions><service-call name="create#gate.comp.GateOrg" in-map="{orgId: orgId, ownerOrgId: 'ORG_A'}"/></actions>
    </service>
</services>"#;

const DATA: &str = r#"<entity-facade-xml type="seed">
    <gate.comp.GateOrg orgId="ORG_A" ownerOrgId="ORG_A" limitAmount="118,500">
        <gate.comp.GateOrgContact contactId="C1"/>
        <gate.comp.GateOrgNote noteText="first note"/>
    </gate.comp.GateOrg>
    <gate.comp.GateServices.initOrg orgId="ORG_B"/>
</entity-facade-xml>"#;

#[test]
fn a_component_loads_its_entities_rules_services_and_data() {
    if !crate::common::own_process() {
        return;
    }
    let runtime = temp_dir("moqui-comp-rt");
    let cluster_dir = temp_dir("moqui-comp-pg");
    let _guards = (DirGuard(runtime.clone()), DirGuard(cluster_dir.clone()));
    let component = runtime.join("component/gate-comp");
    for (path, text) in [
        ("component.xml", r#"<component name="gate-comp"/>"#),
        ("entity/GateEntities.xml", ENTITIES),
        ("entity/Gate.eecas.xml", RULES),
        ("service/gate/comp/GateServices.xml", SERVICES),
        ("data/GateData.xml", DATA),
    ] {
        let file = component.join(path);
        std::fs::create_dir_all(file.parent().unwrap()).unwrap();
        std::fs::write(file, text).unwrap();
    }
    std::fs::create_dir_all(&cluster_dir).unwrap();
    let framework = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../runtime/base-component/framework");
    load_data(&runtime, &framework, &cluster_dir, &LoadFlags::default()).unwrap();

    let (cluster, driver) = EmbeddedCluster::start(&cluster_dir, 10).unwrap();
    let mut conn = driver.connect().unwrap();
    let text = |row: &moqui_core::driver::DbRow, name: &str| row.get(name).map(to_plain_string).unwrap_or_default();

    let orgs = conn
        .query(
            "SELECT org_id AS \"orgId\", flag, location, limit_amount AS \"limitAmount\" FROM gate_org ORDER BY org_id",
            &[],
        )
        .unwrap();
    assert_eq!(
        orgs.len(),
        2,
        "ORG_A from data, ORG_B from the service the data file calls"
    );
    assert_eq!(text(&orgs[0], "flag"), "Y", "the rule's service set entityValue.flag");
    assert_eq!(text(&orgs[0], "location"), "HQ", "set-results");
    assert_eq!(text(&orgs[0], "limitAmount").parse::<f64>().unwrap(), 118_500.0);
    assert_eq!(text(&orgs[1], "orgId"), "ORG_B");

    let contacts = conn
        .query("SELECT from_date AS \"fromDate\" FROM gate_org_contact", &[])
        .unwrap();
    assert_eq!(contacts.len(), 1);
    assert!(!matches!(contacts[0].get("fromDate"), None | Some(Value::Null)));
    let notes = conn
        .query("SELECT note_seq_id AS \"noteSeqId\" FROM gate_org_note", &[])
        .unwrap();
    assert_eq!(text(&notes[0], "noteSeqId"), "01");

    let enums = conn
        .query(
            "SELECT gate_extra AS \"gateExtra\" FROM enumeration WHERE enum_id = 'GateEnum'",
            &[],
        )
        .unwrap();
    assert_eq!(
        text(&enums[0], "gateExtra"),
        "extended",
        "entity seed-data and extend-entity"
    );
    drop(conn);
    drop(cluster);
}

/// A path under `/tmp`. The runtimes share one Postgres install in `target/<profile>/moqui-test-install`, rather than
/// each unpacking its own.
fn temp_dir(prefix: &str) -> PathBuf {
    static SHARED: std::sync::Once = std::sync::Once::new();
    SHARED.call_once(|| {
        let exe = std::env::current_exe().unwrap();
        let profile_dir = exe.ancestors().nth(2).expect("target directory");
        moqui_lib::env::set_property(
            "MOQUI_INSTALL",
            profile_dir.join("moqui-test-install").display().to_string(),
        );
    });
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    let name = format!("{prefix}-{}-{nanos}", std::process::id());
    if Path::new("/tmp").is_dir() {
        PathBuf::from("/tmp").join(name)
    } else {
        std::env::temp_dir().join(name)
    }
}

struct DirGuard(PathBuf);

impl Drop for DirGuard {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}
