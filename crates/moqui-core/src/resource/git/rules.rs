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

//! Branch rules (`moqui.resource.GitBranchRule`): protected refs of a repository, enforced by the git server before a
//! pushed ref moves and by the ref services of `org.moqui.impl.GitServices` (create, delete and merge), so no
//! component can skip them. A rule names its refs by a pattern: one that starts with `refs/` is matched against the
//! full ref name, any other against a branch's name; `*` stands for any run of characters. Every rule that matches a
//! ref applies: a change is refused when any of them refuses it. Writes through `git://` (a wiki save, release notes)
//! by users with the update right pass and are logged.

use moqui_lib::object::{to_plain_string, Value};

use crate::context::Error;
use crate::entity::EntityFacade;

pub(crate) const GIT_BRANCH_RULE: &str = "moqui.resource.GitBranchRule";

/// One `GitBranchRule` row.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct BranchRule {
    pub id: String,
    pub pattern: String,
    pub allow_force_push: bool,
    pub allow_delete: bool,
    pub require_merge_request: bool,
    pub required_approvals: u32,
    pub dismiss_stale_approvals: bool,
    pub required_contexts: Vec<String>,
    /// Merge method enumIds (`GmmMerge`, ...); any when empty.
    pub allowed_methods: Vec<String>,
    /// Only members of this group push directly, when set.
    pub push_group: Option<String>,
}

/// How a ref changes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum RefChangeKind {
    Create,
    Update,
    Force,
    Delete,
}

impl RefChangeKind {
    pub(crate) fn parse(text: &str) -> Self {
        match text {
            "create" => Self::Create,
            "force" => Self::Force,
            "delete" => Self::Delete,
            _ => Self::Update,
        }
    }
}

fn field(row: &crate::entity::EntityValue, name: &str) -> Option<String> {
    row.get(name)
        .filter(|value| !matches!(value, Value::Null))
        .map(to_plain_string)
        .map(|text| text.trim().to_string())
        .filter(|text| !text.is_empty())
}

fn flag(row: &crate::entity::EntityValue, name: &str) -> bool {
    field(row, name).is_some_and(|text| matches!(text.as_str(), "Y" | "y" | "true"))
}

fn list(text: Option<String>) -> Vec<String> {
    text.unwrap_or_default()
        .split([',', ' ', '\n'])
        .map(str::trim)
        .filter(|item| !item.is_empty())
        .map(str::to_string)
        .collect()
}

impl BranchRule {
    pub(crate) fn of(row: &crate::entity::EntityValue) -> Self {
        Self {
            id: field(row, "branchRuleId").unwrap_or_default(),
            pattern: field(row, "refPattern").unwrap_or_default(),
            allow_force_push: flag(row, "allowForcePush"),
            allow_delete: flag(row, "allowDelete"),
            require_merge_request: flag(row, "requireMergeRequest"),
            required_approvals: field(row, "requiredApprovals")
                .and_then(|text| text.split('.').next().and_then(|whole| whole.parse().ok()))
                .unwrap_or(0),
            dismiss_stale_approvals: flag(row, "dismissStaleApprovals"),
            required_contexts: list(field(row, "requiredStatusContexts")),
            allowed_methods: list(field(row, "allowedMergeMethods")),
            push_group: field(row, "pushUserGroupId"),
        }
    }

    /// Whether this rule names `ref_name` (a full ref name).
    pub(crate) fn matches(&self, ref_name: &str) -> bool {
        if self.pattern.starts_with("refs/") {
            return glob(&self.pattern, ref_name);
        }
        ref_name
            .strip_prefix("refs/heads/")
            .is_some_and(|branch| glob(&self.pattern, branch))
    }
}

/// `*` matches any run of characters (`/` too); everything else itself.
pub(crate) fn glob(pattern: &str, text: &str) -> bool {
    let (pattern, text) = (pattern.as_bytes(), text.as_bytes());
    let (mut p, mut t) = (0, 0);
    let mut star: Option<(usize, usize)> = None;
    while t < text.len() {
        if p < pattern.len() && pattern[p] == b'*' {
            star = Some((p, t));
            p += 1;
        } else if p < pattern.len() && pattern[p] == text[t] {
            p += 1;
            t += 1;
        } else if let Some((star_p, star_t)) = star {
            p = star_p + 1;
            t = star_t + 1;
            star = Some((star_p, star_t + 1));
        } else {
            return false;
        }
    }
    pattern[p..].iter().all(|byte| *byte == b'*')
}

/// The rules of a repository that name `ref_name`.
pub(crate) fn rules_for(entity: &EntityFacade, repository_id: &str, ref_name: &str) -> Result<Vec<BranchRule>, Error> {
    let _rows = super::rows_guard();
    Ok(entity
        .find(GIT_BRANCH_RULE)?
        .eq("repositoryId", Value::string(repository_id))
        .list()?
        .iter()
        .map(BranchRule::of)
        .filter(|rule| rule.matches(ref_name))
        .collect())
}

