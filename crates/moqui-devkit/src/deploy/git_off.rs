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

//! Deploying a component from a git URL in a build without the git feature (the Lite package): the API of `git.rs`,
//! each call saying this build has no git.

use std::sync::Arc;

use moqui_core::context::ExecutionContextFactory;
use serde::Serialize;

use super::load::{LoadPreview, LoadReport, Mode};

const NO_GIT: &str = "this build has no git (the Lite package): deploying from a git URL needs the git feature";

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Storage {
    #[default]
    Database,
    Bare,
    Clone,
}

impl Storage {
    #[must_use]
    pub fn parse(text: &str) -> Option<Self> {
        match text {
            "" | "database" | "db" => Some(Self::Database),
            "bare" => Some(Self::Bare),
            "clone" => Some(Self::Clone),
            _ => None,
        }
    }
}

#[derive(Clone, Debug, Default)]
pub struct DeployRequest {
    pub url: String,
    pub branch: String,
    pub storage: Storage,
    pub directory: String,
    pub username: Option<String>,
    pub token: Option<String>,
}

#[derive(Clone, Debug, Default, Serialize)]
pub struct DeployPreview {
    pub component: String,
    pub load: LoadPreview,
}

#[derive(Clone, Debug, Default, Serialize)]
pub struct UpdatePreview {
    pub component: String,
}

/// # Errors
///
/// Always: this build has no git.
pub fn preview(_factory: &Arc<ExecutionContextFactory>, _request: &DeployRequest) -> Result<DeployPreview, String> {
    Err(NO_GIT.to_string())
}

pub fn deploy(_factory: &Arc<ExecutionContextFactory>, name: &str, _mode: Mode, _data_types: &[String]) -> LoadReport {
    LoadReport {
        component: name.to_string(),
        status: "failed".to_string(),
        problems: vec![NO_GIT.to_string()],
        ..LoadReport::default()
    }
}

/// # Errors
///
/// Always: this build has no git.
pub fn discard(_factory: &Arc<ExecutionContextFactory>, _name: &str) -> Result<(), String> {
    Err(NO_GIT.to_string())
}

/// # Errors
///
/// Always: this build has no git.
pub fn preview_update(_factory: &Arc<ExecutionContextFactory>, _name: &str) -> Result<UpdatePreview, String> {
    Err(NO_GIT.to_string())
}

pub fn update(factory: &Arc<ExecutionContextFactory>, name: &str, data_types: &[String]) -> LoadReport {
    deploy(factory, name, Mode::Now, data_types)
}

/// No component is kept in a repository in this build: nothing to apply.
pub fn install(_factory: &Arc<ExecutionContextFactory>) {}

/// No component is kept in a repository in this build: nothing to write out.
pub fn restore(_factory: &Arc<ExecutionContextFactory>) -> usize {
    0
}
