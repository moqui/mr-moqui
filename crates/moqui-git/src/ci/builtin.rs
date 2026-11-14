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

//! The built-in pipeline (plans/Dogfood.md D11): a repository whose root has a `component.xml` and no `ci.xml` gets
//! one pipeline, `component`, on push and merge-request, with one job that runs the component's regression suite as
//! `./moq test` runs it locally, so a spec over the regression budget fails it here too.

use moqui_lib::node::MNode;

use super::file::{Event, Job, Limits, Pipeline, Step};

/// The pipeline's name, so its status context is `ci/component/test`.
pub const PIPELINE: &str = "component";
pub const JOB: &str = "test";

/// What the job runs: see the script for what its image must hold.
pub const SCRIPT: &str = include_str!("builtin-component.sh");

/// What a `component.xml` says of its component.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Component {
    pub name: String,
    /// The version of its `depends-on name="framework"`, when it names one.
    pub framework_version: Option<String>,
}

/// Read a `component.xml`; `None` when it is not one (not XML, another root, no name the job could use).
pub fn component(text: &str) -> Option<Component> {
    let root = MNode::parse_text("component.xml", text).ok()?;
    if !root.has_name("component") {
        return None;
    }
    let name = root.attribute("name").filter(|name| super::file::valid_name(name))?;
    let framework_version = root
        .children_named("depends-on")
        .into_iter()
        .find(|depends| depends.attribute_is("name", "framework") || depends.attribute_is("name", "moqui-framework"))
        .and_then(|depends| depends.attribute("version"))
        .map(|version| version.trim().to_string())
        .filter(|version| super::file::valid_name(version));
    Some(Component {
        name,
        framework_version,
    })
}

/// The built-in pipeline of `component`: `image` is the conf's `git-ci` `builtin-image` (`{frameworkVersion}` in it
/// is the version the component names, `latest` when it names none), `pool` its `builtin-pool`.
pub fn pipeline(component: &Component, image: &str, pool: Option<String>, workers: u32, limits: &Limits) -> Pipeline {
    let version = component.framework_version.as_deref().unwrap_or("latest");
    let mut env = vec![
        ("MOQUI_CI_COMPONENT".to_string(), component.name.clone()),
        ("MOQUI_CI_WORKERS".to_string(), workers.max(1).to_string()),
    ];
    if let Some(version) = &component.framework_version {
        env.push(("MOQUI_CI_FRAMEWORK_VERSION".to_string(), version.clone()));
    }
    Pipeline {
        name: PIPELINE.to_string(),
        events: vec![Event::Push, Event::MergeRequest],
        jobs: vec![Job {
            name: JOB.to_string(),
            image: image.replace("{frameworkVersion}", version),
            pool,
            timeout_minutes: limits.default_timeout_minutes,
            fetch_depth: 1,
            env,
            steps: vec![Step {
                name: Some(format!("moq test {}", component.name)),
                run: SCRIPT.to_string(),
                timeout_minutes: None,
                continue_on_error: false,
            }],
            reports: vec!["test-results/*.xml".to_string()],
            ..Job::default()
        }],
        ..Pipeline::default()
    }
}
