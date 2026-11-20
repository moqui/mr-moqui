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

use super::{apply_from, Outcome, Source};

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

/// S4: a secret at `CHANGEME` is made into the seal, never into `runtime/.env` or the conf; one an override set is
/// not made; and a boot with no seal (a production instance before init) keeps it in memory only, writing nothing.
#[test]
fn a_secret_at_changeme_is_made_into_the_seal_never_dotenv_unless_an_override_set_it() {
    use crate::seal::{secrets, Keeper, Mode, SealConf};
    let dir = std::env::temp_dir().join(format!("moqui-conf-env-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let keeper = Keeper::open(&crate::seal::file::path_in(&dir), SealConf::with_purpose("dev")).unwrap();
    keeper.start(Mode::Unattended).unwrap();

    let first = conf();
    secrets::prepare_boot(&first, &[], Some(&keeper)).unwrap();
    let kept = keeper
        .bootstrap_secret(secrets::CRYPT_PASS)
        .unwrap()
        .expect("the crypt pass is in seal.json");
    assert_eq!(kept.len(), 44, "32 random bytes in base64");
    let pass = first
        .first_named("entity-facade")
        .unwrap()
        .attribute("crypt-pass")
        .unwrap();
    assert!(pass.ends_with("CHANGEME"), "the conf keeps no secret");
    assert!(!dir.join(".env").exists());
    // the next start keeps the one it made
    secrets::prepare_boot(&conf(), &[], Some(&keeper)).unwrap();
    assert_eq!(keeper.bootstrap_secret(secrets::CRYPT_PASS).unwrap().unwrap(), kept);

    let second = conf();
    let overrides = apply_from(
        &second,
        vec![name(
            "moqui_conf__entity_facade__crypt_pass",
            "given-by-the-operator",
            Source::Environment,
        )],
    );
    secrets::prepare_boot(&second, &overrides, Some(&keeper)).unwrap();
    let given = second
        .first_named("entity-facade")
        .unwrap()
        .attribute("crypt-pass")
        .unwrap();
    assert_eq!(given, "given-by-the-operator", "the environment's value stands");
    assert_eq!(keeper.bootstrap_secret(secrets::CRYPT_PASS).unwrap().unwrap(), kept);

    // runtime/.env, which the server's own user writes, sets no secret once there is a seal other than the dev seal:
    // the store answers it. Under the dev seal (a dev checkout's, a test run's) it is still read.
    let dotenv_pass = |keeper: &Keeper| {
        let third = conf();
        let overrides = apply_from(
            &third,
            vec![name(
                "moqui_conf__entity_facade__crypt_pass",
                "written-into-dotenv",
                Source::DotEnv,
            )],
        );
        secrets::prepare_boot(&third, &overrides, Some(keeper)).unwrap();
        third
            .first_named("entity-facade")
            .unwrap()
            .attribute("crypt-pass")
            .unwrap()
    };
    assert_eq!(dotenv_pass(&keeper), "written-into-dotenv");
    let guarded_dir = dir.join("shamir");
    let guarded = Keeper::open(&crate::seal::file::path_in(&guarded_dir), SealConf::with_purpose("dev")).unwrap();
    guarded
        .init(crate::seal::Init {
            method: crate::seal::MethodKind::Shamir,
            passphrase: None,
        })
        .unwrap();
    assert!(guarded.refuses_dotenv_secrets() && !keeper.refuses_dotenv_secrets());
    assert_eq!(dotenv_pass(&guarded), "CHANGEME");

    let unsealed = dir.join("production");
    let ephemeral = Keeper::open(
        &crate::seal::file::path_in(&unsealed),
        SealConf::with_purpose("production"),
    )
    .unwrap();
    ephemeral.start(Mode::Unattended).unwrap();
    secrets::prepare_boot(&conf(), &[], Some(&ephemeral)).unwrap();
    assert!(!unsealed.join(".env").exists() && !unsealed.join("db/seal.json").exists());
    let _ = std::fs::remove_dir_all(&dir);
}
