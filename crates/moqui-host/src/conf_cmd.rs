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

//! `moqui conf env` (plans/archive/Conf-Paths.md §4): each attribute of the conf this runtime merges, with the environment
//! name that sets it over the conf files (`moqui_conf__ssh_server__port`), its value (a secret masked) and, when an
//! environment name set it, which one and from where. How a deployment finds a setting's name; `./moq -D` completes
//! from `--names`.

use std::collections::HashMap;

use moqui_core::context::conf_env::{self, Override};
use moqui_core::context::conf_schema;
use moqui_lib::error::Error;
use moqui_lib::node::path::AttrPath;
use moqui_lib::node::MNode;

/// The text shown for a secret's value.
const MASKED: &str = "******";

/// `moqui conf <subcommand>`.
///
/// # Errors
///
/// An unknown subcommand or option, or a conf that does not merge.
pub fn dispatch(args: &[String]) -> Result<(), Error> {
    match args {
        [sub, rest @ ..] if sub == "env" => env(rest),
        _ => Err(crate::usage::command_usage("conf")),
    }
}

/// One attribute of the merged conf.
struct Line {
    env: String,
    selector: String,
    value: String,
    set_by: Option<String>,
}

fn env(args: &[String]) -> Result<(), Error> {
    let mut names_only = false;
    let mut filter: Option<String> = None;
    for arg in args {
        match arg.as_str() {
            "--names" => names_only = true,
            other if other.starts_with('-') => return Err(crate::usage::command_usage("conf")),
            other => filter = Some(other.to_ascii_lowercase()),
        }
    }
    let (runtime, framework) = crate::runtime_framework_dirs();
    let env_path = runtime.join(".env");
    if env_path.is_file() {
        moqui_lib::env::load_dotenv_file(&env_path)?;
    }
    let components = moqui_core::context::discover_components(&runtime, &framework)?;
    let dirs: Vec<std::path::PathBuf> = components.iter().map(|info| info.location.clone()).collect();
    let (conf, overrides) = match moqui_core::context::merge_component_confs_with(&framework, &runtime, &dirs, false)? {
        Some(merged) => merged,
        None => moqui_core::context::load_merged_conf_with(&framework, &runtime)?,
    };
    let mut lines = Vec::new();
    collect(&conf, &set_by_of(&overrides), &mut lines);
    let shown = lines.iter().filter(|line| {
        filter.as_deref().is_none_or(|text| {
            line.env.to_ascii_lowercase().contains(text) || line.selector.to_ascii_lowercase().contains(text)
        })
    });
    let mut out = String::new();
    for line in shown {
        if names_only {
            out.push_str(&line.env);
        } else {
            out.push_str(&format!("{}={}  # {}", line.env, line.value, line.selector));
            if let Some(set_by) = &line.set_by {
                out.push_str(&format!(", set by {set_by}"));
            }
        }
        out.push('\n');
    }
    print!("{out}");
    for item in overrides.iter().filter(|item| !item.applied()) {
        if let conf_env::Outcome::Refused(why) = &item.outcome {
            eprintln!("{} from {} is not used: {why}", item.name, item.source.name());
        }
    }
    Ok(())
}

/// The name and source of each override that set an attribute, by the attribute's compare key.
fn set_by_of(overrides: &[Override]) -> HashMap<String, String> {
    overrides
        .iter()
        .filter(|item| item.applied())
        .filter_map(|item| {
            let path = AttrPath::parse(item.selector.as_deref()?).ok()?;
            Some((
                conf_env::compare_key(&path),
                format!("{} from {}", item.name, item.source.name()),
            ))
        })
        .collect()
}

fn collect(node: &MNode, set_by: &HashMap<String, String>, out: &mut Vec<Line>) {
    for child in node.children() {
        child.for_each_attribute(|name, value| {
            let Some(path) = AttrPath::of(&child, name, &conf_schema::keying) else {
                return;
            };
            let Some(env) = path.env_name() else {
                return;
            };
            let secret = conf_env::attribute_secret(&path);
            out.push(Line {
                env,
                selector: path.selector(),
                value: if secret && !value.is_empty() {
                    MASKED.to_string()
                } else {
                    value.to_string()
                },
                set_by: set_by.get(&conf_env::compare_key(&path)).cloned(),
            });
        });
        collect(&child, set_by, out);
    }
}
