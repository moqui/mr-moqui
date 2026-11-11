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

//! Host sessions (plans/Terminal-Fancy.md §7): a program on the server's operating system, on a pty ([`super::exec`]),
//! run as the server's OS user (D10). A profile names the program: `shell`, the server user's own shell as a login
//! shell (D9); the conf's `terminal` `host-profile` elements, offered only when their command is on the `PATH`; and
//! the user's own (the `TERMINAL_PROFILES` preference), which grant nothing a user who may open a shell could not type.
//! The program gets the server's environment less every variable that names a property of the Moqui conf, with the
//! terminal's own set (§7.3), and starts in the profile's directory, else `host-directory`, else the checkout above
//! the runtime.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use moqui_core::context::ExecutionContextFactory;
use moqui_lib::node::MNode;
use rustix::process::Signal;

use super::exec;
use super::session::Program;
use super::Ended;

/// The user's own profiles: a preference, a JSON list of objects as the conf's elements.
pub(crate) const PROFILES_PREFERENCE: &str = "TERMINAL_PROFILES";
/// The profile of the server user's own shell.
pub(crate) const SHELL_PROFILE: &str = "shell";

/// A program a host session runs.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct Profile {
    pub name: String,
    pub title: String,
    /// The program; none is the server user's shell.
    pub command: Option<String>,
    pub args: Vec<String>,
    /// The arguments in place of `args` when a session the server ended is started again (an agent's `--resume`).
    pub resume_args: Option<Vec<String>>,
    pub directory: Option<String>,
    /// As a login shell: `arg0` starts with `-`.
    pub login: bool,
    /// Start bash or zsh with the shell integration script (§11.2).
    pub integration: bool,
    /// A coding agent: quiet output reads as done (§11.1).
    pub agent: bool,
    pub env: Vec<(String, String)>,
    /// The user's own, not the conf's.
    pub own: bool,
}

/// Arguments: a JSON list of strings, or words separated by spaces.
fn parse_args(text: &str) -> Vec<String> {
    let text = text.trim();
    if text.starts_with('[') {
        if let Ok(list) = serde_json::from_str::<Vec<String>>(text) {
            return list;
        }
    }
    text.split_whitespace().map(str::to_string).collect()
}

fn truthy(text: Option<String>) -> bool {
    text.is_some_and(|text| text.trim() == "true")
}

/// The conf's profiles, `terminal` `host-profile`, in order.
pub(crate) fn conf_profiles(conf_text: &str) -> Vec<Profile> {
    let Ok(conf) = MNode::parse_text("moqui-conf", conf_text) else {
        return Vec::new();
    };
    let mut profiles = Vec::new();
    for terminal in conf.named_iter("terminal") {
        for node in terminal.named_iter("host-profile") {
            let Some(name) = node.attribute("name").filter(|name| !name.trim().is_empty()) else {
                continue;
            };
            let env = node
                .named_iter("env")
                .filter_map(|env| Some((env.attribute("name")?, env.attribute("value").unwrap_or_default())))
                .collect();
            profiles.push(Profile {
                title: node
                    .attribute("title")
                    .filter(|title| !title.is_empty())
                    .unwrap_or_else(|| name.clone()),
                name,
                command: node.attribute("command").filter(|command| !command.trim().is_empty()),
                args: node.attribute("args").map(|args| parse_args(&args)).unwrap_or_default(),
                resume_args: node.attribute("resume-args").map(|args| parse_args(&args)),
                directory: node
                    .attribute("directory")
                    .filter(|directory| !directory.trim().is_empty()),
                login: truthy(node.attribute("login")),
                integration: truthy(node.attribute("integration")),
                agent: truthy(node.attribute("agent")),
                env,
                own: false,
            });
        }
    }
    profiles
}

