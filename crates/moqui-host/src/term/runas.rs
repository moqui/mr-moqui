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

//! A host session as another OS account (plans/Terminal-Fancy2.md §4.6, D1): a person's own account on a shared dev
//! server, or Agent Code's `run-as` account, by one mechanism. The server starts `sudo -n -- <moqui> term-exec --run-as
//! <account> --spec <b64>` under a sudoers line the administrator installs (`moqui term-exec --print-setup`). As root,
//! `term-exec` reads [`CONF_PATH`] (root's, written by no one else): the group whose members may be used, accounts named
//! one by one, and the server's own account. The server is not trusted to name them. It checks the account, gives it
//! the pty, starts itself again as a child that drops to the account (supplementary groups, gid, uid: one thread, so
//! rustix's thread calls drop it whole) and then becomes the program, and stays as the parent that ends it: a SIGHUP or
//! SIGTERM, which sudo relays from the server, goes to the program's group, SIGKILL 5 seconds later; it exits with the
//! program's status. Linux only: rustix's credential calls are Linux's. No new `unsafe`; the accounts are read with
//! `getent`, so NSS (LDAP, SSSD) accounts are found too.

// the checks and the supervisor run on Linux alone; elsewhere they are built for their tests
#![cfg_attr(not(target_os = "linux"), allow(dead_code))]

use std::io;

/// The administrator's file of the accounts the server may start programs as.
pub(crate) const CONF_PATH: &str = "/etc/moqui/run-as.conf";
/// The account a `term-exec` child drops to: `uid:gid:groups,...`, set by the root parent alone.
pub(crate) const DROP_ENV: &str = "MOQUI_TERM_DROP";
/// What a program finds on its `PATH` before its login shell sets its own.
const DEFAULT_PATH: &str = "/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin";

/// [`CONF_PATH`]'s settings.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct RunAsConf {
    /// The group whose members may be used.
    pub group: Option<String>,
    /// Accounts that may be used, by name.
    pub accounts: Vec<String>,
    /// The server's own account, which is never one.
    pub server: Option<String>,
}

impl RunAsConf {
    /// Lines of `key = value` (`group`, `account`, `server`), `#` comments; anything else refuses the whole file.
    pub fn parse(text: &str) -> Result<Self, String> {
        let mut conf = Self::default();
        for (number, line) in text.lines().enumerate() {
            let line = line.split('#').next().unwrap_or_default().trim();
            if line.is_empty() {
                continue;
            }
            let Some((key, value)) = line.split_once('=') else {
                return Err(format!("{CONF_PATH} line {}: not key = value", number + 1));
            };
            let value = value.trim().to_string();
            if value.is_empty()
                || !value
                    .chars()
                    .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '.' | '_' | '-'))
            {
                return Err(format!(
                    "{CONF_PATH} line {}: {value:?} is not an account or group name",
                    number + 1
                ));
            }
            match key.trim() {
                "group" => conf.group = Some(value),
                "account" => conf.accounts.push(value),
                "server" => conf.server = Some(value),
                other => return Err(format!("{CONF_PATH} line {}: no setting {other}", number + 1)),
            }
        }
        Ok(conf)
    }
}

/// Whether a file of `owner` uid and `mode` may say which accounts root starts programs as: root's, and writable by
/// no one else.
pub(crate) fn conf_file_safe(owner: u32, mode: u32) -> Result<(), String> {
    if owner != 0 {
        return Err(format!("{CONF_PATH} is not root's"));
    }
    if mode & 0o022 != 0 {
        return Err(format!("{CONF_PATH} is writable by its group or by anyone"));
    }
    Ok(())
}

/// An OS account, as the system knows it.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct Account {
    pub name: String,
    pub uid: u32,
    pub gid: u32,
    pub home: String,
    pub shell: String,
    /// Its supplementary groups' ids.
    pub groups: Vec<u32>,
}

