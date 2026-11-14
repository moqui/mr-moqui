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

//! The pipeline file: `ci.xml` at a repository's root, read at the commit a run is for (plans/Dogfood.md §8.1,
//! `framework/xsd/git-ci-3.xsd`, docs/interfaces/Pipelines.md). The reader is strict: an element or attribute the
//! schema does not have is an error with its line, so a run of a file that is not valid is an `error` status that
//! says where.

use moqui_lib::node::MNode;

/// What a file may hold, from the conf's `git-ci` element ([`super::limits`]).
#[derive(Clone, Copy, Debug)]
pub struct Limits {
    pub max_bytes: usize,
    pub max_pipelines: usize,
    /// Jobs of one pipeline, after its matrices are expanded.
    pub max_jobs: usize,
    /// Combinations of one job's matrix.
    pub max_matrix: usize,
    pub max_steps: usize,
    pub max_timeout_minutes: u32,
    pub default_timeout_minutes: u32,
    pub max_fetch_depth: u32,
}

impl Default for Limits {
    fn default() -> Self {
        Self {
            max_bytes: 256 * 1024,
            max_pipelines: 20,
            max_jobs: 50,
            max_matrix: 20,
            max_steps: 50,
            max_timeout_minutes: 360,
            default_timeout_minutes: 60,
            max_fetch_depth: 50,
        }
    }
}

/// Why a file is not valid, with the line of the element.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FileError {
    pub line: u32,
    pub message: String,
}

impl std::fmt::Display for FileError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        if self.line > 0 {
            write!(f, "ci.xml line {}: {}", self.line, self.message)
        } else {
            write!(f, "ci.xml: {}", self.message)
        }
    }
}

fn error<T>(node: &MNode, message: impl Into<String>) -> Result<T, FileError> {
    Err(FileError {
        line: node.line(),
        message: message.into(),
    })
}

/// The events a pipeline runs on.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Event {
    Push,
    MergeRequest,
    Tag,
    Schedule,
    Manual,
    Deploy,
}

impl Event {
    pub fn parse(text: &str) -> Option<Self> {
        Some(match text {
            "push" => Self::Push,
            "merge-request" => Self::MergeRequest,
            "tag" => Self::Tag,
            "schedule" => Self::Schedule,
            "manual" => Self::Manual,
            "deploy" => Self::Deploy,
            _ => return None,
        })
    }

