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

//! The `qvue-screen` canvas dialect (plans/design-dev-aide.md 13.5, P9), offered on dev instances only: a `.qvue` screen
//! as it would be saved, two files in one program. Its first line says where the XML stands and the page that shows it
//! (`<!-- qvue-screen at="component://.../Board.xml" path="/qapps/..." -->`); then the XML screen (its parameters,
//! transitions, actions, `render-modes="qvue"` and the `<text type="qvue">` naming the `.qvue` beside it); then the SFC
//! blocks of a `vue-sfc` program (`<template>`, `<script>`, `<style>`, `<actions>`, `<reads>`), which become the `.qvue`.
//!
//! The XML is checked as the editor checks it, the SFC as a `vue-sfc` program is. The SFC previews in the `vue-sfc`
//! sandbox frame, not in the page: it is code the agent wrote. Its `$bridge` reads go to the read proxy, which answers
//! the screen's own paths (and the reads it declared) with the program standing at its location (F14's overlay, the
//! canvas's), under the read fence; its submits are its `<actions>`, held as approvals as a `vue-sfc` canvas's are. In a
//! saved screen `$bridge` is the framework's mixin over `moqui.request` (`WebrootVue`), so the component's code is the
//! same in the canvas and the screen. Saving is the agent's `propose_edit` of the two files; nothing is written here.

use std::sync::Arc;

use serde_json::{json, Value as Json};

use super::screen_xml::{header_named, Header};
use super::{Dialect, Prepared, Refused, WriteInput};
use moqui_core::context::{ExecutionContext, ExecutionContextFactory};
use moqui_core::devtools::Severity;

/// The skill whose sections the errors name.
pub const SKILL: &str = "qvue-screens";
/// The most characters a program has: the XML and the SFC.
const MAX_PROGRAM_CHARS: usize = 120_000;

/// The `.qvue` beside an XML screen location: the same path with `.qvue` for `.xml`.
#[must_use]
pub fn qvue_location(location: &str) -> String {
    location
        .strip_suffix(".xml")
        .map_or_else(|| format!("{location}.qvue"), |base| format!("{base}.qvue"))
}

/// A program's header, its XML screen (from `<screen` to `</screen>`) and its SFC (the rest after the XML).
#[must_use]
pub fn parts(program: &str) -> Option<(Header, String, String)> {
    let header = header_named(program, "qvue-screen")?;
    let start = program.find("<screen")?;
    let close = program[start..]
        .find("</screen>")
        .map(|at| start + at + "</screen>".len())?;
    let xml = program[start..close].to_string();
    let sfc = program[close..].trim_start().to_string();
    Some((header, xml, sfc))
}

/// The SFC of a program, the `vue-sfc` program the sandbox runs: the client and the action requests take it.
#[must_use]
pub fn sfc_of(program: &str) -> String {
    parts(program).map(|(_, _, sfc)| sfc).unwrap_or_default()
}

/// An `/apps` path as the read proxy normalizes one: a `/qapps/...` page read as its twin, no query.
fn apps_path(path: &str) -> String {
    let path = path.split('?').next().unwrap_or(path);
    path.strip_prefix("/qapps/")
        .map_or_else(|| path.to_string(), |rest| format!("/apps/{rest}"))
}

fn refused(error: &str, problems: Vec<Json>) -> Refused {
    Refused {
        error: json!({"error": "qvue_screen", "instruction": error, "problems": problems, "skill": SKILL}).to_string(),
        problems,
    }
}

#[derive(Default)]
pub struct QvueScreen;