/// What the system says of accounts and groups: `getent` on a server, a table in a test.
pub(crate) trait Accounts {
    fn account(&self, name: &str) -> Option<Account>;
    /// A group's id and its listed members.
    fn group(&self, name: &str) -> Option<(u32, Vec<String>)>;
}

/// Whether `name` may be started as, by `conf`: it exists, is not root, is not the server's own account (by uid, from
/// sudo's `SUDO_UID`, and by name, from the file), and is in the file's group or named in it.
pub(crate) fn check(
    conf: &RunAsConf,
    name: &str,
    server_uid: Option<u32>,
    accounts: &dyn Accounts,
) -> Result<Account, String> {
    let refused = |why: &str| Err(format!("{name}: {why}"));
    if name.is_empty()
        || !name
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '.' | '_' | '-'))
    {
        return refused("not an account name");
    }
    let Some(account) = accounts.account(name) else {
        return refused("no such account");
    };
    if account.uid == 0 {
        return refused("root is never one");
    }
    if server_uid == Some(account.uid) || conf.server.as_deref() == Some(name) {
        return refused("the server's own account is never one");
    }
    let named = conf.accounts.iter().any(|allowed| allowed == name);
    let member = conf
        .group
        .as_deref()
        .and_then(|group| accounts.group(group))
        .is_some_and(|(gid, members)| {
            account.gid == gid || account.groups.contains(&gid) || members.iter().any(|member| member == name)
        });
    if !named && !member {
        return refused(&format!("not allowed by {CONF_PATH}"));
    }
    Ok(account)
}

/// What the server hands the supervisor in its arguments: sudo clears the environment that carries it otherwise.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct RunSpec {
    pub argv: Vec<String>,
    pub arg0: Option<String>,
    pub env: Vec<(String, String)>,
    /// Empty: the account's home.
    pub directory: String,
    /// The account's own login shell, in place of `argv`.
    pub account_shell: bool,
}

impl RunSpec {
    pub fn encode(&self) -> String {
        use base64::Engine as _;
        let json = serde_json::json!({"argv": self.argv, "arg0": self.arg0, "env": self.env,
            "directory": self.directory, "accountShell": self.account_shell});
        base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(json.to_string())
    }

