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

use std::collections::BTreeSet;
use std::path::Path;

use super::{argv, available, conf_profiles, directory, environment, own_profiles, EnvFor, Profile};

const CONF: &str = r#"<moqui-conf>
    <default-property name="instance_purpose" value="dev"/>
    <default-property name="entity_ds_password" value=""/>
    <terminal>
        <host-profile name="shell" title="My shell" integration="true"/>
        <host-profile name="sh" title="POSIX" command="sh" args="-c 'x'" login="true"/>
        <host-profile name="nowhere" command="no-such-program-here"/>
        <host-profile name="log" command="tail" args='["-F", "runtime/log/moqui.log"]'>
            <env name="LESS" value="R"/>
        </host-profile>
    </terminal>
</moqui-conf>"#;

#[test]
fn profiles_are_the_shell_then_those_on_the_path_then_the_users_own() {
    let own = r#"[{"name": "agent", "command": "sh", "args": ["-l"], "resumeArgs": "--resume", "agent": true},
        {"name": "log", "command": "sh"}, {"title": "no name"}]"#;
    let profiles = available(CONF, Some(own));
    let names: Vec<&str> = profiles.iter().map(|profile| profile.name.as_str()).collect();
    assert_eq!(
        names,
        vec!["shell", "sh", "log", "agent"],
        "nowhere is not on the PATH; the conf's log stays"
    );
    assert_eq!(profiles[0].title, "My shell");
    assert!(profiles[0].integration && profiles[0].login && profiles[0].command.is_none());
    let log = conf_profiles(CONF)
        .into_iter()
        .find(|profile| profile.name == "log")
        .unwrap();
    assert_eq!(log.args, vec!["-F", "runtime/log/moqui.log"]);
    assert_eq!(log.env, vec![("LESS".to_string(), "R".to_string())]);
    let agent = own_profiles(own)
        .into_iter()
        .find(|profile| profile.name == "agent")
        .unwrap();
    assert!(agent.own && agent.agent);
    assert_eq!(agent.resume_args, Some(vec!["--resume".to_string()]));
}

#[test]
fn a_login_shell_is_named_with_a_dash_and_resume_args_take_the_args_place() {
    let profile = Profile {
        name: "sh".to_string(),
        command: Some("sh".to_string()),
        args: vec!["-i".to_string()],
        resume_args: Some(vec!["--continue".to_string()]),
        login: true,
        ..Profile::default()
    };
    let (args, arg0) = argv(&profile, false);
    assert!(args[0].ends_with("/sh"), "resolved on the PATH: {args:?}");
    assert_eq!((&args[1..], arg0.as_deref()), (&["-i".to_string()][..], Some("-sh")));
    assert_eq!(&argv(&profile, true).0[1..], &["--continue".to_string()]);
}

#[test]
fn the_directory_is_the_asked_then_the_profiles_then_the_confs_then_above_the_runtime() {
    let tmp = std::env::temp_dir();
    let runtime = tmp.join("checkout").join("runtime");
    let profile = Profile {
        directory: Some("/no/such/dir".to_string()),
        ..Profile::default()
    };
    assert_eq!(directory(Some(tmp.to_str().unwrap()), &profile, "", &runtime), tmp);
    let profile = Profile {
        directory: Some(tmp.to_str().unwrap().to_string()),
        ..Profile::default()
    };
    assert_eq!(directory(None, &profile, "/", &runtime), tmp);
    assert_eq!(directory(None, &Profile::default(), "/", &runtime), Path::new("/"));
    // neither the checkout nor the runtime is there: the runtime path, as given
    assert_eq!(directory(None, &Profile::default(), "", &runtime), runtime);
}

#[test]
fn the_environment_leaves_out_the_confs_properties_and_sets_the_terminals() {
    let server = vec![
        ("PATH".to_string(), "/usr/bin".to_string()),
        ("entity_ds_password".to_string(), "secret".to_string()),
        ("ENTITY_DS_PASSWORD".to_string(), "secret".to_string()),
        ("MOQUI_CONF".to_string(), "conf/x.xml".to_string()),
        ("TERM".to_string(), "dumb".to_string()),
    ];
    let properties: BTreeSet<String> = BTreeSet::from(["entity_ds_password".to_string()]);
    let profile = Profile {
        env: vec![("EDITOR".to_string(), "vi".to_string())],
        ..Profile::default()
    };
    let env = environment(&EnvFor {
        server,
        properties: &properties,
        session_id: "s1",
        runtime: Path::new("/srv/runtime"),
        username: "john.doe",
        profile: &profile,
    });
    let get = |name: &str| env.iter().find(|(key, _)| key == name).map(|(_, value)| value.as_str());
    assert_eq!(get("PATH"), Some("/usr/bin"));
    assert_eq!(get("entity_ds_password"), None);
    assert_eq!(get("ENTITY_DS_PASSWORD"), None);
    assert_eq!(get("MOQUI_CONF"), None);
    assert_eq!(get("TERM"), Some("xterm-256color"));
    assert_eq!(get("LANG"), Some("C.UTF-8"));
    assert_eq!(get("MOQUI_TERMINAL_SESSION"), Some("s1"));
    assert_eq!(get("MOQUI_USER"), Some("john.doe"));
    assert_eq!(get("EDITOR"), Some("vi"));
}
