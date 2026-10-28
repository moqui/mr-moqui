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

//! `propose_edit` (plans/design-dev-aide.md 13.2 and 13.3, WRITE): the one write path of every agent edit (D11). A
//! change set of files, each by operations of the lossless XML layer, by exact text replacements, or as a whole new
//! text, is checked first (an error comes back to the model with its rule and skill section, and nothing is
//! proposed; an edit of an upstream component while the lock is on is such an error, D17), then becomes an approval of
//! kind `file`: the card reads the change as artifacts with Preview, Accept all, Review and Reject. Accept writes each
//! file through the save pipeline when it is still at its base, or when the change still applies to what it is now (a
//! three-way merge by the change's own operations); Review hands the change set to the Workspace's editor, whose save
//! resolves it with what the user did.

use std::collections::BTreeMap;

use moqui_core::devtools::index::{self, extract};
use moqui_core::devtools::{Problem, Severity};
use moqui_core::llm::approval::{
    self, ApprovalRequest, ChangeRow, HandoffTarget, PreviewMode, Requested, Resumed, TargetKind,
};
use moqui_core::llm::run::context::TurnContext;
use moqui_core::llm::tools::format::ToolError;
use moqui_core::llm::tools::schema::{object, Property};
use moqui_core::llm::tools::{caps, AgentTool, Classification, ToolInput, ToolOutcome, ToolSpec};
use moqui_lib::node::MNode;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value as Json};

use super::check::{problems_of, upstream_problem};
use super::{context, file, NamedFile};
use crate::tier::Tier;
use crate::workspace::{save_text, sha256, Context};

/// The skill that unlocks the tool, as `openui-lang` unlocks `write_ui`.
pub const SKILL: &str = "moqui-dev";

/// The arguments of a change set, as the approval binds them.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ChangeSet {
    /// One line: what the change does.
    #[serde(default)]
    pub summary: String,
    /// `extend` (the default: the site component's extensions) or `in-place`.
    #[serde(default)]
    pub target: String,
    pub files: Vec<FileChange>,
}

/// One file of a change set.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FileChange {
    /// `component://<component>/<path>`, or `root:path`.
    pub file: String,
    /// The hash of the file as it was proposed against; empty for a file the change makes.
    #[serde(default)]
    pub base_hash: String,
    /// Operations of the lossless XML layer (`moqui_lib::xml_edit::Op`).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub ops: Vec<moqui_lib::xml_edit::Op>,
    /// Exact replacements: each `old` must be in the text once.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub edits: Vec<TextEdit>,
    /// The whole text: a new file, or the file replaced.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct TextEdit {
    pub old: String,
    pub new: String,
}

impl FileChange {
    /// The file's text after this change, from `current` (the file now, `None` when it is not there).
    ///
    /// # Errors
    ///
    /// Returns why the change does not apply: an operation that names no element, an `old` text not there once, a
    /// file not there with no whole text.
    pub fn apply(&self, current: Option<&str>) -> Result<String, String> {
        let mut text = match (&self.text, current) {
            (Some(text), _) => text.clone(),
            (None, Some(current)) => current.to_string(),
            (None, None) => return Err(format!("{} is not there: give its whole text", self.file)),
        };
        if !self.ops.is_empty() {
            text = moqui_lib::xml_edit::apply(&text, &self.ops)
                .map_err(|err| format!("{}: {err}", self.file))?
                .text;
        }
        for edit in &self.edits {
            match text.matches(edit.old.as_str()).count() {
                1 => text = text.replacen(edit.old.as_str(), &edit.new, 1),
                0 => {
                    return Err(format!(
                        "{}: the text to replace is not there: {}",
                        self.file,
                        first_line(&edit.old)
                    ))
                }
                many => {
                    return Err(format!(
                        "{}: the text to replace is there {many} times; give more of it: {}",
                        self.file,
                        first_line(&edit.old)
                    ))
                }
            }
        }
        Ok(text)
    }
}

fn first_line(text: &str) -> &str {
    text.trim().lines().next().unwrap_or("")
}

pub struct ProposeEditTool(ToolSpec);

