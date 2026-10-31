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

use super::*;

/// A Postgres server the conf names gets the `schema-name` as its search path and the isolation level as the session's
/// default, a space in it escaped for the startup options.
#[cfg(feature = "postgres")]
#[test]
fn a_postgres_session_takes_the_schema_and_the_isolation_level() {
    use moqui_core::entity::datasource::IsolationLevel;

    let text = r#"<moqui-conf>
        <entity-facade><datasource group-name="transactional" database-conf-name="postgres" schema-name="app"/></entity-facade>
        <database-list>
            <database name="postgres" default-isolation-level="ReadCommitted">
                <inline-jdbc><xa-properties serverName="db.example" databaseName="moqui"/></inline-jdbc>
            </database>
        </database-list>
    </moqui-conf>"#;
    let mut conf = datasource_conf(&MNode::parse_text("test", text).unwrap(), "transactional").unwrap();
    assert_eq!(
        session_options(&conf).unwrap().as_deref(),
        Some("-c search_path=app,public -c default_transaction_isolation=read\\ committed")
    );
    conf.schema.clear();
    conf.isolation_level = Some(IsolationLevel::Serializable);
    assert_eq!(
        session_options(&conf).unwrap().as_deref(),
        Some("-c default_transaction_isolation=serializable")
    );
    conf.isolation_level = None;
    assert_eq!(session_options(&conf).unwrap(), None);
}
