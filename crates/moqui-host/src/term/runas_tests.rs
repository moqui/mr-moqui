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

use super::{check, conf_file_safe, environment, setup_text, Account, Accounts, RunAsConf, RunSpec};

/// A system of three accounts and a group, for the checks without root.
struct Table;

impl Accounts for Table {
    fn account(&self, name: &str) -> Option<Account> {
        let account = |uid: u32, gid: u32, groups: Vec<u32>| Account {
            name: name.to_string(),
            uid,
            gid,
            home: format!("/home/{name}"),
            shell: "/bin/bash".to_string(),
            groups,
        };
        match name {
            "root" => Some(account(0, 0, vec![0])),
            "moqui" => Some(account(500, 500, vec![500])),
            "alice" => Some(account(1001, 1001, vec![1001, 2000])),
            "bob" => Some(account(1002, 1002, vec![1002])),
            "agent" => Some(account(1003, 1003, vec![1003])),
            _ => None,
        }
    }

    fn group(&self, name: &str) -> Option<(u32, Vec<String>)> {
        (name == "moqui-term").then(|| (2000, vec!["carol".to_string()]))
    }
}

#[test]
fn the_conf_file_is_read_strictly() {
    let conf = RunAsConf::parse("# who\ngroup = moqui-term\naccount = agent  # the agent's\nserver = moqui\n").unwrap();
    assert_eq!(conf.group.as_deref(), Some("moqui-term"));
    assert_eq!(conf.accounts, ["agent"]);
    assert_eq!(conf.server.as_deref(), Some("moqui"));
    assert!(RunAsConf::parse("groups = x").is_err(), "no such setting");
    assert!(RunAsConf::parse("group").is_err(), "not key = value");
    assert!(RunAsConf::parse("account = a b").is_err(), "not a name");
    assert!(RunAsConf::parse("account = $(id)").is_err());
    assert!(conf_file_safe(0, 0o100644).is_ok());
    assert!(conf_file_safe(500, 0o100644).is_err(), "not root's");
    assert!(conf_file_safe(0, 0o100664).is_err(), "group writable");
    assert!(conf_file_safe(0, 0o100646).is_err(), "anyone writable");
}

#[test]
fn an_account_is_one_the_file_allows_and_never_root_or_the_servers() {
    let conf = RunAsConf::parse("group = moqui-term\naccount = agent\nserver = moqui\n").unwrap();
    assert_eq!(
        check(&conf, "alice", Some(500), &Table).unwrap().uid,
        1001,
        "a member of the group"
    );
    assert_eq!(check(&conf, "agent", Some(500), &Table).unwrap().uid, 1003, "named");
    let refused = |name: &str, server_uid: Option<u32>| check(&conf, name, server_uid, &Table).unwrap_err();
    assert!(refused("bob", Some(500)).contains("not allowed"));
    assert!(refused("root", Some(500)).contains("root"));
    assert!(refused("moqui", None).contains("server's own"), "by the file's name");
    assert!(refused("alice", Some(1001)).contains("server's own"), "by sudo's uid");
    assert!(refused("nobody-here", Some(500)).contains("no such"));
    assert!(refused("../etc", Some(500)).contains("not an account name"));
    let no_group = RunAsConf::parse("account = agent").unwrap();
    assert!(
        check(&no_group, "alice", Some(500), &Table).is_err(),
        "no group, not named"
    );
}

#[test]
fn the_spec_rides_in_the_arguments_and_the_environment_is_the_accounts() {
    let spec = RunSpec {
        argv: vec!["claude".to_string(), "--resume".to_string()],
        arg0: None,
        env: vec![
            ("HOME".to_string(), "/srv/moqui".to_string()),
            ("EDITOR".to_string(), "vi".to_string()),
        ],
        directory: "/work/project".to_string(),
        account_shell: false,
    };
    assert_eq!(RunSpec::decode(&spec.encode()).unwrap(), spec);
    assert!(RunSpec::decode("not base64 !").is_err());
    let account = Table.account("alice").unwrap();
    let env = environment(&spec.env, &account);
    let get = |name: &str| env.iter().find(|(key, _)| key == name).map(|(_, value)| value.as_str());
    assert_eq!(get("HOME"), Some("/home/alice"), "the account's home, not the server's");
    assert_eq!(get("USER"), Some("alice"));
    assert_eq!(get("SHELL"), Some("/bin/bash"));
    assert_eq!(get("EDITOR"), Some("vi"));
    let helper = super::helper("sudo", "alice", &spec).unwrap();
    assert_eq!(helper.0, "sudo");
    assert_eq!(&helper.1[..2], ["-n", "--"]);
    assert_eq!(&helper.1[3..6], ["term-exec", "--run-as", "alice"]);
    let setup = setup_text("moqui", "/opt/moqui/moqui");
    assert!(
        setup.contains("moqui ALL=(root) NOPASSWD: /opt/moqui/moqui term-exec *"),
        "{setup}"
    );
    assert!(
        setup.contains("!use_pty") && setup.contains("server = moqui"),
        "{setup}"
    );
}
