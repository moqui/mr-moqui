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

//! The ECA rules as the framework holds them (plans/design-dev-aide.md 10), for the dev-aide's ECA list: each SECA,
//! EECA, EmECA and gitECA rule with its file and line, read from the files in the order and with the rules the loads
//! use (components in order, files by name; a later rule with an id replacing an earlier one with that id; SECA and
//! gitECA by priority, an entity's EECA rules with no id first), and the rules a later one replaced, marked so with
//! the rule that replaced them. A file that does not parse is skipped, as at a load.

use std::path::{Path, PathBuf};

use moqui_lib::node::MNode;
use serde::Serialize;

use crate::context::ExecutionContextFactory;

/// One rule as the list shows it.
#[derive(Clone, Debug, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct EcaRuleInfo {
    /// `seca`, `eeca`, `emeca` or `giteca`.
    pub kind: String,
    pub id: String,
    pub file: String,
    pub line: u32,
    pub component: String,
    /// SECA and gitECA: the priority (5 by default); the others none.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub priority: Option<i64>,
    /// The service (SECA), the entity (EECA), the event (gitECA).
    pub target: String,
    /// When it runs: a SECA's `when`, an EECA's events (create, update, delete, before), a gitECA's repository and ref.
    pub when: String,
    /// Its condition's text, one line, cut at 160 characters.
    pub condition: String,
    /// Its actions' element names, in order.
    pub actions: Vec<String>,
    /// For a rule a later one of its id replaced: that rule's `file:line`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub replaced_by: Option<String>,
}

/// Every ECA rule of the factory's components, the active ones in the order they run and each replaced one after the
/// rule that replaced it.
#[must_use]
pub fn rule_list(factory: &ExecutionContextFactory) -> Vec<EcaRuleInfo> {
    let mut by_kind: Vec<(String, Vec<EcaRuleInfo>)> = ["seca", "eeca", "emeca", "giteca"]
        .iter()
        .map(|kind| ((*kind).to_string(), Vec::new()))
        .collect();
    for component in factory.components() {
        for (dir, suffixes) in [
            (
                component.location.join("service"),
                &[".secas.xml", ".emecas.xml", ".gitecas.xml"][..],
            ),
            (component.location.join("entity"), &[".eecas.xml"][..]),
        ] {
            for path in files(&dir, suffixes) {
                let Ok(root) = MNode::parse_file(&path) else {
                    continue;
                };
                for (kind, element) in [
                    ("seca", "seca"),
                    ("eeca", "eeca"),
                    ("emeca", "emeca"),
                    ("giteca", "giteca"),
                ] {
                    for node in root.named_iter(element) {
                        let info = info_of(kind, &node, &path, &component.name);
                        if let Some((_, rules)) = by_kind.iter_mut().find(|(have, _)| have == kind) {
                            rules.push(info);
                        }
                    }
                }
            }
        }
    }
    let mut out = Vec::new();
    for (kind, loaded) in by_kind {
        out.extend(in_force(&kind, loaded));
    }
    out
}

fn files(dir: &Path, suffixes: &[&str]) -> Vec<PathBuf> {
    let Ok(read) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut paths: Vec<PathBuf> = read
        .filter_map(|entry| entry.ok().map(|entry| entry.path()))
        .filter(|path| {
            path.file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| suffixes.iter().any(|suffix| name.ends_with(suffix)))
        })
        .collect();
    paths.sort();
    paths
}

fn info_of(kind: &str, node: &MNode, path: &Path, component: &str) -> EcaRuleInfo {
    let attr = |name: &str| node.attribute(name).unwrap_or_default();
    let flag = |name: &str| node.attribute_is(name, "true");
    let (target, when) = match kind {
        "seca" => (attr("service"), attr("when")),
        "eeca" => {
            let events: Vec<&str> = [
                ("on-create", "create"),
                ("on-update", "update"),
                ("on-delete", "delete"),
            ]
            .iter()
            .filter(|(name, _)| flag(name))
            .map(|(_, word)| *word)
            .collect();
            (
                attr("entity"),
                format!(
                    "{}{}",
                    events.join(", "),
                    if flag("run-before") { " (before)" } else { "" }
                ),
            )
        }
        "giteca" => (
            attr("event"),
            [attr("repository"), attr("ref-pattern")].join(" ").trim().to_string(),
        ),
        _ => (String::new(), String::new()),
    };
    let condition = node
        .first_named("condition")
        .and_then(|condition| condition.children().into_iter().next())
        // an expression as its text, any other condition element as XML
        .map(|first| match first.name().as_str() {
            "expression" => first.text().unwrap_or_default(),
            _ => first.write_string(),
        })
        .or_else(|| node.attribute("condition"))
        .unwrap_or_default()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ");
    let condition = if condition.chars().count() > 160 {
        condition.chars().take(159).collect::<String>() + "…"
    } else {
        condition
    };
    EcaRuleInfo {
        kind: kind.to_string(),
        id: attr("id"),
        file: path.display().to_string(),
        line: node.line(),
        component: component.to_string(),
        priority: matches!(kind, "seca" | "giteca").then(|| {
            node.attribute("priority")
                .and_then(|text| text.trim().parse().ok())
                .unwrap_or(5)
        }),
        target,
        when,
        condition,
        actions: node
            .first_named("actions")
            .map(|actions| actions.children().iter().map(MNode::name).collect())
            .unwrap_or_default(),
        replaced_by: None,
    }
}

