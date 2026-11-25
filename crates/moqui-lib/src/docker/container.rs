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

//! Containers that stay, and their volumes (plans/Vault-Instances.md §4.2): create without starting (with the secret
//! files put in before it can start), start, stop, restart, inspect, logs and remove, on Docker and on Podman. Every id
//! and name that goes into a path is checked first ([`valid_name`]).

use std::collections::BTreeMap;
use std::time::Duration;

use serde_json::json;

use super::archive::secrets_tar;
use super::podman::{is_source_name, problem_text, read_text};
use super::spec::{valid_name, ContainerSpec, SECRETS_LABEL};
use super::{DockerHost, Engine, CALL_TIMEOUT};
use crate::error::Error;
use crate::http::{Method, OpenRequest};

/// The longest log tail asked for.
pub const MAX_LOG_LINES: u32 = 10_000;
/// The longest grace a stop or restart gives a container before it is killed.
pub const MAX_GRACE: Duration = Duration::from_secs(10 * 60);

/// What the host says of a container.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ContainerInfo {
    pub id: String,
    /// Its name, without Docker's leading `/`.
    pub name: String,
    /// `created`, `running`, `restarting`, `paused`, `exited` or `dead`.
    pub status: String,
    pub running: bool,
    pub exit_code: Option<i64>,
    /// `starting`, `healthy` or `unhealthy`, when it has a health check.
    pub health: Option<String>,
    /// The id of its image (`sha256:...`), the digest of the image's configuration.
    pub image_id: String,
    /// The image's registry digests (`name@sha256:...`), when it came from a registry.
    pub repo_digests: Vec<String>,
    pub labels: BTreeMap<String, String>,
}

/// What the host says of a volume.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct VolumeInfo {
    pub name: String,
    pub driver: String,
    pub mountpoint: String,
    pub labels: BTreeMap<String, String>,
}

fn string_map(value: &serde_json::Value) -> BTreeMap<String, String> {
    value
        .as_object()
        .map(|map| {
            map.iter()
                .filter_map(|(name, value)| Some((name.clone(), value.as_str()?.to_string())))
                .collect()
        })
        .unwrap_or_default()
}

/// `id` when it may go into a path.
fn checked<'a>(what: &str, id: &'a str) -> Result<&'a str, Error> {
    if valid_name(id) {
        Ok(id)
    } else {
        Err(Error::new(format!("{id:?} is not a {what} id or name")))
    }
}

/// The text of a log answer: a stream of frames, each an 8-byte header (stream, 0, 0, 0, a big-endian length) and its
/// bytes, for a container with no terminal; the bytes as they are for one with a terminal.
pub fn demux_logs(bytes: &[u8]) -> String {
    let framed = |rest: &[u8]| rest.len() >= 8 && rest[0] <= 2 && rest[1..4] == [0, 0, 0];
    if !framed(bytes) {
        return String::from_utf8_lossy(bytes).into_owned();
    }
    let mut out = Vec::with_capacity(bytes.len());
    let mut rest = bytes;
    while framed(rest) {
        let length = u32::from_be_bytes([rest[4], rest[5], rest[6], rest[7]]);
        let length = usize::try_from(length).unwrap_or(usize::MAX);
        let body = &rest[8..];
        let (frame, after) = body.split_at(length.min(body.len()));
        out.extend_from_slice(frame);
        rest = after;
    }
    String::from_utf8_lossy(&out).into_owned()
}

impl DockerHost {
    /// Make `spec`'s container without starting it: on Docker the container, then its secret files put into it; on
    /// Podman its secrets, then the container that mounts them. Its id. Nothing is left when a step fails.
    ///
    /// # Errors
    ///
    /// When [`ContainerSpec::check`] refuses the spec (nothing is sent then), or the host refuses a step.
    pub fn create_spec(&self, spec: &ContainerSpec) -> Result<String, Error> {
        spec.check()?;
        if let Some(network) = &spec.network {
            checked("network", network)?;
        }
        if self.engine()? == Engine::Podman {
            return self.create_podman(spec);
        }
        let id = self.create(&spec.name, &spec.docker_body(), spec.pull)?;
        if !spec.secrets.is_empty() {
            if let Err(err) = self.put_secret_files(&id, spec) {
                if let Err(removed) = self.remove(&id) {
                    tracing::error!("container {id}, whose secret files were not put, was not removed: {removed}");
                }
                return Err(err);
            }
        }
        Ok(id)
    }