impl Default for ProposeEditTool {
    fn default() -> Self {
        let mut spec = ToolSpec::new(
            "propose_edit",
            "Propose a change set of Moqui component files for the user to accept: each file by XML operations (ops: \
             set-attribute, insert, remove, move, replace, set-text, wrap, unwrap, each element by a path or selector), \
             by exact replacements (edits: old and new text, old there once), or by its whole text (a new file). Extend \
             upstream components from the site component (extend-entity, screen-extend, seed data, ECA rules); every \
             file is checked first, and a problem comes back instead of a proposal. Needs the moqui-dev skill.",
            object(&[
                Property::required("summary", "string", "One line: what the change does"),
                Property::required(
                    "files",
                    "array",
                    "[{file: component://<component>/<path>, ops?: [...], edits?: [{old, new}], text?: whole text}]",
                ),
                Property::optional("target", "string", "extend (the default) or in-place"),
            ]),
            Classification::Write,
        )
        .needs(&[caps::DEV]);
        spec.may_pause = true;
        Self(spec)
    }
}

impl AgentTool for ProposeEditTool {
    fn spec(&self) -> &ToolSpec {
        &self.0
    }

    fn call(&self, ctx: &mut TurnContext<'_>, args: Json) -> ToolOutcome {
        if ctx.run.is_some() && moqui_core::llm::skills::loaded(ctx, SKILL).is_none() {
            return ToolError::new(
                "skill_required",
                format!("propose_edit needs the {SKILL} skill. Call skill with name \"{SKILL}\", then propose again."),
            )
            .outcome();
        }
        match propose(ctx, &args) {
            Ok(outcome) => outcome,
            Err(error) => error.outcome(),
        }
    }

    fn resume(&self, ctx: &mut TurnContext<'_>, _args: Json, _input: &ToolInput) -> ToolOutcome {
        let view = match approval::for_call(ctx) {
            Ok(Some(view)) => view,
            Ok(None) => return ToolOutcome::Failed("the change set's approval is not there".into()),
            Err(error) => return ToolOutcome::Failed(error.to_string()),
        };
        match approval::resumed(&view) {
            Ok(Resumed::Approved) => write(ctx, &view.approval_id, &view.args),
            Ok(Resumed::Done(text) | Resumed::HandedOff(text)) => ToolOutcome::text(text),
            Ok(Resumed::Waiting(pause)) => ToolOutcome::Pause(pause),
            Err(error) => ToolOutcome::Failed(error.to_string()),
        }
    }
}

/// A file of a change set, read and changed.
pub(crate) struct Prepared<'a> {
    pub named: NamedFile<'a>,
    pub current: Option<String>,
    pub new_text: String,
    pub kind: moqui_core::devtools::ArtifactKind,
    pub location: String,
}

/// Read each file of `set` and apply its change, in the order the reload wants (entities, services, ECA rules, screens).
pub(crate) fn prepare<'a>(context: &'a Context, set: &ChangeSet) -> Result<Vec<Prepared<'a>>, ToolError> {
    let mut prepared = Vec::with_capacity(set.files.len());
    for change in &set.files {
        let named = file(context, &change.file)?;
        let current = std::fs::read_to_string(&named.path).ok();
        let new_text = change
            .apply(current.as_deref())
            .map_err(|why| ToolError::new("does_not_apply", why))?;
        let located = index::locate(&context.factory, &named.path);
        let kind = located
            .as_ref()
            .map_or(moqui_core::devtools::ArtifactKind::Other, |found| found.kind);
        let location = located.map_or_else(|| named.shown.clone(), |found| found.location);
        prepared.push(Prepared {
            named,
            current,
            new_text,
            kind,
            location,
        });
    }
    prepared.sort_by_key(|file| order_of(file.kind));
    Ok(prepared)
}

fn order_of(kind: moqui_core::devtools::ArtifactKind) -> u8 {
    use moqui_core::devtools::ArtifactKind as Kind;
    match kind {
        Kind::Entity => 0,
        Kind::Data => 1,
        Kind::Service | Kind::Rest => 2,
        Kind::Seca | Kind::Eeca => 3,
        Kind::Screen | Kind::ScreenExtend => 4,
        _ => 5,
    }
}