/// Whether the current user may push this change of a protected ref directly (not through a merge request), by
/// `rules`; why not. `in_group` says whether the user is in a group.
pub(crate) fn check_direct(
    rules: &[BranchRule],
    ref_name: &str,
    change: RefChangeKind,
    in_group: &dyn Fn(&str) -> bool,
) -> Result<(), String> {
    let short = super::browse::short_name(ref_name);
    for rule in rules {
        match change {
            RefChangeKind::Delete if !rule.allow_delete => {
                return Err(format!("protected branch: {short} may not be deleted"));
            }
            RefChangeKind::Force if !rule.allow_force_push => {
                return Err(format!("protected branch: {short} may not be force-pushed"));
            }
            RefChangeKind::Delete => {}
            _ => match &rule.push_group {
                Some(group) if !in_group(group) => {
                    return Err(if rule.require_merge_request {
                        format!("protected branch: {short} changes through a merge request")
                    } else {
                        format!("protected branch: only {group} may push to {short}")
                    });
                }
                Some(_) => {}
                None if rule.require_merge_request => {
                    return Err(format!("protected branch: {short} changes through a merge request"));
                }
                None => {}
            },
        }
    }
    Ok(())
}

/// What a merge request must have before it merges into a ref with `rules`: the approvals, the commit statuses by
/// context, and the methods allowed. Why it may not merge yet.
pub(crate) fn check_merge(
    rules: &[BranchRule],
    method: &str,
    approvals: u32,
    successful_contexts: &[String],
) -> Result<(), String> {
    for rule in rules {
        if !rule.allowed_methods.is_empty() && !rule.allowed_methods.iter().any(|allowed| allowed == method) {
            return Err(format!(
                "the branch rules allow only {} here",
                rule.allowed_methods.join(", ")
            ));
        }
        if approvals < rule.required_approvals {
            return Err(format!("{} of {} approvals given", approvals, rule.required_approvals));
        }
        let missing: Vec<&str> = rule
            .required_contexts
            .iter()
            .filter(|context| !successful_contexts.contains(context))
            .map(String::as_str)
            .collect();
        if !missing.is_empty() {
            return Err(format!("checks not passed: {}", missing.join(", ")));
        }
    }
    Ok(())
}

/// Whether the current user is in `group`; a failure to read the memberships reads as no.
pub(crate) fn current_user_in(group: &str) -> bool {
    crate::user::is_in_group(group).unwrap_or_else(|err| {
        tracing::error!(group, "cannot read the user's groups for a branch rule: {err}");
        false
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn patterns_match_branches_or_full_refs() {
        assert!(glob("main", "main"));
        assert!(!glob("main", "mainline"));
        assert!(glob("release/*", "release/1.0"));
        assert!(glob("*-stable", "2-stable"));
        assert!(glob("*", "any/thing"));
        assert!(!glob("release/*", "feature/x"));
        let rule = BranchRule {
            pattern: "main".into(),
            ..Default::default()
        };
        assert!(rule.matches("refs/heads/main"));
        assert!(!rule.matches("refs/tags/main"));
        let tags = BranchRule {
            pattern: "refs/tags/v*".into(),
            ..Default::default()
        };
        assert!(tags.matches("refs/tags/v1.0"));
    }

    #[test]
    fn a_protected_branch_takes_changes_by_its_rule() {
        let rule = BranchRule {
            pattern: "main".into(),
            require_merge_request: true,
            ..Default::default()
        };
        let none = |_: &str| false;
        let rules = vec![rule.clone()];
        assert!(check_direct(&rules, "refs/heads/main", RefChangeKind::Update, &none)
            .unwrap_err()
            .contains("merge request"));
        assert!(check_direct(&rules, "refs/heads/main", RefChangeKind::Force, &none).is_err());
        assert!(check_direct(&rules, "refs/heads/main", RefChangeKind::Delete, &none).is_err());
        let pushers = vec![BranchRule {
            push_group: Some("RELEASERS".into()),
            ..rule
        }];
        let releaser = |group: &str| group == "RELEASERS";
        assert!(check_direct(&pushers, "refs/heads/main", RefChangeKind::Update, &releaser).is_ok());
        assert!(check_direct(&pushers, "refs/heads/main", RefChangeKind::Update, &none).is_err());
        assert!(check_direct(&[], "refs/heads/main", RefChangeKind::Force, &none).is_ok());

        let merge_rules = vec![BranchRule {
            required_approvals: 2,
            required_contexts: vec!["ci/build".into()],
            allowed_methods: vec!["GmmSquash".into()],
            ..Default::default()
        }];
        assert!(check_merge(&merge_rules, "GmmMerge", 2, &["ci/build".into()]).is_err());
        assert!(check_merge(&merge_rules, "GmmSquash", 1, &["ci/build".into()]).is_err());
        assert!(check_merge(&merge_rules, "GmmSquash", 2, &[]).is_err());
        assert!(check_merge(&merge_rules, "GmmSquash", 2, &["ci/build".into()]).is_ok());
    }
}
