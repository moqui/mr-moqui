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
            <ssh-server port="2222"/>
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

#[test]
fn a_renamed_attribute_is_refused_naming_its_replacement_from_a_file_or_the_environment() {
    for (retired, replacement, hint) in super::RETIRED_ATTRIBUTES {
        let (element, attribute) = retired.split_once('@').unwrap();
        let xml = match element.split_once('/') {
            Some((outer, inner)) => {
                format!("<moqui-conf><{outer}><{inner} {attribute}=\"4318\"/></{outer}></moqui-conf>")
            }
            None => format!("<moqui-conf><{element} {attribute}=\"4318\"/></moqui-conf>"),
        };
        let file = MNode::parse_text("moqui-conf", &xml).unwrap();
        let err = super::refuse_retired(&file).unwrap_err().to_string();
        assert!(err.contains(retired) && err.contains(replacement), "{err}");
        assert!(err.contains(&super::env_name_of(replacement)), "{err}");
        assert!(err.contains(hint), "{err}");
        // a set while running is refused the same way (plans/Runtime-Config.md §8.2)
        let path = moqui_lib::node::path::AttrPath::parse(retired).unwrap();
        let refused = super::retired_attribute(&path).unwrap();
        assert!(refused.contains(replacement) && refused.contains(hint), "{refused}");
    }
    // the SSH login dialog's flag names the access mode each value became (plans/Runtime-Config.md §3.4)
    let dialog = MNode::parse_text(
        "moqui-conf",
        r#"<moqui-conf><ssh-server login-dialog="false"/></moqui-conf>"#,
    )
    .unwrap();
    let err = super::refuse_retired(&dialog).unwrap_err().to_string();
    assert!(
        err.contains("ssh-server@access") && err.contains("false is access=\"login\""),
        "{err}"
    );
    let access = moqui_lib::node::path::AttrPath::parse("ssh-server@access").unwrap();
    assert_eq!(super::retired_attribute(&access), None);
    // the environment form: moqui_conf__pass_authority__grpc_port adds the attribute to the merged conf
    let conf = MNode::parse_text(
        "moqui-conf",
        "<moqui-conf><pass-authority grpc-enabled=\"true\"/></moqui-conf>",
    )
    .unwrap();
    assert!(super::refuse_retired(&conf).is_ok());
    let _ = apply_from(
        &conf,
        vec![name(
            "moqui_conf__pass_authority__grpc_port",
            "4318",
            Source::Environment,
        )],
    );
    let err = super::refuse_retired(&conf).unwrap_err().to_string();
    assert!(
        err.contains("moqui_conf__pass_authority__grpc_port") && err.contains("grpc-listen-port"),
        "{err}"
    );
    // the current names are not refused
    let current = MNode::parse_text(
        "moqui-conf",
        r#"<moqui-conf><webapp-list><webapp name="webroot" grpc-listen-port="4317" grpc-listen-bind=""/></webapp-list>
            <telemetry><receiver grpc-enabled="true"/></telemetry></moqui-conf>"#,
    )
    .unwrap();
    assert!(super::refuse_retired(&current).is_ok());
}