    pub fn name(self) -> &'static str {
        match self {
            Self::Push => "push",
            Self::MergeRequest => "merge-request",
            Self::Tag => "tag",
            Self::Schedule => "schedule",
            Self::Manual => "manual",
            Self::Deploy => "deploy",
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct CiFile {
    pub pipelines: Vec<Pipeline>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Pipeline {
    pub name: String,
    pub events: Vec<Event>,
    pub branches: Vec<String>,
    pub tags: Vec<String>,
    pub cron: Option<String>,
    pub env: Vec<(String, String)>,
    pub jobs: Vec<Job>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Job {
    pub name: String,
    pub image: String,
    pub pool: Option<String>,
    pub timeout_minutes: u32,
    pub needs: Vec<String>,
    pub fetch_depth: u32,
    pub environment: Option<String>,
    pub env: Vec<(String, String)>,
    pub secrets: Vec<SecretAsk>,
    pub services: Vec<Service>,
    pub matrix: Vec<Axis>,
    pub caches: Vec<Cache>,
    pub steps: Vec<Step>,
    pub artifacts: Vec<Artifact>,
    /// The JUnit patterns of its reports.
    pub reports: Vec<String>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SecretAsk {
    pub name: String,
    /// The variable it is given as.
    pub env: String,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Service {
    pub name: String,
    pub image: String,
    pub env: Vec<(String, String)>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Axis {
    pub name: String,
    pub values: Vec<String>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Cache {
    pub key: String,
    pub path: String,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Step {
    pub name: Option<String>,
    pub run: String,
    pub timeout_minutes: Option<u32>,
    pub continue_on_error: bool,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Artifact {
    pub name: String,
    pub path: String,
    pub retention_days: Option<u32>,
}

/// A name of a pipeline, a job, an axis, a service or an artifact: what a status context, a host name and a file
/// name can all carry.
pub(crate) fn valid_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 64
        && name
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '.' | '-' | '_'))
        && !name.starts_with(['.', '-'])
}

/// An environment variable's name.
fn valid_variable(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 128
        && name.chars().all(|ch| ch.is_ascii_alphanumeric() || ch == '_')
        && !name.starts_with(|ch: char| ch.is_ascii_digit())
}

/// A path inside the checkout: relative, with no `..` segment.
pub(crate) fn valid_path(path: &str) -> bool {
    !path.is_empty()
        && !path.starts_with('/')
        && !path.contains('\\')
        && !path.contains('\0')
        && path.split('/').all(|segment| segment != ".." && segment != ".git")
}

fn check_attributes(node: &MNode, allowed: &[&str]) -> Result<(), FileError> {
    for (name, _) in node.attributes() {
        // the schema reference of the root element
        if name.starts_with("xmlns") || name.starts_with("xsi:") {
            continue;
        }
        if !allowed.contains(&name.as_str()) {
            return error(
                node,
                format!(
                    "<{}> has no attribute {name} (it has {})",
                    node.name(),
                    allowed.join(", ")
                ),
            );
        }
    }
    Ok(())
}

fn required(node: &MNode, name: &str) -> Result<String, FileError> {
    match node.attribute(name).map(|text| text.trim().to_string()) {
        Some(text) if !text.is_empty() => Ok(text),
        _ => error(node, format!("<{}> needs {name}", node.name())),
    }
}

fn optional(node: &MNode, name: &str) -> Option<String> {
    node.attribute(name)
        .map(|text| text.trim().to_string())
        .filter(|text| !text.is_empty())
}

fn named(node: &MNode) -> Result<String, FileError> {
    let name = required(node, "name")?;
    if !valid_name(&name) {
        return error(
            node,
            format!(
                "the name '{name}' of <{}> must be letters, digits, dot, dash and underscore, at most 64",
                node.name()
            ),
        );
    }
    Ok(name)
}

fn words(text: Option<String>) -> Vec<String> {
    text.unwrap_or_default()
        .split_whitespace()
        .map(str::to_string)
        .collect()
}

fn positive(node: &MNode, name: &str) -> Result<Option<u32>, FileError> {
    let Some(text) = optional(node, name) else {
        return Ok(None);
    };
    match text.parse::<u32>() {
        Ok(number) if number > 0 => Ok(Some(number)),
        _ => error(node, format!("{name} '{text}' is not a positive number")),
    }
}

fn env_of(node: &MNode) -> Result<(String, String), FileError> {
    check_attributes(node, &["name", "value"])?;
    let name = required(node, "name")?;
    if !valid_variable(&name) {
        return error(node, format!("'{name}' is not an environment variable name"));
    }
    let Some(value) = node.attribute("value") else {
        return error(node, format!("<env name=\"{name}\"> needs value"));
    };
    Ok((name, value))
}

fn no_children(node: &MNode) -> Result<(), FileError> {
    match node.first_child() {
        Some(child) => error(&child, format!("<{}> has no <{}>", node.name(), child.name())),
        None => Ok(()),
    }
}

fn job_of(node: &MNode, limits: &Limits) -> Result<Job, FileError> {
    check_attributes(
        node,
        &[
            "name",
            "image",
            "pool",
            "timeout-minutes",
            "needs",
            "fetch-depth",
            "environment",
        ],
    )?;
    let mut job = Job {
        name: named(node)?,
        image: required(node, "image")?,
        pool: optional(node, "pool"),
        needs: words(optional(node, "needs")),
        environment: optional(node, "environment"),
        ..Job::default()
    };
    if job.image.chars().any(char::is_whitespace) {
        return error(node, format!("the image '{}' has a space in it", job.image));
    }
    job.timeout_minutes = positive(node, "timeout-minutes")?.unwrap_or(limits.default_timeout_minutes);
    if job.timeout_minutes > limits.max_timeout_minutes {
        return error(
            node,
            format!(
                "timeout-minutes {} of job {} is over this server's limit of {}",
                job.timeout_minutes, job.name, limits.max_timeout_minutes
            ),
        );
    }
    job.fetch_depth = positive(node, "fetch-depth")?.unwrap_or(1);
    if job.fetch_depth > limits.max_fetch_depth {
        return error(
            node,
            format!(
                "fetch-depth {} of job {} is over this server's limit of {}",
                job.fetch_depth, job.name, limits.max_fetch_depth
            ),
        );
    }
    for child in node.children() {
        match child.name().as_str() {
            "env" => {
                no_children(&child)?;
                job.env.push(env_of(&child)?);
            }
            "secret" => {
                check_attributes(&child, &["name", "env"])?;
                no_children(&child)?;
                let name = required(&child, "name")?;
                let env = optional(&child, "env").unwrap_or_else(|| name.clone());
                if !valid_variable(&name) || !valid_variable(&env) {
                    return error(
                        &child,
                        format!("the secret '{name}' and its variable must be environment variable names"),
                    );
                }
                job.secrets.push(SecretAsk { name, env });
            }
            "service" => {
                check_attributes(&child, &["name", "image"])?;
                let mut service = Service {
                    name: named(&child)?,
                    image: required(&child, "image")?,
                    env: Vec::new(),
                };
                for env in child.children() {
                    if !env.has_name("env") {
                        return error(&env, format!("<service> has no <{}>", env.name()));
                    }
                    service.env.push(env_of(&env)?);
                }
                if job.services.iter().any(|other| other.name == service.name) {
                    return error(&child, format!("two services are named {}", service.name));
                }
                job.services.push(service);
            }
            "matrix" => {
                check_attributes(&child, &[])?;
                if !job.matrix.is_empty() {
                    return error(&child, format!("job {} has two <matrix> elements", job.name));
                }
                for axis in child.children() {
                    if !axis.has_name("axis") {
                        return error(&axis, format!("<matrix> has no <{}>", axis.name()));
                    }
                    check_attributes(&axis, &["name", "values"])?;
                    let name = named(&axis)?;
                    let values = words(Some(required(&axis, "values")?));
                    if let Some(bad) = values.iter().find(|value| !valid_name(value)) {
                        return error(
                            &axis,
                            format!(
                                "the value '{bad}' of axis {name} must be letters, digits, dot, dash and underscore"
                            ),
                        );
                    }
                    if job.matrix.iter().any(|other| other.name == name) {
                        return error(&axis, format!("two axes are named {name}"));
                    }
                    job.matrix.push(Axis { name, values });
                }
                if job.matrix.is_empty() {
                    return error(&child, "<matrix> needs an <axis>");
                }
                let combinations = job
                    .matrix
                    .iter()
                    .fold(1usize, |total, axis| total.saturating_mul(axis.values.len()));
                if combinations > limits.max_matrix {
                    return error(
                        &child,
                        format!(
                            "the matrix of job {} has {combinations} combinations, over this server's limit of {}",
                            job.name, limits.max_matrix
                        ),
                    );
                }
            }
            "cache" => {
                check_attributes(&child, &["key", "path"])?;
                no_children(&child)?;
                let cache = Cache {
                    key: required(&child, "key")?,
                    path: required(&child, "path")?,
                };
                if !valid_path(&cache.path) {
                    return error(
                        &child,
                        format!("the cache path '{}' must be inside the checkout", cache.path),
                    );
                }
                job.caches.push(cache);
            }
            "step" => {
                check_attributes(&child, &["run", "name", "timeout-minutes", "continue-on-error"])?;
                no_children(&child)?;
                let continue_on_error = match optional(&child, "continue-on-error").as_deref() {
                    None | Some("false") => false,
                    Some("true") => true,
                    Some(other) => return error(&child, format!("continue-on-error '{other}' is not true or false")),
                };
                job.steps.push(Step {
                    name: optional(&child, "name"),
                    run: required(&child, "run")?,
                    timeout_minutes: positive(&child, "timeout-minutes")?,
                    continue_on_error,
                });
            }
            "artifact" => {
                check_attributes(&child, &["name", "path", "retention-days"])?;
                no_children(&child)?;
                // a matrix value may be part of the name: each is a name itself
                let name = required(&child, "name")?;
                let mut plain = String::new();
                let mut rest = name.as_str();
                while let Some((before, after)) = rest.split_once("${matrix.") {
                    plain.push_str(before);
                    plain.push('m');
                    rest = after.split_once('}').map_or("", |(_, tail)| tail);
                }
                plain.push_str(rest);
                if !valid_name(&plain) {
                    return error(
                        &child,
                        format!(
                            "the name '{name}' of <artifact> must be letters, digits, dot, dash and underscore, at most 64"
                        ),
                    );
                }
                let artifact = Artifact {
                    name,
                    path: required(&child, "path")?,
                    retention_days: positive(&child, "retention-days")?,
                };
                if !valid_path(&artifact.path) {
                    return error(
                        &child,
                        format!("the artifact path '{}' must be inside the checkout", artifact.path),
                    );
                }
                if job.artifacts.iter().any(|other| other.name == artifact.name) {
                    return error(&child, format!("two artifacts are named {}", artifact.name));
                }
                job.artifacts.push(artifact);
            }
            "report" => {
                check_attributes(&child, &["junit"])?;
                no_children(&child)?;
                let pattern = required(&child, "junit")?;
                if !valid_path(&pattern) {
                    return error(
                        &child,
                        format!("the report path '{pattern}' must be inside the checkout"),
                    );
                }
                job.reports.push(pattern);
            }
            other => return error(&child, format!("<job> has no <{other}>")),
        }
    }
    if job.steps.is_empty() {
        return error(node, format!("job {} has no <step>", job.name));
    }
    if job.steps.len() > limits.max_steps {
        return error(
            node,
            format!(
                "job {} has {} steps, over this server's limit of {}",
                job.name,
                job.steps.len(),
                limits.max_steps
            ),
        );
    }
    Ok(job)
}

fn pipeline_of(node: &MNode, limits: &Limits) -> Result<Pipeline, FileError> {
    check_attributes(node, &["name", "on", "branches", "tags", "cron"])?;
    let mut pipeline = Pipeline {
        name: named(node)?,
        branches: words(optional(node, "branches")),
        tags: words(optional(node, "tags")),
        cron: optional(node, "cron"),
        ..Pipeline::default()
    };
    for word in required(node, "on")?.split_whitespace() {
        let Some(event) = Event::parse(word) else {
            return error(
                node,
                format!("'{word}' is not an event (push, merge-request, tag, schedule, manual, deploy)"),
            );
        };
        if !pipeline.events.contains(&event) {
            pipeline.events.push(event);
        }
    }
    match (&pipeline.cron, pipeline.events.contains(&Event::Schedule)) {
        (None, true) => {
            return error(
                node,
                format!("pipeline {} runs on schedule and has no cron", pipeline.name),
            )
        }
        (Some(_), false) => {
            return error(
                node,
                format!("pipeline {} has a cron and does not run on schedule", pipeline.name),
            )
        }
        (Some(cron), true) if cron.split_whitespace().count() < 6 => {
            return error(node, format!("the cron '{cron}' needs six fields, seconds first"))
        }
        _ => {}
    }
    for child in node.children() {
        match child.name().as_str() {
            "env" => {
                no_children(&child)?;
                pipeline.env.push(env_of(&child)?);
            }
            "job" => {
                let job = job_of(&child, limits)?;
                if pipeline.jobs.iter().any(|other| other.name == job.name) {
                    return error(
                        &child,
                        format!("two jobs of pipeline {} are named {}", pipeline.name, job.name),
                    );
                }
                pipeline.jobs.push(job);
            }
            other => return error(&child, format!("<pipeline> has no <{other}>")),
        }
    }
    if pipeline.jobs.is_empty() {
        return error(node, format!("pipeline {} has no <job>", pipeline.name));
    }
    let expanded: usize = pipeline
        .jobs
        .iter()
        .map(|job| {
            job.matrix
                .iter()
                .map(|axis| axis.values.len())
                .product::<usize>()
                .max(1)
        })
        .sum();
    if expanded > limits.max_jobs {
        return error(
            node,
            format!(
                "pipeline {} has {expanded} jobs, over this server's limit of {}",
                pipeline.name, limits.max_jobs
            ),
        );
    }
    // needs: each names a job of the pipeline, and no job waits on itself through the others
    for job in &pipeline.jobs {
        for need in &job.needs {
            if need == &job.name || !pipeline.jobs.iter().any(|other| &other.name == need) {
                return error(
                    node,
                    format!(
                        "job {} of pipeline {} needs {need}, which is not another job of it",
                        job.name, pipeline.name
                    ),
                );
            }
        }
    }
    let mut settled: Vec<&str> = Vec::new();
    loop {
        let before = settled.len();
        for job in &pipeline.jobs {
            if !settled.contains(&job.name.as_str()) && job.needs.iter().all(|need| settled.contains(&need.as_str())) {
                settled.push(&job.name);
            }
        }
        if settled.len() == pipeline.jobs.len() {
            break;
        }
        if settled.len() == before {
            let stuck: Vec<&str> = pipeline
                .jobs
                .iter()
                .map(|job| job.name.as_str())
                .filter(|name| !settled.contains(name))
                .collect();
            return error(
                node,
                format!(
                    "the jobs {} of pipeline {} need each other in a circle",
                    stuck.join(", "),
                    pipeline.name
                ),
            );
        }
    }
    Ok(pipeline)
}

/// Read a `ci.xml`.
///
/// # Errors
///
/// The first thing that is not valid, with its line.
pub fn parse(text: &str, limits: &Limits) -> Result<CiFile, FileError> {
    if text.len() > limits.max_bytes {
        return Err(FileError {
            line: 0,
            message: format!(
                "the file is {} bytes, over this server's limit of {}",
                text.len(),
                limits.max_bytes
            ),
        });
    }
    let root = MNode::parse_text("ci.xml", text).map_err(|err| FileError {
        line: 0,
        message: format!("not well-formed XML: {}", err.message()),
    })?;
    if !root.has_name("ci") {
        return error(&root, format!("the root element is <{}>, not <ci>", root.name()));
    }
    check_attributes(&root, &[])?;
    let mut file = CiFile::default();
    for child in root.children() {
        if !child.has_name("pipeline") {
            return error(&child, format!("<ci> has no <{}>", child.name()));
        }
        let pipeline = pipeline_of(&child, limits)?;
        if file.pipelines.iter().any(|other| other.name == pipeline.name) {
            return error(&child, format!("two pipelines are named {}", pipeline.name));
        }
        file.pipelines.push(pipeline);
    }
    if file.pipelines.is_empty() {
        return error(&root, "<ci> has no <pipeline>");
    }
    if file.pipelines.len() > limits.max_pipelines {
        return error(
            &root,
            format!(
                "the file has {} pipelines, over this server's limit of {}",
                file.pipelines.len(),
                limits.max_pipelines
            ),
        );
    }
    Ok(file)
}

#[cfg(test)]
#[path = "file_tests.rs"]
mod tests;
