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

//! The `screen-xml` canvas dialect (plans/design-dev-aide.md 13.5, D11), offered on dev instances only: the program is
//! an XML screen file or a `screen-extend` as it would be saved, its first line saying where it stands
//! (`<!-- screen-xml at="component://..." path="/qapps/..." -->`). The server checks it as the editor does (the
//! schema digest and the references); the canvas is the page at `path` rendered with the program standing at its
//! location (F14's overlay, a `canvas:<artifactId>` preview token), under the read fence, so nothing it runs writes
//! and the screen's links show and are not followed. Nothing is written for a preview.

use std::sync::Arc;

use serde_json::{json, Value as Json};

use super::{Dialect, Prepared, Refused, WriteInput};
use moqui_core::context::{ExecutionContext, ExecutionContextFactory};
use moqui_devkit::Severity;

/// The most characters a program has.
const MAX_PROGRAM_CHARS: usize = 120_000;

/// Where a program stands and the page that shows it, from its first line.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Header {
    pub location: String,
    pub path: String,
}

/// The header of a program: `<!-- screen-xml at="component://..." path="/qapps/..." -->` before its root element.
#[must_use]
pub fn header(program: &str) -> Option<Header> {
    header_named(program, "screen-xml")
}

/// The header of a program whose first comment starts with `keyword` (`screen-xml`, `qvue-screen`).
#[must_use]
pub fn header_named(program: &str, keyword: &str) -> Option<Header> {
    let (_, after_open) = program.split_once("<!--")?;
    let (comment, _) = after_open.split_once("-->")?;
    if !comment.trim_start().starts_with(keyword) {
        return None;
    }
    let attribute = |name: &str| -> String {
        let key = format!("{name}=\"");
        comment
            .split_once(&key)
            .and_then(|(_, rest)| rest.split_once('"'))
            .map(|(value, _)| value.to_string())
            .unwrap_or_default()
    };
    let location = attribute("at");
    (location.starts_with("component://")).then(|| Header {
        location,
        path: attribute("path"),
    })
}

#[derive(Default)]
pub struct ScreenXml;

fn refused(error: &str, problems: Vec<Json>) -> Refused {
    Refused {
        error: json!({"error": "screen_xml", "instruction": error, "problems": problems,
            "skill": "screen-xml"})
        .to_string(),
        problems,
    }
}