fn propose(ctx: &mut TurnContext<'_>, args: &Json) -> Result<ToolOutcome, ToolError> {
    let mut set: ChangeSet = serde_json::from_value(args.clone()).map_err(|err| {
        ToolError::new("bad_arguments", format!("The change set does not read: {err}"))
            .hint("files is a list of {file, ops | edits | text}.")
    })?;
    if set.files.is_empty() {
        return Err(ToolError::new("no_files", "A change set names at least one file."));
    }
    let context = context(Tier::Extend)?;
    let prepared = prepare(&context, &set)?;
    // every file is checked first: an error goes back to the model, and nothing is proposed
    let mut problems: Vec<Problem> = Vec::new();
    for file in &prepared {
        problems.extend(problems_of(&context, &file.named.path, &file.new_text));
        problems.extend(upstream_problem(
            &context,
            file.named.root,
            &file.named.relative,
            &file.named.path,
        ));
    }
    let errors: Vec<&Problem> = problems
        .iter()
        .filter(|problem| problem.severity == Severity::Error)
        .collect();
    if !errors.is_empty() {
        return Ok(super::json(&json!({
            "proposed": false,
            "instruction": "Nothing was proposed: fix these problems and propose again. Each names its rule and the \
                skill section with it.",
            "problems": errors,
        })));
    }
    // each file's base, which the approval binds and Accept checks
    for change in &mut set.files {
        let named = file(&context, &change.file)?;
        change.base_hash = std::fs::read(&named.path)
            .map(|bytes| sha256(&bytes))
            .unwrap_or_default();
        change.file.clone_from(&named.shown);
    }
    let card = change_card(&prepared);
    let rows: Vec<ChangeRow> = card["rows"]
        .as_array()
        .into_iter()
        .flatten()
        .map(|row| ChangeRow {
            field: row["file"].as_str().unwrap_or_default().to_string(),
            label: row["artifact"].as_str().unwrap_or_default().to_string(),
            current: String::new(),
            proposed: row["change"].as_str().unwrap_or_default().to_string(),
        })
        .collect();
    let sites: Vec<&str> = prepared
        .iter()
        .filter_map(|file| {
            file.named
                .root
                .component_of(&file.named.relative)
                .map(|component| component.name.as_str())
        })
        .collect();
    let summary = if set.summary.trim().is_empty() {
        format!("Assist proposes {} changes", rows.len().max(prepared.len()))
    } else {
        set.summary.trim().to_string()
    };
    let request = ApprovalRequest {
        target_kind: TargetKind::File,
        target_name: prepared
            .iter()
            .map(|file| file.named.shown.as_str())
            .collect::<Vec<_>>()
            .join(", "),
        classification: Classification::Write,
        args: serde_json::to_value(&set).unwrap_or(Json::Null),
        summary: summary.clone(),
        changes: rows,
        snapshot: json!({"rows": card["rows"], "files": card["files"], "values": card["values"],
            "preview": card["preview"], "components": sites, "summary": summary}),
        record_refs: Json::Null,
        preview: PreviewMode::None,
        handoff: Some(HandoffTarget {
            path: "/apps/dev/Workspace".into(),
            parameters: BTreeMap::from([("review".to_string(), "{approvalId}".to_string())]),
            form: String::new(),
            screen_location: "component://dev-aide/screen/DevAide/Workspace.xml".into(),
            transition: "review".into(),
            extra_path: Vec::new(),
            fields: Vec::new(),
        }),
    };
    Ok(match approval::request(ctx, &request) {
        Requested::Pause(pause) => ToolOutcome::Pause(pause),
        Requested::Approved(grant) => write(ctx, &grant.approval_id, &request.args),
        Requested::Refused(why) => ToolOutcome::Failed(why),
    })
}