    pub fn decode(text: &str) -> Result<Self, String> {
        use base64::Engine as _;
        let bytes = base64::engine::general_purpose::URL_SAFE_NO_PAD
            .decode(text)
            .map_err(|err| format!("the spec is not base64: {err}"))?;
        let value: serde_json::Value =
            serde_json::from_slice(&bytes).map_err(|err| format!("the spec is not JSON: {err}"))?;
        let strings = |value: &serde_json::Value| -> Vec<String> {
            value
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(|item| item.as_str().map(str::to_string))
                .collect()
        };
        Ok(Self {
            argv: strings(&value["argv"]),
            arg0: value["arg0"].as_str().map(str::to_string),
            env: value["env"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(|pair| Some((pair.get(0)?.as_str()?.to_string(), pair.get(1)?.as_str()?.to_string())))
                .collect(),
            directory: value["directory"].as_str().unwrap_or_default().to_string(),
            account_shell: value["accountShell"].as_bool().unwrap_or(false),
        })
    }
}

/// The command line that starts `spec` as `account` through `sudo`: the server's (or the holder's) helper line.
pub(crate) fn helper(sudo: &str, account: &str, spec: &RunSpec) -> io::Result<(String, Vec<String>)> {
    let exe = std::env::current_exe()?.to_string_lossy().into_owned();
    let args = [
        "-n",
        "--",
        &exe,
        "term-exec",
        "--run-as",
        account,
        "--spec",
        &spec.encode(),
    ];
    Ok((sudo.to_string(), args.iter().map(|arg| (*arg).to_string()).collect()))
}

/// The environment of a program as `account`: the spec's, with the account's own `HOME`, `USER`, `LOGNAME` and `SHELL`,
/// and a `PATH` its login shell then sets.
pub(crate) fn environment(spec_env: &[(String, String)], account: &Account) -> Vec<(String, String)> {
    let mut env: std::collections::BTreeMap<String, String> = spec_env.iter().cloned().collect();
    for (name, value) in [
        ("HOME", account.home.as_str()),
        ("USER", account.name.as_str()),
        ("LOGNAME", account.name.as_str()),
        ("SHELL", account.shell.as_str()),
        ("PATH", DEFAULT_PATH),
    ] {
        env.insert(name.to_string(), value.to_string());
    }
    env.remove(DROP_ENV);
    env.into_iter().collect()
}

/// The sudoers line and the `run-as.conf` an administrator installs (`moqui term-exec --print-setup`).
pub(crate) fn setup_text(server_user: &str, exe: &str) -> String {
    format!(
        "# /etc/sudoers.d/moqui-term: the server's user may run only term-exec as root, with no password and no pty of\n\
         # sudo's own between it and the program (install with visudo -f /etc/sudoers.d/moqui-term)\n\
         Defaults!{exe} !use_pty, !requiretty\n\
         {server_user} ALL=(root) NOPASSWD: {exe} term-exec *\n\n\
         # {CONF_PATH}: owned by root, mode 0644 or tighter; term-exec refuses it otherwise\n\
         # the members of this group may have their programs started as them\n\
         group = moqui-term\n\
         # accounts named one by one (Agent Code's run-as account, for one)\n\
         # account = agent\n\
         # the server's own account, which is never one\n\
         server = {server_user}\n"
    )
}

/// The accounts as `getent` answers: the system's databases, NSS included.
pub(crate) struct GetEnt;

fn getent(database: &str, key: &str) -> Option<String> {
    let out = std::process::Command::new("getent")
        .args([database, key])
        .env_clear()
        .env("PATH", DEFAULT_PATH)
        .output()
        .ok()?;
    out.status.success().then(|| {
        String::from_utf8_lossy(&out.stdout)
            .lines()
            .next()
            .unwrap_or_default()
            .to_string()
    })
}

impl Accounts for GetEnt {
    fn account(&self, name: &str) -> Option<Account> {
        // name:x:uid:gid:gecos:home:shell
        let line = getent("passwd", name)?;
        let fields: Vec<&str> = line.split(':').collect();
        if fields.len() < 7 || fields[0] != name {
            return None;
        }
        let groups = std::process::Command::new("id")
            .args(["-G", name])
            .env_clear()
            .env("PATH", DEFAULT_PATH)
            .output()
            .ok()
            .filter(|out| out.status.success())
            .map(|out| {
                String::from_utf8_lossy(&out.stdout)
                    .split_whitespace()
                    .filter_map(|gid| gid.parse().ok())
                    .collect()
            })
            .unwrap_or_default();
        Some(Account {
            name: name.to_string(),
            uid: fields[2].parse().ok()?,
            gid: fields[3].parse().ok()?,
            home: fields[5].to_string(),
            shell: if fields[6].is_empty() {
                "/bin/sh".to_string()
            } else {
                fields[6].to_string()
            },
            groups,
        })
    }