    /// Put `spec`'s secret files into the created container `id` (`PUT /containers/{id}/archive?path=/`).
    fn put_secret_files(&self, id: &str, spec: &ContainerSpec) -> Result<(), Error> {
        let id = checked("container", id)?;
        let tar = secrets_tar(&spec.secrets)?;
        let length = u64::try_from(tar.len()).unwrap_or(u64::MAX);
        let response = self.client.upload(
            OpenRequest {
                method: Method::Put,
                url: self.url(&format!("/containers/{id}/archive?path=%2F")),
                headers: vec![("content-type".to_string(), "application/x-tar".to_string())],
                body: None,
                timeout: Some(CALL_TIMEOUT),
            },
            Box::new(std::io::Cursor::new(tar)),
            Some(length),
        )?;
        let status = response.status;
        let text = read_text(response.reader);
        if status >= 300 {
            return Err(problem_text(
                &format!("putting the secret files into container {}", spec.name),
                status,
                &text,
            ));
        }
        Ok(())
    }

    /// Stop container `id`, killing it after `grace` (at most [`MAX_GRACE`]); one stopped already is stopped.
    ///
    /// # Errors
    ///
    /// When the host refuses it, or has no such container.
    pub fn stop(&self, id: &str, grace: Duration) -> Result<(), Error> {
        let id = checked("container", id)?;
        let grace = grace.min(MAX_GRACE);
        let response = self.send(
            Method::Post,
            &format!("/containers/{id}/stop?t={}", grace.as_secs()),
            None,
            CALL_TIMEOUT.saturating_add(grace),
        )?;
        // 304: stopped already
        if response.status() >= 300 && response.status() != 304 {
            return Err(Self::problem(&format!("stopping container {id}"), &response));
        }
        Ok(())
    }

    /// Rename container `id` to `name` (a deploy sets the old one aside while its replacement takes the name).
    ///
    /// # Errors
    ///
    /// When the name is not one, the host refuses it (the name is taken), or has no such container.
    pub fn rename(&self, id: &str, name: &str) -> Result<(), Error> {
        let id = checked("container", id)?;
        if !super::valid_name(name) {
            return Err(Error::new(format!("{name:?} is not a container name")));
        }
        let response = self.send(
            Method::Post,
            &format!("/containers/{id}/rename?name={}", super::encode(name)),
            None,
            CALL_TIMEOUT,
        )?;
        if response.status() >= 300 {
            return Err(Self::problem(&format!("renaming container {id}"), &response));
        }
        Ok(())
    }

    /// Restart container `id`, killing it after `grace` (at most [`MAX_GRACE`]) if it does not stop.
    ///
    /// # Errors
    ///
    /// When the host refuses it, or has no such container.
    pub fn restart(&self, id: &str, grace: Duration) -> Result<(), Error> {
        let id = checked("container", id)?;
        let grace = grace.min(MAX_GRACE);
        let response = self.send(
            Method::Post,
            &format!("/containers/{id}/restart?t={}", grace.as_secs()),
            None,
            CALL_TIMEOUT.saturating_add(grace),
        )?;
        if response.status() >= 300 {
            return Err(Self::problem(&format!("restarting container {id}"), &response));
        }
        Ok(())
    }