/// Accept: each file still at its base, or the change still applying to it now, is written through the save pipeline;
/// nothing is written when one does not.
fn write(ctx: &mut TurnContext<'_>, approval_id: &str, args: &Json) -> ToolOutcome {
    let set: ChangeSet = match serde_json::from_value(args.clone()) {
        Ok(set) => set,
        Err(err) => return ToolOutcome::Failed(format!("the approved change set does not read: {err}")),
    };
    let context = match context(Tier::Extend) {
        Ok(context) => context,
        Err(error) => return error.outcome(),
    };
    let prepared = match prepare(&context, &set) {
        Ok(prepared) => prepared,
        Err(error) => {
            approval::conflict(ctx, approval_id, &json!([{"reason": error.instruction}]));
            return ToolOutcome::text(format!(
                "The files changed since the proposal and the change no longer applies ({}), so nothing was written. \
                 Read the files again and propose anew.",
                error.instruction
            ));
        }
    };
    if let Err(why) = approval::begin(ctx, approval_id, args) {
        return ToolOutcome::text(why);
    }
    let detail = json!({
        "agentRunId": ctx.run.map(|run| run.run_id.clone()),
        "approvalId": approval_id,
        "toolCallId": ctx.call.tool_call_id,
    });
    let mut written: Vec<Json> = Vec::new();
    let mut failed = None;
    for file in &prepared {
        let base = file
            .current
            .as_deref()
            .map(|text| sha256(text.as_bytes()))
            .unwrap_or_default();
        // the base the save checks is the file as read just now; the change was applied to that text
        let base = std::fs::read(&file.named.path)
            .map(|bytes| sha256(&bytes))
            .unwrap_or(base);
        let resolved = match context.resolve(file.named.root, &file.named.relative) {
            Ok(resolved) => resolved,
            Err(err) => {
                failed = Some(err.to_string());
                break;
            }
        };
        match save_text(
            &context,
            file.named.root,
            &resolved,
            &file.new_text,
            &base,
            Some(detail.clone()),
        ) {
            Ok(out)
                if out
                    .get("conflict")
                    .is_some_and(|value| matches!(value, moqui_lib::object::Value::Bool(true))) =>
            {
                failed = Some(format!("{} changed while it was written", file.named.shown));
                break;
            }
            Ok(out) => written.push(json!({
                "file": file.named.shown,
                "reload": out.get("reload").map(moqui_lib::json::to_json),
                "problems": out.get("problems").map(moqui_lib::json::to_json),
                "status": out.get("status").map(moqui_lib::object::to_plain_string),
            })),
            Err(err) => {
                failed = Some(format!("{}: {err}", file.named.shown));
                break;
            }
        }
    }
    let ok = failed.is_none();
    let result = json!({"written": written, "failed": failed});
    approval::finish(ctx, approval_id, ok, &result);
    let mut text = format!(
        "The user accepted the change set: {} of {} files written.",
        written.len(),
        prepared.len()
    );
    if let Some(why) = failed {
        text.push_str(&format!(" Stopped: {why}."));
    }
    text.push_str(&format!(" {}", serde_json::to_string(&written).unwrap_or_default()));
    ToolOutcome::text(text)
}

// ---------------------------------------------------------------------------------------------------------------------
// The card: the change as artifacts, the files with their counts, the values a business user may change, the screens to
// preview.