    fn group(&self, name: &str) -> Option<(u32, Vec<String>)> {
        // name:x:gid:member,member
        let line = getent("group", name)?;
        let fields: Vec<&str> = line.split(':').collect();
        if fields.len() < 4 {
            return None;
        }
        let members = fields[3]
            .split(',')
            .filter(|member| !member.is_empty())
            .map(str::to_string)
            .collect();
        Some((fields[2].parse().ok()?, members))
    }
}

/// `term-exec --run-as <account> --spec <b64>`, run as root through sudo: check, start the program as the account, and
/// supervise it. Answers the exit status.
#[cfg(target_os = "linux")]
pub(crate) fn supervise(args: &[String]) -> i32 {
    use std::os::fd::AsFd;
    use std::os::unix::fs::MetadataExt;
    let fail = |message: String| {
        eprintln!("term-exec: {message}");
        126
    };
    let value_of = |name: &str| {
        args.iter()
            .position(|arg| arg == name)
            .and_then(|at| args.get(at + 1))
            .cloned()
    };
    let (Some(name), Some(spec)) = (value_of("--run-as"), value_of("--spec")) else {
        return fail("--run-as needs an account and --spec".to_string());
    };
    if !rustix::process::geteuid().is_root() {
        return fail("--run-as runs as root, through sudo (moqui term-exec --print-setup)".to_string());
    }
    let spec = match RunSpec::decode(&spec) {
        Ok(spec) => spec,
        Err(err) => return fail(err),
    };
    let conf = std::fs::metadata(CONF_PATH)
        .map_err(|err| format!("{CONF_PATH}: {err}"))
        .and_then(|meta| conf_file_safe(meta.uid(), meta.mode()))
        .and_then(|()| std::fs::read_to_string(CONF_PATH).map_err(|err| format!("{CONF_PATH}: {err}")))
        .and_then(|text| RunAsConf::parse(&text));
    let conf = match conf {
        Ok(conf) => conf,
        Err(err) => return fail(err),
    };
    let server_uid = std::env::var("SUDO_UID").ok().and_then(|uid| uid.parse().ok());
    let account = match check(&conf, &name, server_uid, &GetEnt) {
        Ok(account) => account,
        Err(err) => return fail(err),
    };
    // programs that open /dev/tty again (ssh, sudo, gpg) need the pty to be the account's
    let stdin = io::stdin();
    let uid = rustix::process::Uid::from_raw(account.uid);
    if let Err(err) = rustix::fs::fchown(stdin.as_fd(), Some(uid), None)
        .and_then(|()| rustix::fs::fchmod(stdin.as_fd(), rustix::fs::Mode::from_raw_mode(0o620)))
    {
        return fail(format!("the terminal was not given to {name}: {err}"));
    }
    let (argv, arg0) = if spec.account_shell {
        let base = std::path::Path::new(&account.shell)
            .file_name()
            .map_or_else(|| "sh".to_string(), |base| base.to_string_lossy().into_owned());
        (vec![account.shell.clone()], Some(format!("-{base}")))
    } else {
        (spec.argv.clone(), spec.arg0.clone())
    };
    let directory = Some(spec.directory.as_str())
        .filter(|dir| !dir.is_empty() && std::path::Path::new(dir).is_dir())
        .unwrap_or(&account.home)
        .to_string();
    let groups: Vec<String> = account.groups.iter().map(u32::to_string).collect();
    let exe = match std::env::current_exe() {
        Ok(exe) => exe,
        Err(err) => return fail(format!("this program: {err}")),
    };
    let exec_spec = serde_json::json!({"argv": argv, "arg0": arg0}).to_string();
    let child = std::process::Command::new(exe)
        .arg("term-exec")
        .env_clear()
        .envs(environment(&spec.env, &account))
        .env(super::exec::EXEC_ENV, exec_spec)
        .env(
            DROP_ENV,
            format!("{}:{}:{}", account.uid, account.gid, groups.join(",")),
        )
        .current_dir(&directory)
        .spawn();
    let child = match child {
        Ok(child) => child,
        Err(err) => return fail(format!("the program did not start: {err}")),
    };
    match tokio::runtime::Builder::new_current_thread().enable_all().build() {
        Ok(runtime) => runtime.block_on(watch(child)),
        Err(err) => fail(format!("the supervisor did not start: {err}")),
    }
}

/// Wait for the program, passing on a hang-up or an end to its group, and killing it 5 seconds after.
#[cfg(target_os = "linux")]
async fn watch(mut child: std::process::Child) -> i32 {
    use tokio::signal::unix::{signal, SignalKind};
    let pid = i32::try_from(child.id()).ok().and_then(rustix::process::Pid::from_raw);
    let (Ok(mut hangup), Ok(mut terminate), Ok(mut interrupt)) = (
        signal(SignalKind::hangup()),
        signal(SignalKind::terminate()),
        signal(SignalKind::interrupt()),
    ) else {
        return child.wait().map_or(1, |status| status.code().unwrap_or(1));
    };
    let pass = |to: rustix::process::Signal| {
        if let Some(pid) = pid {
            let _ = rustix::process::kill_process_group(pid, to).or_else(|_| rustix::process::kill_process(pid, to));
        }
    };
    let waited = tokio::task::spawn_blocking(move || child.wait());
    tokio::pin!(waited);
    let mut kill_at: Option<tokio::time::Instant> = None;
    loop {
        let deadline = kill_at.unwrap_or_else(|| tokio::time::Instant::now() + std::time::Duration::from_secs(86_400));
        tokio::select! {
            done = &mut waited => {
                return match done {
                    Ok(Ok(status)) => {
                        use std::os::unix::process::ExitStatusExt;
                        status.code().or_else(|| status.signal().map(|signal| 128 + signal)).unwrap_or(1)
                    }
                    _ => 1,
                };
            }
            _ = hangup.recv() => { pass(rustix::process::Signal::HUP); kill_at.get_or_insert(tokio::time::Instant::now() + std::time::Duration::from_secs(5)); }
            _ = terminate.recv() => { pass(rustix::process::Signal::TERM); kill_at.get_or_insert(tokio::time::Instant::now() + std::time::Duration::from_secs(5)); }
            _ = interrupt.recv() => { pass(rustix::process::Signal::INT); }
            () = tokio::time::sleep_until(deadline), if kill_at.is_some() => {
                pass(rustix::process::Signal::KILL);
                kill_at = None;
            }
        }
    }
}

#[cfg(not(target_os = "linux"))]
pub(crate) fn supervise(_args: &[String]) -> i32 {
    eprintln!("term-exec: --run-as is for Linux servers only");
    126
}

/// In a `term-exec` child the root parent started ([`DROP_ENV`]): drop to the account, groups first, before the
/// program. Refused when the drop does not hold.
#[cfg(target_os = "linux")]
pub(crate) fn drop_privileges(text: &str) -> Result<(), String> {
    use rustix::process::{Gid, Uid};
    let mut parts = text.splitn(3, ':');
    let number = |part: Option<&str>| part.and_then(|value| value.parse::<u32>().ok());
    let (Some(uid), Some(gid)) = (number(parts.next()), number(parts.next())) else {
        return Err(format!("{DROP_ENV} is not uid:gid:groups"));
    };
    if uid == 0 {
        return Err("never root".to_string());
    }
    let groups: Vec<Gid> = parts
        .next()
        .unwrap_or_default()
        .split(',')
        .filter_map(|gid| gid.parse::<u32>().ok())
        .map(Gid::from_raw)
        .collect();
    // one thread here: the thread's credentials are the process's
    rustix::thread::set_thread_groups(&groups).map_err(|err| format!("the groups: {err}"))?;
    let (gid, uid) = (Gid::from_raw(gid), Uid::from_raw(uid));
    rustix::thread::set_thread_res_gid(gid, gid, gid).map_err(|err| format!("the group: {err}"))?;
    rustix::thread::set_thread_res_uid(uid, uid, uid).map_err(|err| format!("the user: {err}"))?;
    if rustix::process::geteuid() != uid || rustix::process::getegid() != gid {
        return Err("the account did not hold".to_string());
    }
    Ok(())
}

#[cfg(not(target_os = "linux"))]
pub(crate) fn drop_privileges(_text: &str) -> Result<(), String> {
    Err("another account is for Linux servers only".to_string())
}

#[cfg(test)]
#[path = "runas_tests.rs"]
mod tests;