/// The rules of one kind in force, in the order they run, each replaced rule after the one that replaced it.
fn in_force(kind: &str, loaded: Vec<EcaRuleInfo>) -> Vec<EcaRuleInfo> {
    let mut active: Vec<EcaRuleInfo> = Vec::new();
    let mut replaced: Vec<EcaRuleInfo> = Vec::new();
    for rule in loaded {
        let same = (!rule.id.is_empty() && kind != "emeca")
            .then(|| active.iter().position(|have| have.id == rule.id))
            .flatten();
        match same {
            Some(at) => {
                let mut old = std::mem::replace(&mut active[at], rule);
                old.replaced_by = Some(format!("{}:{}", active[at].file, active[at].line));
                replaced.push(old);
            }
            None => active.push(rule),
        }
    }
    match kind {
        // the SECA load: the rules with no id, then those with an id, each in load order; then by priority
        "seca" => {
            let (with_id, no_id): (Vec<_>, Vec<_>) = active.into_iter().partition(|rule| !rule.id.is_empty());
            active = no_id.into_iter().chain(with_id).collect();
            active.sort_by_key(|rule| rule.priority.unwrap_or(5));
        }
        "giteca" => active.sort_by_key(|rule| rule.priority.unwrap_or(5)),
        // by entity, each entity's rules with no id first
        "eeca" => active.sort_by(|a, b| a.target.cmp(&b.target).then((!a.id.is_empty()).cmp(&!b.id.is_empty()))),
        _ => {}
    }
    let mut out = Vec::new();
    for rule in active {
        let at = format!("{}:{}", rule.file, rule.line);
        out.push(rule);
        out.extend(
            replaced
                .iter()
                .filter(|old| old.replaced_by.as_deref() == Some(at.as_str()))
                .cloned(),
        );
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rule(kind: &str, id: &str, line: u32, priority: i64, file: &str) -> EcaRuleInfo {
        EcaRuleInfo {
            kind: kind.into(),
            id: id.into(),
            file: file.into(),
            line,
            component: "c".into(),
            priority: Some(priority),
            target: "s".into(),
            when: "post-service".into(),
            condition: String::new(),
            actions: Vec::new(),
            replaced_by: None,
        }
    }

    #[test]
    fn a_later_rule_of_an_id_replaces_it_and_the_order_is_the_loads() {
        let loaded = vec![
            rule("seca", "A", 1, 5, "a.xml"),
            rule("seca", "", 2, 5, "a.xml"),
            rule("seca", "B", 3, 1, "a.xml"),
            rule("seca", "A", 9, 5, "z.xml"),
        ];
        let shown = in_force("seca", loaded);
        let order: Vec<(String, u32, Option<String>)> = shown
            .iter()
            .map(|rule| (rule.id.clone(), rule.line, rule.replaced_by.clone()))
            .collect();
        assert_eq!(
            order,
            [
                ("B".to_string(), 3, None),
                (String::new(), 2, None),
                ("A".to_string(), 9, None),
                ("A".to_string(), 1, Some("z.xml:9".to_string())),
            ]
        );
    }

    #[test]
    fn a_rule_reads_its_target_when_condition_and_actions() {
        let node = MNode::parse_text(
            "x.secas.xml",
            "<secas>\n<seca id=\"r\" service=\"create#moqui.example.Example\" when=\"post-service\" priority=\"3\">\n<condition><expression>  a &amp;&amp;\n b</expression></condition>\n<actions><set field=\"x\"/><service-call name=\"y\"/></actions></seca></secas>",
        )
        .unwrap();
        let seca = node.first_named("seca").unwrap();
        let info = info_of("seca", &seca, Path::new("x.secas.xml"), "c");
        assert_eq!(info.line, 2);
        assert_eq!(info.priority, Some(3));
        assert_eq!(info.target, "create#moqui.example.Example");
        assert_eq!(info.condition, "a && b");
        assert_eq!(info.actions, ["set", "service-call"]);
    }
}