/// The user's own profiles, from the `TERMINAL_PROFILES` preference's JSON.
pub(crate) fn own_profiles(json: &str) -> Vec<Profile> {
    let Ok(serde_json::Value::Array(list)) = serde_json::from_str::<serde_json::Value>(json) else {
        return Vec::new();
    };
    let text = |value: &serde_json::Value, key: &str| {
        value
            .get(key)
            .and_then(serde_json::Value::as_str)
            .map(str::trim)
            .filter(|text| !text.is_empty())
            .map(str::to_string)
    };
    let args = |value: &serde_json::Value, key: &str| match value.get(key) {
        Some(serde_json::Value::Array(items)) => Some(
            items
                .iter()
                .filter_map(serde_json::Value::as_str)
                .map(str::to_string)
                .collect(),
        ),
        Some(serde_json::Value::String(text)) => Some(parse_args(text)),
        _ => None,
    };
    list.iter()
        .filter_map(|item| {
            let name = text(item, "name")?;
            Some(Profile {
                title: text(item, "title").unwrap_or_else(|| name.clone()),
                name,
                command: text(item, "command"),
                args: args(item, "args").unwrap_or_default(),
                resume_args: args(item, "resumeArgs"),
                directory: text(item, "directory"),
                login: item.get("login").and_then(serde_json::Value::as_bool).unwrap_or(false),
                integration: item
                    .get("integration")
                    .and_then(serde_json::Value::as_bool)
                    .unwrap_or(false),
                agent: item.get("agent").and_then(serde_json::Value::as_bool).unwrap_or(false),
                env: item
                    .get("env")
                    .and_then(serde_json::Value::as_object)
                    .map(|env| {
                        env.iter()
                            .filter_map(|(name, value)| Some((name.clone(), value.as_str()?.to_string())))
                            .collect()
                    })
                    .unwrap_or_default(),
                own: true,
            })
        })
        .collect()
}

/// The server user's shell: `SHELL`, else `/bin/bash`, else `/bin/sh`.
fn server_shell() -> String {
    std::env::var("SHELL")
        .ok()
        .filter(|shell| !shell.is_empty() && Path::new(shell).is_file())
        .or_else(|| Path::new("/bin/bash").is_file().then(|| "/bin/bash".to_string()))
        .unwrap_or_else(|| "/bin/sh".to_string())
}

/// `command` as a file to run: a path that is a file, or a name found on the `PATH`.
pub(crate) fn resolve(command: &str) -> Option<PathBuf> {
    if command.contains('/') {
        let path = PathBuf::from(command);
        return path.is_file().then_some(path);
    }
    let path = std::env::var_os("PATH")?;
    std::env::split_paths(&path)
        .map(|dir| dir.join(command))
        .find(|candidate| candidate.is_file())
}

/// The profiles that resolve to a program, the shell first: the conf's, then the user's own (a name of the conf's is
/// the conf's).
pub(crate) fn available(conf_text: &str, own_json: Option<&str>) -> Vec<Profile> {
    let mut profiles = vec![Profile {
        name: SHELL_PROFILE.to_string(),
        title: "Shell".to_string(),
        login: true,
        ..Profile::default()
    }];
    let mut seen: BTreeSet<String> = BTreeSet::from([SHELL_PROFILE.to_string()]);
    let own = own_json.map(own_profiles).unwrap_or_default();
    for profile in conf_profiles(conf_text).into_iter().chain(own) {
        if profile.name == SHELL_PROFILE {
            // the conf may title or set up the shell, never name another program for it
            if let Some(shell) = profiles.first_mut() {
                shell.title.clone_from(&profile.title);
                shell.integration = profile.integration;
                shell.env.clone_from(&profile.env);
                shell.directory = profile.directory.clone().or(shell.directory.take());
            }
            continue;
        }
        if !seen.insert(profile.name.clone()) {
            continue;
        }
        if profile.command.as_deref().and_then(resolve).is_some() {
            profiles.push(profile);
        }
    }
    profiles
}

/// The argv and `arg0` of `profile`, its `resume-args` in place of its args when started again.
pub(crate) fn argv(profile: &Profile, resume: bool) -> (Vec<String>, Option<String>) {
    let program = profile.command.clone().unwrap_or_else(server_shell);
    let program = resolve(&program).map_or(program, |path| path.to_string_lossy().into_owned());
    let args = match (&profile.resume_args, resume) {
        (Some(resume_args), true) => resume_args.clone(),
        _ => profile.args.clone(),
    };
    let arg0 = profile.login.then(|| {
        let base = Path::new(&program)
            .file_name()
            .map_or_else(|| program.clone(), |name| name.to_string_lossy().into_owned());
        format!("-{base}")
    });
    let mut argv = vec![program];
    argv.extend(args);
    (argv, arg0)
}

/// Where a host session starts: the given directory, else the profile's, else `host-directory`, else the directory
/// above the runtime (the checkout that holds `moq`), else the runtime.
pub(crate) fn directory(requested: Option<&str>, profile: &Profile, host_directory: &str, runtime: &Path) -> PathBuf {
    let home = std::env::var("HOME").unwrap_or_default();
    let expand = |text: &str| match text.strip_prefix("~/") {
        Some(rest) if !home.is_empty() => PathBuf::from(&home).join(rest),
        _ if text == "~" && !home.is_empty() => PathBuf::from(&home),
        _ => PathBuf::from(text),
    };
    [requested, profile.directory.as_deref(), Some(host_directory)]
        .into_iter()
        .flatten()
        .map(str::trim)
        .filter(|text| !text.is_empty())
        .map(expand)
        .find(|dir| dir.is_dir())
        .or_else(|| runtime.parent().filter(|parent| parent.is_dir()).map(Path::to_path_buf))
        .unwrap_or_else(|| runtime.to_path_buf())
}

