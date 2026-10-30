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

//! The framework's VCard and ICalendar tools on a booted factory, as a component's conf selects them (moved from
//! moqui-core's native_components test, whose runtime helpers are copied here).

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

use moqui_core::context::{BootOptions, Error, ExecutionContextFactory, Executor};
use moqui_core::driver::{DbConnection, DbDriver};
use moqui_core::entity::{EntityFacade, EntityGroup};
use moqui_core::transaction::{PoolConfig, TransactionManager};
use moqui_lib::error::Error as LibError;
use moqui_lib::object::Value;

struct NoDb;
impl DbDriver for NoDb {
    fn connect(&self) -> Result<Box<dyn DbConnection>, LibError> {
        Err(LibError::new("test must not access a database"))
    }
}
struct Runtime(PathBuf);
impl Runtime {
    fn new(component: &str, config: &str) -> Self {
        static NEXT: AtomicUsize = AtomicUsize::new(0);
        let root = std::env::temp_dir().join(format!(
            "mr-native-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        let framework = root.join("framework");
        let probe = root.join("component/probe");
        std::fs::create_dir_all(framework.join("conf")).unwrap();
        std::fs::create_dir_all(probe.join("service")).unwrap();
        std::fs::write(framework.join("component.xml"), "<component name=\"framework\"/>").unwrap();
        std::fs::write(
            framework.join("conf/MoquiDefaultConf.xml"),
            format!("<moqui-conf><component-list><component-dir location=\"component\"/></component-list><service-facade><service-file location=\"component://{component}/service/Probe.xml\"/></service-facade></moqui-conf>"),
        )
        .unwrap();
        std::fs::write(
            probe.join("component.xml"),
            format!("<component name=\"{component}\"/>"),
        )
        .unwrap();
        std::fs::write(
            probe.join("MoquiConf.xml"),
            format!("<moqui-conf>{config}</moqui-conf>"),
        )
        .unwrap();
        Self(root)
    }
    fn boot(&self) -> Result<Arc<ExecutionContextFactory>, Error> {
        ExecutionContextFactory::boot_with_options(
            &self.0,
            self.0.join("framework"),
            BootOptions {
                executor: Executor::Inline,
                ..BootOptions::default()
            },
        )
    }
    fn services(&self, xml: &str) {
        std::fs::write(self.0.join("component/probe/service/Probe.xml"), xml).unwrap();
    }
}
impl Drop for Runtime {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}
fn attach(factory: &ExecutionContextFactory) {
    let transactions = TransactionManager::new(
        Arc::new(NoDb),
        PoolConfig {
            min: 0,
            ..Default::default()
        },
    )
    .unwrap();
    let framework = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../runtime/base-component/framework");
    let facade = EntityFacade::open(
        &framework,
        "transactional",
        vec![EntityGroup::new("transactional", "postgres", transactions)],
    )
    .unwrap();
    factory.set_entity(facade);
}

/// The framework's VCard and ICalendar tools, selected as MoquiDefaultConf.xml selects them, reach a script with
/// plain values both ways.
#[test]
fn framework_contact_and_calendar_tools_reach_scripts() {
    let default_conf = std::fs::read_to_string(
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../../runtime/base-component/framework/conf/MoquiDefaultConf.xml"),
    )
    .unwrap();
    for class in [moqui_dav::pim::VCARD_CLASS, moqui_dav::pim::ICALENDAR_CLASS] {
        assert!(
            default_conf.contains(&format!("<tool-factory class=\"{class}\"")),
            "{class}"
        );
    }
    let runtime = Runtime::new(
        "native-probe",
        &format!(
            "<tools><tool-factory class=\"{}\"/><tool-factory class=\"{}\"/></tools>",
            moqui_dav::pim::VCARD_CLASS,
            moqui_dav::pim::ICALENDAR_CLASS
        ),
    );
    runtime.services(
        r#"<services>
      <service verb="use" noun="Tools" authenticate="false" transaction="ignore">
        <out-parameters><parameter name="text"/><parameter name="starts" type="List"/><parameter name="ics"/></out-parameters>
        <actions><script><![CDATA[
            const vcard = ec.factory.getTool('VCard')
            const cards = vcard.parse('BEGIN:VCARD\r\nVERSION:3.0\r\nFN:Vee Card\r\nN:Card;Vee;;;\r\nEMAIL;TYPE=work,pref:vee@example.com\r\nEND:VCARD\r\n')
            const written = vcard.write([{partyId: 'P1', firstName: 'Vee', lastName: 'Card', name: 'Vee Card',
                emails: [{emailAddress: 'vee@example.com', contactMechPurposeId: 'EmailPrimary'}]}], '4.0')
            text = [ec.factory.getToolFactory('VCard') != null, ec.factory.getToolFactory('Nope') == null, cards.length,
                cards[0].firstName, cards[0]['email1.label'], written.split('\r\n')[2]].join('|')
            const calendar = ec.factory.getTool('ICalendar')
            // 2027-05-10 10:00 in Chicago, weekly five times; the second week from the range
            starts = Array.from(calendar.expand('FREQ=WEEKLY;COUNT=5', Date.UTC(2027, 4, 10, 15), 'America/Chicago', ['20270524T100000'],
                null, Date.UTC(2027, 4, 16), Date.UTC(2027, 5, 30)))
            const parsed = calendar.parse(calendar.write({method: 'REQUEST', events: [{uid: 'u1', summary: 'Sync', start: Date.UTC(2027, 4, 10, 15),
                end: Date.UTC(2027, 4, 10, 16), timeZone: 'America/Chicago', rrule: 'FREQ=WEEKLY;COUNT=5',
                organizer: {name: 'Ada', email: 'ada@example.com'}, attendees: [{email: 'bob@example.com', rsvp: true}]}]}))
            ics = [parsed.method, parsed.events.length, parsed.events[0].timeZone, parsed.events[0].start === Date.UTC(2027, 4, 10, 15),
                parsed.events[0].attendees[0].partstat].join('|')
        ]]></script></actions>
      </service>
    </services>"#,
    );
    let factory = runtime.boot().unwrap();
    attach(&factory);
    factory.init_tools().unwrap();
    assert!(factory
        .get_tool_factory_typed::<moqui_dav::pim::VCardToolFactory>("VCard")
        .is_some());
    let ec = factory.new_context();
    let _entered = ec.enter();
    let out = moqui_core::service::sync("Probe.use#Tools", &BTreeMap::new()).unwrap();
    assert_eq!(
        out["text"],
        Value::string("true|true|1|Vee|* Work|UID:urn:moqui:mantle.party.Party:P1")
    );
    let week = 7 * 24 * 3_600_000i64;
    let first = 1_809_961_200_000i64; // 2027-05-10T15:00:00Z
    assert_eq!(
        out["starts"],
        Value::List(vec![
            Value::I64(first + week),
            Value::I64(first + 3 * week),
            Value::I64(first + 4 * week)
        ])
    );
    assert_eq!(out["ics"], Value::string("REQUEST|1|America/Chicago|true|NEEDS-ACTION"));
}