/// A retired value is refused by name wherever it is set (plans/Runtime-Config.md §8.2): in a conf file, under a keyed
/// element or not, and by an environment name, which the merged conf holds too; the message names the attribute and its
/// environment name, and the replacement and its own. Another value of the attribute is not refused.
#[test]
fn a_retired_value_is_refused_from_a_file_or_the_environment() {
    use moqui_lib::node::path::{self, AttrPath};
    const TABLE: &[super::RetiredValue] = &[
        (
            "webapp-list/webapp[webroot]@rc1-listen-port",
            &["off", ""],
            "webapp-list/webapp[webroot]@rc1-listen-enabled",
            "rc1-listen-enabled=\"false\" turns it off and keeps the port",
        ),
        ("rc1-server@port", &[""], "rc1-server@enabled", ""),
    ];
    let keyed = AttrPath::parse(TABLE[0].0).unwrap();
    let file = MNode::parse_text("moqui-conf", "<moqui-conf/>").unwrap();
    let target = path::resolve(&file, &keyed, &crate::context::conf_schema::keying, true).unwrap();
    target.node.set_attribute(target.attribute.clone(), " OFF ");
    let err = super::refuse_retired_with(&file, TABLE).unwrap_err().to_string();
    for named in [
        "webapp-list/webapp[webroot]@rc1-listen-port",
        "moqui_conf__webapp_list__webapp__WEBROOT__rc1_listen_port",
        "webapp-list/webapp[webroot]@rc1-listen-enabled",
        "moqui_conf__webapp_list__webapp__WEBROOT__rc1_listen_enabled",
        "\"OFF\"",
        "keeps the port",
    ] {
        assert!(err.contains(named), "{named} not in {err}");
    }
    target.node.set_attribute(target.attribute.clone(), "8080");
    assert!(super::refuse_retired_with(&file, TABLE).is_ok());
    target.node.set_attribute(target.attribute.clone(), "");
    let err = super::refuse_retired_with(&file, TABLE).unwrap_err().to_string();
    assert!(err.contains("set empty"), "{err}");

    // the environment form, in any spelling of the name
    let conf = MNode::parse_text(
        "moqui-conf",
        r#"<moqui-conf><webapp-list><webapp name="webroot"/></webapp-list></moqui-conf>"#,
    )
    .unwrap();
    assert!(super::refuse_retired_with(&conf, TABLE).is_ok());
    let _ = apply_from(
        &conf,
        vec![name(
            "MOQUI_CONF__WEBAPP_LIST__WEBAPP__WEBROOT__RC1_LISTEN_PORT",
            "off",
            Source::Environment,
        )],
    );
    let err = super::refuse_retired_with(&conf, TABLE).unwrap_err().to_string();
    assert!(
        err.contains("moqui_conf__webapp_list__webapp__WEBROOT__rc1_listen_port") && err.contains("rc1-listen-enabled"),
        "{err}"
    );

    // what a set while running checks, by the path it resolved
    let server = AttrPath::parse("rc1-server@port").unwrap();
    assert!(super::retired_value_in(TABLE, &server, " ").is_some());
    assert!(super::retired_value_in(TABLE, &server, "22").is_none());
    assert!(super::retired_value_in(TABLE, &AttrPath::parse("rc1-server@host").unwrap(), "").is_none());
}

/// Each retired value's row names attributes with environment names, its values trimmed and in lower case as they are
/// compared.
#[test]
fn every_retired_value_row_names_attributes_and_compares_its_values() {
    for (selector, values, replacement, _) in super::RETIRED_VALUES {
        assert!(!super::env_name_of(selector).is_empty(), "{selector}");
        assert!(!super::env_name_of(replacement).is_empty(), "{replacement}");
        for value in *values {
            assert_eq!(*value, value.trim().to_ascii_lowercase(), "{selector}");
        }
    }
}

/// The web listeners' old spellings of "off" (a port of `off` or empty) are refused, from a conf file or by an
/// environment name, naming the switch that replaced them; a written port is not (plans/Runtime-Config.md §8.2).
#[test]
fn the_web_listeners_old_spellings_of_off_are_refused() {
    for listener in ["http", "https", "grpc"] {
        for retired in ["off", " OFF ", ""] {
            let file = MNode::parse_text(
                "moqui-conf",
                &format!(
                    r#"<moqui-conf><webapp-list><webapp name="webroot" {listener}-listen-port="{retired}"/></webapp-list></moqui-conf>"#
                ),
            )
            .unwrap();
            let err = super::refuse_retired(&file).unwrap_err().to_string();
            let switch = format!("webapp-list/webapp[webroot]@{listener}-listen-enabled");
            assert!(
                err.contains(&switch) && err.contains(&super::env_name_of(&switch)),
                "{listener} {retired:?}: {err}"
            );
        }
        let written = MNode::parse_text(
            "moqui-conf",
            &format!(
                r#"<moqui-conf><webapp-list><webapp name="webroot" {listener}-listen-enabled="false" {listener}-listen-port="0"/></webapp-list></moqui-conf>"#
            ),
        )
        .unwrap();
        assert!(super::refuse_retired(&written).is_ok(), "{listener}");
        let conf = MNode::parse_text(
            "moqui-conf",
            r#"<moqui-conf><webapp-list><webapp name="webroot"/></webapp-list></moqui-conf>"#,
        )
        .unwrap();
        let _ = apply_from(
            &conf,
            vec![name(
                &format!("moqui_conf__webapp_list__webapp__WEBROOT__{listener}_listen_port"),
                "off",
                Source::Environment,
            )],
        );
        let err = super::refuse_retired(&conf).unwrap_err().to_string();
        assert!(err.contains(&format!("{listener}-listen-enabled")), "{err}");
    }
}