/// The names of the Moqui conf's properties, as the environment may carry them.
pub(crate) fn conf_property_names(conf_text: &str) -> BTreeSet<String> {
    let Ok(conf) = MNode::parse_text("moqui-conf", conf_text) else {
        return BTreeSet::new();
    };
    conf.named_iter("default-property")
        .filter_map(|node| node.attribute("name"))
        .filter(|name| !name.is_empty())
        .collect()
}

/// What a host session's program runs with (§7.3): the server's environment less the conf's properties, with the
/// terminal's own set and the profile's.
pub(crate) struct EnvFor<'a> {
    pub server: Vec<(String, String)>,
    pub properties: &'a BTreeSet<String>,
    pub session_id: &'a str,
    pub runtime: &'a Path,
    pub username: &'a str,
    pub profile: &'a Profile,
}

pub(crate) fn environment(of: &EnvFor<'_>) -> Vec<(String, String)> {
    let names: BTreeSet<String> = of
        .properties
        .iter()
        .flat_map(|name| {
            [
                name.clone(),
                name.replace('.', "_"),
                name.to_ascii_uppercase().replace('.', "_"),
            ]
        })
        .collect();
    let mut env: BTreeMap<String, String> = of
        .server
        .iter()
        .filter(|(name, _)| !names.contains(name) && !name.starts_with("MOQUI_") && name != exec::EXEC_ENV)
        .cloned()
        .collect();
    env.insert("TERM".to_string(), "xterm-256color".to_string());
    env.insert("COLORTERM".to_string(), "truecolor".to_string());
    if !["LANG", "LC_ALL", "LC_CTYPE"]
        .iter()
        .any(|name| env.contains_key(*name))
    {
        env.insert("LANG".to_string(), "C.UTF-8".to_string());
    }
    env.insert("TERM_PROGRAM".to_string(), "mr-moqui".to_string());
    env.insert(
        "TERM_PROGRAM_VERSION".to_string(),
        env!("CARGO_PKG_VERSION").to_string(),
    );
    env.insert("MOQUI_TERMINAL_SESSION".to_string(), of.session_id.to_string());
    env.insert("MOQUI_RUNTIME".to_string(), of.runtime.to_string_lossy().into_owned());
    env.insert("MOQUI_USER".to_string(), of.username.to_string());
    for (name, value) in &of.profile.env {
        env.insert(name.clone(), value.clone());
    }
    env.into_iter().collect()
}

/// The shell integration scripts (§11.2), written under the runtime's `tmp` when a session uses them.
const BASH_INTEGRATION: &str = include_str!("integration/moqui.bash");
const ZSH_INTEGRATION: &str = include_str!("integration/zshenv");

/// Which shell `program` is, for its integration: bash or zsh, else none. macOS's own `/bin/bash` (3.2) is left alone,
/// as Ghostty leaves it.
fn integrated_shell(program: &str) -> Option<&'static str> {
    let name = Path::new(program).file_name()?.to_string_lossy();
    match name.as_ref() {
        "bash" if !(cfg!(target_os = "macos") && program == "/bin/bash") => Some("bash"),
        "zsh" => Some("zsh"),
        _ => None,
    }
}

/// Start the shell of `launch` with its integration: bash in POSIX mode with `ENV` naming the script (which reads the
/// user's startup files as a login shell would, when it is one), zsh with `ZDOTDIR` naming the script's directory (the
/// user's own `ZDOTDIR` kept in `MOQUI_ZDOTDIR`). A script that cannot be written leaves the shell as it was.
fn integrate(launch: &mut Launch, runtime: &Path) {
    let Some(shell) = launch.argv.first().and_then(|program| integrated_shell(program)) else {
        return;
    };
    let dir = runtime.join("tmp").join("term-integration");
    let (file, text) = match shell {
        "bash" => (dir.join("moqui.bash"), BASH_INTEGRATION),
        _ => (dir.join("zsh").join(".zshenv"), ZSH_INTEGRATION),
    };
    let written = file
        .parent()
        .map_or(Ok(()), std::fs::create_dir_all)
        .and_then(|()| std::fs::write(&file, text));
    if let Err(err) = written {
        tracing::warn!(
            "the terminal's shell integration was not written to {}: {err}",
            file.display()
        );
        return;
    }
    let mut set = |name: &str, value: String| {
        launch.env.retain(|(key, _)| key != name);
        launch.env.push((name.to_string(), value));
    };
    if shell == "bash" {
        set("ENV", file.to_string_lossy().into_owned());
        if launch.arg0.take().is_some() {
            // a login shell in POSIX mode would read the profile itself, then ENV: the script reads it instead
            set("MOQUI_SHELL_LOGIN", "1".to_string());
        }
        launch.argv.insert(1, "--posix".to_string());
    } else {
        if let Some(own) = std::env::var_os("ZDOTDIR") {
            set("MOQUI_ZDOTDIR", own.to_string_lossy().into_owned());
        }
        set("ZDOTDIR", dir.join("zsh").to_string_lossy().into_owned());
    }
}