    /// What the host says of container `id`: its state, health and image. `None` when the host has no such container.
    ///
    /// # Errors
    ///
    /// When the host refuses it, or its answer is not JSON.
    pub fn inspect(&self, id: &str) -> Result<Option<ContainerInfo>, Error> {
        let id = checked("container", id)?;
        let response = self.send(Method::Get, &format!("/containers/{id}/json"), None, CALL_TIMEOUT)?;
        if response.status() == 404 {
            return Ok(None);
        }
        if response.status() >= 300 {
            return Err(Self::problem(&format!("inspecting container {id}"), &response));
        }
        let body: serde_json::Value = serde_json::from_str(response.body())
            .map_err(|err| Error::new(format!("inspecting container {id}: the answer is not JSON: {err}")))?;
        let state = &body["State"];
        let image_id = body["Image"].as_str().unwrap_or_default().to_string();
        Ok(Some(ContainerInfo {
            id: body["Id"].as_str().unwrap_or(id).to_string(),
            name: body["Name"]
                .as_str()
                .unwrap_or_default()
                .trim_start_matches('/')
                .to_string(),
            status: state["Status"].as_str().unwrap_or_default().to_string(),
            running: state["Running"].as_bool().unwrap_or(false),
            exit_code: state["ExitCode"].as_i64(),
            health: state["Health"]["Status"]
                .as_str()
                .filter(|health| !health.is_empty())
                .map(str::to_string),
            repo_digests: self.repo_digests(&image_id),
            image_id,
            labels: string_map(&body["Config"]["Labels"]),
        }))
    }

    /// The registry digests of the image `image_id`; none when it has none or the host does not say.
    fn repo_digests(&self, image_id: &str) -> Vec<String> {
        let name = image_id.strip_prefix("sha256:").unwrap_or(image_id);
        if !valid_name(name) {
            return Vec::new();
        }
        match self.send(Method::Get, &format!("/images/{name}/json"), None, CALL_TIMEOUT) {
            Ok(response) if response.status() < 300 => serde_json::from_str::<serde_json::Value>(response.body())
                .ok()
                .and_then(|body| {
                    body["RepoDigests"].as_array().map(|digests| {
                        digests
                            .iter()
                            .filter_map(|digest| digest.as_str().map(str::to_string))
                            .collect()
                    })
                })
                .unwrap_or_default(),
            Ok(_) => Vec::new(),
            Err(err) => {
                tracing::warn!("the digests of image {image_id} were not read: {err}");
                Vec::new()
            }
        }
    }

    /// The last `lines` lines (at most [`MAX_LOG_LINES`]) of container `id`'s output and errors, interleaved as they
    /// came.
    ///
    /// # Errors
    ///
    /// When the host refuses it, or has no such container.
    pub fn logs(&self, id: &str, lines: u32) -> Result<String, Error> {
        let id = checked("container", id)?;
        let response = self.send(
            Method::Get,
            &format!(
                "/containers/{id}/logs?stdout=true&stderr=true&tail={}",
                lines.min(MAX_LOG_LINES)
            ),
            None,
            CALL_TIMEOUT,
        )?;
        if response.status() >= 300 {
            return Err(Self::problem(&format!("reading the logs of container {id}"), &response));
        }
        Ok(demux_logs(response.bytes()))
    }

    /// Remove container `id`, running or not, with its anonymous volumes when `volumes` (a named volume stays until
    /// [`Self::remove_volume`]); on Podman its secrets too: those its [`SECRETS_LABEL`] lists that have the shape of a
    /// secret this driver made. One that is not there is removed. A container made by [`Self::create_spec`] is removed
    /// by this, not by [`Self::remove`], which leaves a Podman container's secrets on the host.
    ///
    /// # Errors
    ///
    /// When the host refuses it. A secret that is not deleted is logged, not an error.
    pub fn remove_container(&self, id: &str, volumes: bool) -> Result<(), Error> {
        let id = checked("container", id)?;
        let secrets: Vec<String> = if self.engine()? == Engine::Podman {
            self.inspect(id)?
                .and_then(|info| info.labels.get(SECRETS_LABEL).cloned())
                .map(|names| {
                    names
                        .split(',')
                        .filter(|name| is_source_name(name))
                        .map(str::to_string)
                        .collect()
                })
                .unwrap_or_default()
        } else {
            Vec::new()
        };
        let response = self.send(
            Method::Delete,
            &format!("/containers/{id}?force=true&v={volumes}"),
            None,
            CALL_TIMEOUT,
        )?;
        if response.status() >= 300 && response.status() != 404 {
            self.gone_anyway(id, &response)?;
        }
        self.delete_podman_secrets(&secrets);
        Ok(())
    }