impl Dialect for ScreenXml {
    fn name(&self) -> &'static str {
        "screen-xml"
    }

    fn label(&self) -> &'static str {
        "Moqui screen (dev)"
    }

    fn skill(&self) -> &'static str {
        "screen-xml"
    }

    fn dev_only(&self) -> bool {
        true
    }

    fn available(&self) -> bool {
        moqui_core::devtools::is_dev_instance()
    }

    fn prepare(
        &self,
        _factory: &ExecutionContextFactory,
        _previous: Option<&str>,
        input: &WriteInput,
        _paths: &dyn super::openui::Paths,
    ) -> Result<Prepared, Refused> {
        let Some(program) = input.program.as_deref() else {
            return Err(refused(
                "A screen-xml canvas is written whole: send program, not patch.",
                Vec::new(),
            ));
        };
        if program.chars().count() > MAX_PROGRAM_CHARS {
            return Err(refused(
                &format!("The program is longer than {MAX_PROGRAM_CHARS} characters."),
                Vec::new(),
            ));
        }
        let Some(header) = header(program) else {
            return Err(refused(
                "Start the program with <!-- screen-xml at=\"component://<component>/<path of the file>\" \
                 path=\"/qapps/<the page that shows it>\" -->, then the screen or screen-extend as it would be saved.",
                Vec::new(),
            ));
        };
        let factory = ExecutionContext::with_current(|ec| Arc::clone(ec.factory()));
        let problems = factory
            .map(|factory| {
                let path = location_path(&factory, &header.location)
                    .unwrap_or_else(|| std::path::PathBuf::from(header.location.trim_start_matches("component://")));
                moqui_devkit::check::check_text(&factory, &path, program)
            })
            .unwrap_or_default();
        let errors: Vec<Json> = problems
            .iter()
            .filter(|problem| problem.severity == Severity::Error)
            .map(|problem| serde_json::to_value(problem).unwrap_or(Json::Null))
            .collect();
        if !errors.is_empty() {
            return Err(refused(
                "The screen has errors; fix them and write it again. Each names its rule and skill section.",
                errors,
            ));
        }
        Ok(Prepared {
            program: program.to_string(),
            statements: vec![header.location],
            mutations: Vec::new(),
            reads: Vec::new(),
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

/// The overlay of a `canvas:<artifactId>` preview token: the program of a `screen-xml` canvas of the current user's own
/// conversation, standing at its location.
#[must_use]
pub fn canvas_overlay(
    factory: &Arc<ExecutionContextFactory>,
    artifact_id: &str,
) -> Option<moqui_core::screen::overlay::Overlay> {
    let facade = factory.entity()?;
    let user_id = moqui_core::user::current_user_id().ok().flatten()?;
    let canvas = crate::run::quietly(|| super::Canvas::of_id(&facade, artifact_id))
        .ok()
        .flatten()?;
    if canvas.dialect != "screen-xml" && canvas.dialect != "qvue-screen" {
        return None;
    }
    let owner = crate::run::quietly(|| {
        facade
            .find("moqui.agent.AgentConversation")?
            .eq(
                "conversationId",
                moqui_lib::object::Value::string(&canvas.conversation_id),
            )
            .disable_authz()
            .one()
    })
    .ok()
    .flatten()
    .and_then(|row| row.get("userId").map(moqui_lib::object::to_plain_string))?;
    if owner != user_id {
        return None;
    }
    let mut overlay = moqui_core::screen::overlay::Overlay::new();
    if canvas.dialect == "qvue-screen" {
        // the XML screen at its location and the SFC beside it as its .qvue
        let (header, xml, sfc) = super::qvue_screen::parts(&canvas.program)?;
        overlay.insert_location(factory, &header.location, &xml);
        overlay.insert_location(factory, &super::qvue_screen::qvue_location(&header.location), &sfc);
        return Some(overlay);
    }
    let header = header(&canvas.program)?;
    overlay.insert_location(factory, &header.location, &canvas.program);
    Some(overlay)
}

crate::submit_dialect!(ScreenXml);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_header_names_the_location_and_the_page() {
        let program = "<!-- screen-xml at=\"component://my-site/screen-extend/example/screen/Gallery/ExampleDetail.xml\" path=\"/qapps/example/Gallery/ExampleDetail?exampleId=100\" -->\n<screen-extend/>";
        assert_eq!(
            header(program),
            Some(Header {
                location: "component://my-site/screen-extend/example/screen/Gallery/ExampleDetail.xml".into(),
                path: "/qapps/example/Gallery/ExampleDetail?exampleId=100".into(),
            })
        );
        assert_eq!(header("<screen/>"), None);
        assert_eq!(header("<!-- screen-xml at=\"/etc/passwd\" --><screen/>"), None);
        // the first comment only; an attribute with no closing quote is empty
        assert_eq!(
            header("x <!--screen-xml at=\"component://a.xml\" path=\"/qapps/é\"--> <!-- at=\"component://b.xml\" -->"),
            Some(Header {
                location: "component://a.xml".into(),
                path: "/qapps/é".into()
            })
        );
        assert_eq!(
            header("<!-- screen-xml at=\"component://a.xml\" path=\"/qapps -->").map(|h| h.path),
            Some(String::new())
        );
        assert_eq!(header("<!-- screen-xml at=\"component://a.xml\""), None);
    }

    #[test]
    fn a_comment_that_closes_inside_its_opening_reads_to_the_next_close() {
        // "<!-->" once took "-->" inside "<!--" and panicked on the slice
        assert_eq!(header("<!-->"), None);
        assert_eq!(header("<!--->"), None);
        assert_eq!(
            header("<!-->screen-xml at=\"component://a.xml\" -->").map(|h| h.location),
            None,
            "the comment is \">screen-xml ...\", which does not start with the keyword"
        );
    }
}
