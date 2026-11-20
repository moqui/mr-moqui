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

//! Podman's own API for what its Docker-compatible API does not do (plans/Vault-Instances.md §4.2): a container's
//! secret files are Podman secrets (`POST /libpod/secrets/create`), mounted by the container's SpecGenerator
//! (`POST /libpod/containers/create`, `secrets: [{source, target, uid, gid, mode}]`) and deleted when the container
//! is removed. A host is Podman when `GET /version` names the component "Podman Engine".

use std::io::Read;

use zeroize::Zeroizing;

use super::spec::ContainerSpec;
use super::{Created, DockerHost, CALL_TIMEOUT};
use crate::error::Error;
use crate::http::{Method, OpenRequest};

/// The libpod API's version in its paths: secrets with an owner and a mode, and the SpecGenerator's `Networks`, are
/// Podman 4's.
pub const LIBPOD: &str = "/v4.0.0/libpod";
/// The most of an error answer read for its message.
const MESSAGE_BYTES: u64 = 64 * 1024;

/// Whether a `GET /version` answer is Podman's.
pub fn is_podman(version: &serde_json::Value) -> bool {
    version["Components"]
        .as_array()
        .is_some_and(|components| components.iter().any(|component| component["Name"] == "Podman Engine"))
}

/// The prefix of the Podman secrets this driver makes.
const SOURCE_PREFIX: &str = "moqui-";
/// The hex digits after it.
const SOURCE_HEX: usize = 24;

/// A new Podman secret's name: [`SOURCE_PREFIX`] and 24 random hex digits, so it is short whatever the container's and
/// the file's names, and a secret left by a failed attempt never stands in the way of the next. The file's name in the
/// container is the mount's target, not this.
fn source_name() -> Result<String, Error> {
    let mut nonce = [0u8; SOURCE_HEX / 2];
    getrandom::fill(&mut nonce).map_err(|err| Error::new(format!("no randomness for a secret's name: {err}")))?;
    let nonce: String = nonce.iter().map(|byte| format!("{byte:02x}")).collect();
    Ok(format!("{SOURCE_PREFIX}{nonce}"))
}

/// Whether `name` has the shape of a Podman secret this driver made ([`source_name`]): only those are deleted when a
/// label names them.
pub(crate) fn is_source_name(name: &str) -> bool {
    name.strip_prefix(SOURCE_PREFIX).is_some_and(|hex| {
        hex.len() == SOURCE_HEX
            && hex
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    })
}

impl DockerHost {
    /// The URL of `path` under the libpod API.
    fn libpod_url(&self, path: &str) -> String {
        format!("{}{LIBPOD}{path}", self.base)
    }

    /// Make the Podman secret `name` holding `bytes`. The value goes in the request's body only, from a buffer that is
    /// zeroized when dropped.
    fn create_podman_secret(&self, name: &str, bytes: &[u8]) -> Result<(), Error> {
        let body = Zeroizing::new(bytes.to_vec());
        let length = u64::try_from(body.len()).unwrap_or(u64::MAX);
        let response = self.client.upload(
            OpenRequest {
                method: Method::Post,
                url: self.libpod_url(&format!("/secrets/create?name={}", super::encode(name))),
                headers: vec![("content-type".to_string(), "application/octet-stream".to_string())],
                body: None,
                timeout: Some(CALL_TIMEOUT),
            },
            Box::new(std::io::Cursor::new(body)),
            Some(length),
        )?;
        let status = response.status;
        let text = read_text(response.reader);
        if status >= 300 {
            return Err(problem_text(
                &format!("creating the Podman secret {name}"),
                status,
                &text,
            ));
        }
        Ok(())
    }

    /// Delete the Podman secret `name`; one that is not there is deleted.
    pub(crate) fn delete_podman_secret(&self, name: &str) -> Result<(), Error> {
        let response = self
            .client
            .request(
                Method::Delete,
                self.libpod_url(&format!("/secrets/{}", super::encode(name))),
            )
            .timeout(CALL_TIMEOUT)
            .allow_in_sim(true)
            .send(false)?;
        if response.status() >= 300 && response.status() != 404 {
            return Err(Self::problem(&format!("deleting the Podman secret {name}"), &response));
        }
        Ok(())
    }

    /// Delete each of `names`, logging a failure and going on with the rest.
    pub(crate) fn delete_podman_secrets(&self, names: &[String]) {
        for name in names {
            if let Err(err) = self.delete_podman_secret(name) {
                tracing::error!("the Podman secret {name} was not deleted: {err}");
            }
        }
    }

    /// One `POST /libpod/containers/create` of `body`.
    fn create_podman_container(&self, name: &str, body: &serde_json::Value) -> Result<Created, Error> {
        let response = self
            .client
            .request(Method::Post, self.libpod_url("/containers/create"))
            .timeout(CALL_TIMEOUT)
            .allow_in_sim(true)
            .json(body)
            .send(false)?;
        if response.status() == 404 && super::names_an_image(response.body()) {
            return Ok(Created::NoSuchImage);
        }
        if response.status() >= 300 {
            return Err(Self::problem(&format!("creating container {name}"), &response));
        }
        serde_json::from_str::<serde_json::Value>(response.body())
            .ok()
            .and_then(|body| body["Id"].as_str().map(str::to_string))
            .map(Created::Id)
            .ok_or_else(|| Error::new(format!("creating container {name}: the answer has no Id")))
    }

    /// Make `spec`'s container on Podman: each secret file a Podman secret first, then the container that mounts
    /// them. When a step fails, the secrets made are deleted.
    pub(crate) fn create_podman(&self, spec: &ContainerSpec) -> Result<String, Error> {
        let mut sources: Vec<String> = Vec::new();
        let made = (|| -> Result<String, Error> {
            for secret in &spec.secrets {
                let source = source_name()?;
                // listed first, so a create that fails after Podman made it is still cleaned up (a delete of one that
                // is not there is a delete)
                sources.push(source.clone());
                self.create_podman_secret(&source, &secret.bytes)?;
            }
            let body = spec.podman_body(&sources);
            match self.create_podman_container(&spec.name, &body)? {
                Created::Id(id) => Ok(id),
                Created::NoSuchImage if spec.pull => {
                    self.pull(&spec.image)?;
                    match self.create_podman_container(&spec.name, &body)? {
                        Created::Id(id) => Ok(id),
                        Created::NoSuchImage => Err(Error::new(format!(
                            "creating container {}: the image {} is not on the host after its pull",
                            spec.name, spec.image
                        ))),
                    }
                }
                Created::NoSuchImage => Err(Error::new(format!(
                    "the image {} is not on the host, and it is not pulled on demand: pull it on the host first",
                    spec.image
                ))),
            }
        })();
        if made.is_err() {
            self.delete_podman_secrets(&sources);
        }
        made
    }
}

/// The text of an answer's body, read no further than [`MESSAGE_BYTES`].
pub(crate) fn read_text(reader: Box<dyn Read + Send>) -> String {
    let mut text = Vec::new();
    // a body that cannot be read leaves the status to speak
    let _ = reader.take(MESSAGE_BYTES).read_to_end(&mut text);
    String::from_utf8_lossy(&text).into_owned()
}

/// [`DockerHost::problem`] of an answer read as text.
pub(crate) fn problem_text(what: &str, status: u16, text: &str) -> Error {
    let message = serde_json::from_str::<serde_json::Value>(text)
        .ok()
        .and_then(|body| body["message"].as_str().map(str::to_string))
        .unwrap_or_else(|| text.chars().take(300).collect());
    Error::new(format!("{what}: {status} {message}"))
}