/// What the card shows of a change set.
pub(crate) fn change_card(prepared: &[Prepared<'_>]) -> Json {
    let mut rows: Vec<Json> = Vec::new();
    let mut files: Vec<Json> = Vec::new();
    let mut values: Vec<Json> = Vec::new();
    let mut preview: Vec<Json> = Vec::new();
    for (position, file) in prepared.iter().enumerate() {
        let old = file.current.clone().unwrap_or_default();
        let diff = moqui_core::resource::textdiff::diff(&old, &file.new_text, &file.named.shown, &file.named.shown);
        let site = file.named.root.is_site_path(&file.named.relative);
        files.push(json!({
            "file": file.named.shown, "rootId": file.named.root.id, "path": file.named.relative,
            "added": diff.stats.insertions, "removed": diff.stats.deletions, "new": file.current.is_none(),
            "side": if site { "site" } else { "upstream" },
        }));
        let described = describe(file);
        if described.is_empty() {
            rows.push(
                json!({"file": file.named.shown, "artifact": short_file(&file.named.shown),
                "change": format!("+{} −{}", diff.stats.insertions, diff.stats.deletions)}),
            );
        }
        rows.extend(described);
        for title in new_titles(&old, &file.new_text) {
            values.push(json!({"file": position, "label": "Title", "attribute": "title", "value": title}));
        }
        match file.kind {
            moqui_core::devtools::ArtifactKind::Screen => preview.push(json!({"location": file.location})),
            moqui_core::devtools::ArtifactKind::ScreenExtend => {
                preview.push(json!({"location": extract::extended_location(&file.location)}));
            }
            _ => {}
        }
    }
    json!({"rows": rows, "files": files, "values": values, "preview": preview})
}

fn short_file(shown: &str) -> String {
    shown.rsplit('/').next().unwrap_or(shown).to_string()
}

/// The rows of one file: what it adds, removes or extends, as artifacts ("Entity `Example`: + field `dueDate`
/// (date)"); a screen-extend's fields by the form they join.
fn describe(file: &Prepared<'_>) -> Vec<Json> {
    let read = |text: &str| -> Option<extract::FileIndex> {
        let path = file.named.path.display().to_string();
        let component = file
            .location
            .strip_prefix("component://")
            .and_then(|rest| rest.split('/').next())
            .unwrap_or("");
        extract::extract(
            &extract::FileInfo {
                path: &path,
                location: &file.location,
                component,
                kind: file.kind,
            },
            text,
        )
        .ok()
    };
    let shown = file.named.shown.as_str();
    let mut rows = Vec::new();
    if file.kind == moqui_core::devtools::ArtifactKind::ScreenExtend {
        let base = extract::extended_location(&file.location);
        let screen = short_file(&base).trim_end_matches(".xml").to_string();
        let before = form_fields(file.current.as_deref().unwrap_or(""));
        for (form, name, after, title) in form_fields(&file.new_text) {
            if before
                .iter()
                .any(|(old_form, old_name, _, _)| *old_form == form && *old_name == name)
            {
                continue;
            }
            let label = if title.is_empty() { name.clone() } else { title };
            let place = if after.is_empty() {
                String::new()
            } else {
                format!(" after {after}")
            };
            rows.push(json!({"file": shown, "artifact": format!("Screen {screen}"),
                "change": format!("+ field {label} in {form}{place}")}));
        }
        if rows.is_empty() {
            rows.push(json!({"file": shown, "artifact": format!("Screen {screen}"), "change": "extended"}));
        }
        return rows;
    }
    let (Some(after), before) = (read(&file.new_text), file.current.as_deref().and_then(read)) else {
        return rows;
    };
    let before = before.unwrap_or_default();
    let old_ids: std::collections::BTreeSet<&str> =
        before.artifacts.iter().map(|artifact| artifact.id.as_str()).collect();
    let new_ids: std::collections::BTreeSet<&str> =
        after.artifacts.iter().map(|artifact| artifact.id.as_str()).collect();
    let owner = |artifact: &extract::ArtifactRow| -> String {
        match artifact.kind.as_str() {
            "field" => {
                let entity = artifact.parent.as_deref().unwrap_or("").trim_start_matches("entity:");
                format!("Entity {}", entity.rsplit('.').next().unwrap_or(entity))
            }
            "transition" | "form" | "anchor" => {
                format!("Screen {}", short_file(&file.location).trim_end_matches(".xml"))
            }
            _ => short_file(shown),
        }
    };
    for artifact in after
        .artifacts
        .iter()
        .filter(|artifact| !old_ids.contains(artifact.id.as_str()))
    {
        if artifact.kind == "extend-entity" {
            continue;
        }
        let what = if artifact.summary.is_empty() {
            format!("+ {} {}", artifact.kind, artifact.short_name)
        } else {
            format!("+ {} {} ({})", artifact.kind, artifact.short_name, artifact.summary)
        };
        rows.push(json!({"file": shown, "artifact": owner(artifact), "change": what}));
    }
    for artifact in before
        .artifacts
        .iter()
        .filter(|artifact| !new_ids.contains(artifact.id.as_str()))
    {
        rows.push(json!({"file": shown, "artifact": owner(artifact),
            "change": format!("− {} {}", artifact.kind, artifact.short_name)}));
    }
    rows
}

/// The fields of the forms of a screen or screen-extend text: (form, field, after, title).
fn form_fields(text: &str) -> Vec<(String, String, String, String)> {
    let Ok(root) = MNode::parse_text("change", text) else {
        return Vec::new();
    };
    let mut found = Vec::new();
    let mut stack = vec![root];
    while let Some(node) = stack.pop() {
        if node.has_name("form-single") || node.has_name("form-list") {
            let form = node.attribute("name").unwrap_or_default();
            for field in node.named_iter("field") {
                let title = field
                    .attribute("title")
                    .or_else(|| {
                        field
                            .first_named("default-field")
                            .and_then(|default| default.attribute("title"))
                    })
                    .unwrap_or_default();
                found.push((
                    form.clone(),
                    field.attribute("name").unwrap_or_default(),
                    field.attribute("after").unwrap_or_default(),
                    title,
                ));
            }
        }
        stack.extend(node.child_iter());
    }
    found
}

/// The `title="…"` values of the new text that the old one does not have: what a business user may want to word.
fn new_titles(old: &str, new: &str) -> Vec<String> {
    let titles = |text: &str| -> Vec<String> {
        text.match_indices("title=\"")
            .filter_map(|(at, found)| {
                let rest = &text[at + found.len()..];
                rest.find('"').map(|end| rest[..end].to_string())
            })
            .collect()
    };
    let mut before = titles(old);
    let mut added = Vec::new();
    for title in titles(new) {
        if let Some(position) = before.iter().position(|seen| *seen == title) {
            before.remove(position);
        } else if !title.contains("${") {
            added.push(title);
        }
    }
    added
}

moqui_core::submit_tool!(ProposeEditTool);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_change_applies_by_operations_replacements_or_whole_text() {
        let base = "<entities>\n    <extend-entity entity-name=\"Example\" package=\"moqui.example\">\n    </extend-entity>\n</entities>\n";
        let by_edit = FileChange {
            file: "x".into(),
            edits: vec![TextEdit {
                old: "    </extend-entity>".into(),
                new: "        <field name=\"dueDate\" type=\"date\"/>\n    </extend-entity>".into(),
            }],
            ..FileChange::default()
        };
        let applied = by_edit.apply(Some(base)).unwrap();
        assert!(applied.contains("<field name=\"dueDate\" type=\"date\"/>"));
        // not there once: refused with why
        let twice = FileChange {
            file: "x".into(),
            edits: vec![TextEdit {
                old: "    ".into(),
                new: "  ".into(),
            }],
            ..FileChange::default()
        };
        assert!(twice.apply(Some(base)).unwrap_err().contains("times"));
        // a new file needs its whole text
        assert!(by_edit.apply(None).is_err());
        let ops: Vec<moqui_lib::xml_edit::Op> = serde_json::from_value(json!([
            {"op": "insert", "at": "entities/extend-entity", "position": "last", "xml": "<field name=\"dueDate\" type=\"date\"/>"}
        ]))
        .unwrap();
        let by_ops = FileChange {
            file: "x".into(),
            ops,
            ..FileChange::default()
        };
        assert!(by_ops.apply(Some(base)).unwrap().contains("dueDate"));
    }

    #[test]
    fn titles_and_form_fields_for_the_card() {
        let old = "<screen-extend><form-single name=\"F\"></form-single></screen-extend>";
        let new = "<screen-extend><form-single name=\"F\"><field name=\"dueDate\" after=\"exampleEndDate\"><default-field title=\"Due Date\"><display/></default-field></field></form-single></screen-extend>";
        assert_eq!(new_titles(old, new), ["Due Date"]);
        assert_eq!(
            form_fields(new),
            [(
                "F".to_string(),
                "dueDate".to_string(),
                "exampleEndDate".to_string(),
                "Due Date".to_string()
            )]
        );
    }
}
