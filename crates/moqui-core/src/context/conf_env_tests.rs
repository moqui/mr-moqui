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

use moqui_lib::node::MNode;

use super::{apply_from, generate_secrets, Outcome, Source};

fn conf() -> MNode {
    MNode::parse_text(
        "moqui-conf",
        r#"<moqui-conf>
            <webapp-list><webapp name="webroot" http-port="8080"/></webapp-list>
            <entity-facade crypt-pass="MoquiDefaultPassword:CHANGEME">
                <datasource group-name="transactional" database-conf-name="h2"/>
                <load-data location="a"/>
            </entity-facade>
            <ssh-server port=""/>
        </moqui-conf>"#,
    )
    .unwrap()
}

fn name(name: &str, value: &str, source: Source) -> (String, String, Source) {
    (name.to_string(), value.to_string(), source)
}

#[test]
fn a_name_sets_its_attribute_over_the_files() {
    let conf = conf();
    let applied = apply_from(
        &conf,
        vec![
            name(
                "moqui_conf__webapp_list__webapp__WEBROOT__http_port",
                "8091",
                Source::Environment,
            ),
            name(
                "MOQUI_CONF__ENTITY_FACADE__DATASOURCE__TRANSACTIONAL__DATABASE_CONF_NAME",
                "postgres",
                Source::DotEnv,
            ),
        ],
    );
    assert_eq!(applied[0].outcome, Outcome::Set);
    assert_eq!(
        applied[0].selector.as_deref(),
        Some("webapp-list/webapp[webroot]@http-port")
    );
    let webapp = conf.first_named("webapp-list").unwrap().first_named("webapp").unwrap();
    assert_eq!(webapp.attribute("http-port").as_deref(), Some("8091"));
    let datasource = conf
        .first_named("entity-facade")
        .unwrap()
        .first_named("datasource")
        .unwrap();
    assert_eq!(datasource.attribute("database-conf-name").as_deref(), Some("postgres"));
}

#[test]
fn a_higher_source_wins_and_two_spellings_in_one_source_are_reported() {
    let conf = conf();
    let applied = apply_from(
        &conf,
        vec![
            name("moqui_conf__ssh_server__port", "2222", Source::Property),
            name("MOQUI_CONF__SSH_SERVER__PORT", "2223", Source::Property),
            name("moqui_conf__ssh_server__port", "2224", Source::DotEnv),
        ],
    );
    assert_eq!(
        conf.first_named("ssh-server").unwrap().attribute("port").as_deref(),
        Some("2222")
    );
    assert_eq!(
        applied[1].outcome,
        Outcome::Shadowed {
            by: "moqui_conf__ssh_server__port".to_string(),
            same_source: true
        }
    );
    assert!(matches!(
        applied[2].outcome,
        Outcome::Shadowed { same_source: false, .. }
    ));
}

#[test]
fn a_name_adds_what_the_files_do_not_have() {
    let conf = conf();
    let applied = apply_from(
        &conf,
        vec![
            name("moqui_conf__instance__purpose", "dev", Source::Environment),
            name("moqui_conf__ssh_server__prot", "2222", Source::Environment),
        ],
    );
    assert_eq!(applied[0].outcome, Outcome::Added { element_existed: false });
    assert_eq!(
        conf.first_named("instance").unwrap().attribute("purpose").as_deref(),
        Some("dev")
    );
    // the files have the element: most often a misspelled attribute, which the boot warns of
    assert_eq!(applied[1].outcome, Outcome::Added { element_existed: true });
}

#[test]
fn a_name_that_cannot_be_set_is_refused_and_a_secret_is_known() {
    let conf = conf();
    let applied = apply_from(
        &conf,
        vec![
            name(
                "moqui_conf__entity_facade__load_data__location",
                "b",
                Source::Environment,
            ),
            name(
                "moqui_conf__entity_facade__crypt_pass",
                "a-real-pass",
                Source::Environment,
            ),
        ],
    );
    assert!(matches!(applied[0].outcome, Outcome::Refused(_)));
    assert!(applied[1].secret);
    assert_eq!(applied[1].outcome, Outcome::Set);
}

#[test]
fn a_secret_at_changeme_is_generated_into_dotenv_unless_an_override_set_it() {
    let dir = std::env::temp_dir().join(format!("moqui-conf-env-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let env_path = dir.join(".env");
    let _ = std::fs::remove_file(&env_path);

    let first = conf();
    let made = generate_secrets(&first, &[], &env_path).unwrap();
    assert_eq!(made, vec!["entity-facade@crypt-pass".to_string()]);
    let pass = first
        .first_named("entity-facade")
        .unwrap()
        .attribute("crypt-pass")
        .unwrap();
    assert!(!pass.ends_with("CHANGEME"));
    let text = std::fs::read_to_string(&env_path).unwrap();
    assert!(text.contains(&format!("moqui_conf__entity_facade__crypt_pass={pass}\n")));
    // out of the loaded dotenv table again, which other tests of this process read
    moqui_lib::env::update_dotenv_file(&env_path, &[], &["moqui_conf__entity_facade__crypt_pass"]).unwrap();

    let second = conf();
    let overrides = apply_from(
        &second,
        vec![name(
            "moqui_conf__entity_facade__crypt_pass",
            "CHANGEME",
            Source::Environment,
        )],
    );
    assert!(generate_secrets(&second, &overrides, &env_path).unwrap().is_empty());
    let _ = std::fs::remove_dir_all(&dir);
}