/// Two spellings of one environment name resolve to one attribute; a name of no conf attribute to none.
#[test]
fn an_environment_name_resolves_to_its_attribute_in_any_spelling() {
    use moqui_lib::node::path::AttrPath;
    let canonical = super::env_name_path("moqui_conf__webapp_list__webapp__WEBROOT__https_listen_enabled").unwrap();
    let shouted = super::env_name_path("MOQUI_CONF__WEBAPP_LIST__WEBAPP__WEBROOT__HTTPS_LISTEN_ENABLED").unwrap();
    assert_eq!(super::compare_key(&canonical), super::compare_key(&shouted));
    let selector = AttrPath::parse("webapp-list/webapp[webroot]@https-listen-enabled").unwrap();
    assert_eq!(super::compare_key(&canonical), super::compare_key(&selector));
    assert!(super::env_name_path("MOQUI_CONF").is_none());
    assert!(super::env_name_path("cluster_ssh_port").is_none());
}

/// SSH's and SMTP's old "off", an empty port, is refused naming the switch; a written port with the switch off is not
/// (plans/Runtime-Config.md §8.2).
#[test]
fn an_empty_ssh_or_smtp_port_is_refused() {
    for (element, port) in [("ssh-server", "port"), ("smtp-server", "listen-port")] {
        let empty = MNode::parse_text(
            "moqui-conf",
            &format!(r#"<moqui-conf><{element} {port}=""/></moqui-conf>"#),
        )
        .unwrap();
        let err = super::refuse_retired(&empty).unwrap_err().to_string();
        let switch = format!("{element}@enabled");
        assert!(
            err.contains(&switch) && err.contains(&super::env_name_of(&switch)),
            "{element}: {err}"
        );
        let written = MNode::parse_text(
            "moqui-conf",
            &format!(r#"<moqui-conf><{element} enabled="false" {port}="2222"/></moqui-conf>"#),
        )
        .unwrap();
        assert!(super::refuse_retired(&written).is_ok(), "{element}");
    }
}

/// Call media's empty port, its old "off", is refused naming its switch; an empty plain port of SIP, XMPP or TURN stays
/// valid, that one transport off beside the other (plans/Runtime-Config.md §8.2).
#[test]
fn an_empty_media_port_is_refused_and_an_empty_comms_transport_port_is_not() {
    let empty = MNode::parse_text("moqui-conf", r#"<moqui-conf><media-server port=" "/></moqui-conf>"#).unwrap();
    let err = super::refuse_retired(&empty).unwrap_err().to_string();
    assert!(
        err.contains("media-server@enabled") && err.contains(&super::env_name_of("media-server@enabled")),
        "{err}"
    );
    let transports = MNode::parse_text(
        "moqui-conf",
        r#"<moqui-conf><xmpp-server enabled="true" port="" tls-port="5223"/><sip-server enabled="true" port=""
            tls-port="5061"/><media-server enabled="true" port="10000"><turn-server enabled="true" port=""
            tls-port="5349"/></media-server></moqui-conf>"#,
    )
    .unwrap();
    assert!(super::refuse_retired(&transports).is_ok());
}