impl Dialect for QvueScreen {
    fn name(&self) -> &'static str {
        "qvue-screen"
    }

    fn label(&self) -> &'static str {
        "Moqui .qvue screen (dev)"
    }

    fn skill(&self) -> &'static str {
        SKILL
    }

    fn dev_only(&self) -> bool {
        true
    }

    fn available(&self) -> bool {
        moqui_core::devtools::is_dev_instance()
    }

    fn prepare(
        &self,
        factory: &ExecutionContextFactory,
        previous: Option<&str>,
        input: &WriteInput,
        paths: &dyn super::openui::Paths,
    ) -> Result<Prepared, Refused> {
        let Some(program) = input.program.as_deref() else {
            return Err(refused(
                "A qvue-screen canvas is written whole: send program, not patch.",
                Vec::new(),
            ));
        };
        if program.chars().count() > MAX_PROGRAM_CHARS {
            return Err(refused(
                &format!("The program is longer than {MAX_PROGRAM_CHARS} characters."),
                Vec::new(),
            ));
        }
        let Some((header, xml, sfc)) = parts(program) else {
            return Err(refused(
                "Start the program with <!-- qvue-screen at=\"component://<component>/<path of the screen's .xml>\" \
                 path=\"/qapps/<the page that shows it>\" -->, then the XML screen, then the SFC blocks of its .qvue \
                 (<template>, <script>, and <style>, <actions> or <reads> when it has them).",
                Vec::new(),
            ));
        };
        // the XML names its .qvue and renders as qvue
        let qvue = qvue_location(&header.location);
        if !xml.contains(&format!("location=\"{qvue}\"")) || !xml.contains("qvue") {
            return Err(refused(
                &format!(
                    "The screen's XML renders its .qvue: render-modes=\"qvue\" (with tui when it has a terminal form) and \
                     <widgets><render-mode><text type=\"qvue\" location=\"{qvue}\" template=\"false\"/></render-mode></widgets>."
                ),
                Vec::new(),
            ));
        }
        let checker = ExecutionContext::with_current(|ec| Arc::clone(ec.factory()));
        let errors: Vec<Json> = checker
            .map(|checker| {
                let path = location_path(&checker, &header.location)
                    .unwrap_or_else(|| std::path::PathBuf::from(header.location.trim_start_matches("component://")));
                moqui_core::devtools::check::check_text(&checker, &path, &xml)
            })
            .unwrap_or_default()
            .iter()
            .filter(|problem| problem.severity == Severity::Error)
            .map(|problem| serde_json::to_value(problem).unwrap_or(Json::Null))
            .collect();
        if !errors.is_empty() {
            return Err(refused(
                "The screen's XML has errors; fix them and write it again. Each names its rule and skill section.",
                errors,
            ));
        }
        // the SFC as a vue-sfc program: its checks, its actions and its reads
        let previous_sfc = previous.map(sfc_of);
        let sfc_input = WriteInput {
            title: input.title.clone(),
            program: Some(sfc),
            patch: None,
        };
        let sfc = super::vue_sfc::VueSfc.prepare(factory, previous_sfc.as_deref(), &sfc_input, paths)?;
        let mut reads = sfc.reads;
        // the screen's own page and transitions answer its reads
        let own = apps_path(&header.path);
        if !own.is_empty() && !reads.contains(&own) {
            reads.push(own);
        }
        let mut statements = vec![header.location, qvue];
        statements.extend(sfc.statements);
        Ok(Prepared {
            program: program.to_string(),
            statements,
            mutations: sfc.mutations,
            reads,
        })
    }
}

/// The file path of a `component://` location, when its component is loaded.
fn location_path(factory: &ExecutionContextFactory, location: &str) -> Option<std::path::PathBuf> {
    let (name, rest) = location.strip_prefix("component://")?.split_once('/')?;
    factory
        .components()
        .iter()
        .find(|component| component.name == name)
        .map(|component| component.location.join(rest))
}

/// Whether `path` (an `/apps` path, normalized) is the screen's own page or under it: what a canvas's read may be.
#[must_use]
pub fn own_read(program: &str, path: &str) -> bool {
    let Some((header, _, _)) = parts(program) else {
        return false;
    };
    let own = apps_path(&header.path);
    let path = apps_path(path);
    !own.is_empty() && (path == own || path.starts_with(&format!("{own}/")))
}

crate::submit_dialect!(QvueScreen);

#[cfg(test)]
mod tests {
    use super::*;

    const PROGRAM: &str = "<!-- qvue-screen at=\"component://my-site/screen/MySite/Board.xml\" path=\"/qapps/board?x=1\" -->\n\
        <screen render-modes=\"qvue\"><transition name=\"items\" read-only=\"true\"/>\n\
        <widgets><render-mode><text type=\"qvue\" location=\"component://my-site/screen/MySite/Board.qvue\" template=\"false\"/></render-mode></widgets>\n\
        </screen>\n<template><div>{{ n }}</div></template>\n<script>module.exports = { data() { return { n: 1 }; } };</script>\n";

    #[test]
    fn a_program_is_its_xml_and_its_sfc() {
        let (header, xml, sfc) = parts(PROGRAM).unwrap();
        assert_eq!(header.location, "component://my-site/screen/MySite/Board.xml");
        assert!(xml.starts_with("<screen") && xml.ends_with("</screen>"));
        assert!(sfc.starts_with("<template>") && sfc.contains("module.exports"));
        assert_eq!(
            qvue_location(&header.location),
            "component://my-site/screen/MySite/Board.qvue"
        );
        assert_eq!(sfc_of(PROGRAM), sfc);
        assert!(parts("<screen/>").is_none());
    }

    #[test]
    fn its_reads_are_its_own_page_and_under_it() {
        assert!(own_read(PROGRAM, "/apps/board"));
        assert!(own_read(PROGRAM, "/qapps/board/items"));
        assert!(!own_read(PROGRAM, "/apps/boarding"));
        assert!(!own_read(PROGRAM, "/apps/other/items"));
    }
}