/// What a host program is started with.
#[derive(Clone)]
pub(crate) struct Launch {
    pub argv: Vec<String>,
    pub arg0: Option<String>,
    pub directory: PathBuf,
    pub env: Vec<(String, String)>,
    pub cols: u16,
    pub rows: u16,
}

impl Launch {
    /// The launch of `profile` for `username`'s session `session_id` on `factory`'s server.
    pub fn of(
        factory: &ExecutionContextFactory,
        profile: &Profile,
        session_id: &str,
        username: &str,
        requested_directory: Option<&str>,
        resume: bool,
        size: (u16, u16),
    ) -> Self {
        let conf = factory.conf_text();
        let runtime = factory.runtime_dir();
        let (argv, arg0) = argv(profile, resume);
        let directory = directory(
            requested_directory,
            profile,
            &factory.conf_setting("terminal", "host-directory"),
            runtime,
        );
        let properties = conf_property_names(&conf);
        let env = environment(&EnvFor {
            server: std::env::vars().collect(),
            properties: &properties,
            session_id,
            runtime,
            username,
            profile,
        });
        let mut launch = Self {
            argv,
            arg0,
            directory,
            env,
            cols: size.0,
            rows: size.1,
        };
        if profile.integration {
            integrate(&mut launch, runtime);
        }
        launch
    }

    /// The command line for the audit and the log.
    pub fn command_line(&self) -> String {
        self.argv.join(" ")
    }

    /// Start it on a pty, as a session's program.
    pub fn start(&self) -> std::io::Result<Program> {
        let _runtime = crate::edge::handle().enter();
        let helper = exec::helper("term-exec")?;
        let started = exec::start(
            &helper,
            &self.argv,
            self.arg0.as_deref(),
            &self.directory,
            &self.env,
            self.cols,
            self.rows,
        )?;
        let exec::Started {
            reader,
            writer,
            master,
            pid,
            mut child,
        } = started;
        let (done, ended) = tokio::sync::oneshot::channel();
        // the program is waited for, so nothing is left a zombie
        let _ = std::thread::Builder::new()
            .name("term-host-wait".to_string())
            .spawn(move || {
                let status = child.wait();
                let _ = done.send(status);
            });
        Ok(Program {
            output: Box::pin(reader),
            input: Box::pin(writer),
            resize: Box::new(move |cols, rows| exec::resize(&master, cols, rows)),
            redraw: None,
            hang_up: Some(Box::new(move || exec::signal(pid, Signal::HUP))),
            kill: Some(Box::new(move || exec::signal(pid, Signal::KILL))),
            ended: Box::pin(async move {
                match ended.await {
                    Ok(Ok(status)) => Ended {
                        status: exit_status(status),
                        note: None,
                    },
                    _ => Ended {
                        status: 1,
                        note: Some("the program could not be waited for".to_string()),
                    },
                }
            }),
        })
    }
}

/// A process's exit status as a shell gives it: its code, or 128 and the signal that ended it.
fn exit_status(status: std::process::ExitStatus) -> u32 {
    use std::os::unix::process::ExitStatusExt;
    status
        .code()
        .map(|code| u32::try_from(code).unwrap_or(1))
        .or_else(|| status.signal().map(|signal| 128 + u32::try_from(signal).unwrap_or(0)))
        .unwrap_or(1)
}

/// The limits of host sessions, apart from the apps' and command lines' (§5.6).
pub(crate) struct HostSessions(pub Arc<super::Sessions>);

impl HostSessions {
    pub fn of(factory: &ExecutionContextFactory) -> Arc<super::Sessions> {
        let held = factory.extension(|factory| {
            Self(Arc::new(super::Sessions::for_hosts(
                super::terminal_number(factory, "host-max-sessions", 32),
                super::terminal_number(factory, "host-max-sessions-per-user", 16),
            )))
        });
        Arc::clone(&held.0)
    }
}

#[cfg(test)]
#[path = "host_tests.rs"]
mod tests;