    /// After a removal the engine answered with an error: `Ok` (and the answer logged) when the container is gone all
    /// the same, which rootless Podman does when only its network's cleanup fails (on Ubuntu 26.04 its AppArmor
    /// profile for pasta refuses Podman's signal); else the engine's error.
    pub(crate) fn gone_anyway(&self, id: &str, response: &crate::http::RestResponse) -> Result<(), Error> {
        let problem = Self::problem(&format!("removing container {id}"), response);
        if matches!(self.inspect(id), Ok(None)) {
            tracing::warn!("{problem}; the container is gone, so it is taken as removed");
            return Ok(());
        }
        Err(problem)
    }

    /// Make the named volume `name` (the `local` driver); one there already is kept as it is.
    ///
    /// # Errors
    ///
    /// When the host refuses it.
    pub fn create_volume(&self, name: &str, labels: &[(String, String)]) -> Result<(), Error> {
        let name = checked("volume", name)?;
        let labels: serde_json::Map<String, serde_json::Value> = labels
            .iter()
            .map(|(name, value)| (name.clone(), json!(value)))
            .collect();
        let response = self.send(
            Method::Post,
            "/volumes/create",
            Some(&json!({"Name": name, "Driver": "local", "Labels": labels})),
            CALL_TIMEOUT,
        )?;
        if response.status() >= 300 {
            return Err(Self::problem(&format!("creating volume {name}"), &response));
        }
        Ok(())
    }

    /// What the host says of the volume `name`; `None` when it has no such volume.
    ///
    /// # Errors
    ///
    /// When the host refuses it, or its answer is not JSON.
    pub fn inspect_volume(&self, name: &str) -> Result<Option<VolumeInfo>, Error> {
        let name = checked("volume", name)?;
        let response = self.send(Method::Get, &format!("/volumes/{name}"), None, CALL_TIMEOUT)?;
        if response.status() == 404 {
            return Ok(None);
        }
        if response.status() >= 300 {
            return Err(Self::problem(&format!("inspecting volume {name}"), &response));
        }
        let body: serde_json::Value = serde_json::from_str(response.body())
            .map_err(|err| Error::new(format!("inspecting volume {name}: the answer is not JSON: {err}")))?;
        Ok(Some(VolumeInfo {
            name: body["Name"].as_str().unwrap_or(name).to_string(),
            driver: body["Driver"].as_str().unwrap_or_default().to_string(),
            mountpoint: body["Mountpoint"].as_str().unwrap_or_default().to_string(),
            labels: string_map(&body["Labels"]),
        }))
    }

    /// Remove the volume `name` and what is in it; one that is not there is removed. One a container still uses is
    /// refused by the host.
    ///
    /// # Errors
    ///
    /// When the host refuses it.
    pub fn remove_volume(&self, name: &str) -> Result<(), Error> {
        let name = checked("volume", name)?;
        let response = self.send(Method::Delete, &format!("/volumes/{name}"), None, CALL_TIMEOUT)?;
        if response.status() >= 300 && response.status() != 404 {
            return Err(Self::problem(&format!("removing volume {name}"), &response));
        }
        Ok(())
    }

    /// Remove the network `name`; one that is not there is removed.
    ///
    /// # Errors
    ///
    /// When the host refuses it (a container is still on it).
    pub fn delete_network(&self, name: &str) -> Result<(), Error> {
        let name = checked("network", name)?;
        let response = self.send(Method::Delete, &format!("/networks/{name}"), None, CALL_TIMEOUT)?;
        if response.status() >= 300 && response.status() != 404 {
            return Err(Self::problem(&format!("removing network {name}"), &response));
        }
        Ok(())
    }
}
