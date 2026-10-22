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

//! `org.moqui.impl.DavServices.serve#DavRequest`: one request to the DAV server (`crate::dav`) without a socket, for a
//! spec or a script that checks an application's DAV adapter. The request runs in a context of its own, as on a request
//! worker: authorization on, logged in by its own `Authorization` header; the caller's user and context are untouched.

use std::collections::BTreeMap;

use moqui_lib::error::Error as LibError;
use moqui_lib::object::{is_empty, to_plain_string, Value};

use crate::context::ExecutionContext;
use crate::entity::EntityFacade;

pub(in crate::service) fn register(
    map: &mut BTreeMap<&'static str, crate::service::rust_services::RustServiceFn>,
) -> Result<(), String> {
    rust_service!(map, serve_request)?;
    rust_service!(map, get_sync_addresses)?;
    Ok(())
}

fn serve_request(
    parameters: &BTreeMap<String, Value>,
    _facade: &EntityFacade,
) -> Result<BTreeMap<String, Value>, LibError> {
    let text = |key: &str| {
        parameters
            .get(key)
            .filter(|value| !is_empty(value))
            .map(to_plain_string)
            .unwrap_or_default()
    };
    let mut headers: Vec<(String, String)> = Vec::new();
    if let Some(Value::Map(map)) = parameters.get("headers") {
        for (name, value) in map {
            headers.push((name.to_ascii_lowercase(), to_plain_string(value)));
        }
    }
    let request = crate::dav::DavRequest {
        method: text("method"),
        path: text("path"),
        headers,
        body: text("body").into_bytes(),
        secure: true,
        client_ip: "127.0.0.1".to_string(),
    };
    // a fresh context, as a request worker gives the server one: authorization on, no user until the request logs in
    let factory = ExecutionContext::with_current(|ec| std::sync::Arc::clone(ec.factory()));
    let response = match factory {
        Some(factory) => {
            let context = factory.new_context();
            let _entered = context.enter();
            crate::dav::serve(&factory, &request)
        }
        None => crate::dav::DavResponse::text(500, "no execution context"),
    };
    let headers: indexmap::IndexMap<String, Value> = response
        .headers
        .iter()
        .map(|(name, value)| (name.clone(), Value::string(value)))
        .collect();
    let mut out = BTreeMap::new();
    out.insert("status".to_string(), Value::I32(i32::from(response.status)));
    out.insert("headers".to_string(), Value::Map(headers));
    out.insert(
        "body".to_string(),
        Value::string(String::from_utf8_lossy(&response.body).into_owned()),
    );
    Ok(out)
}

/// `get#SyncAddresses`: where the current user's devices sync: the DAV server's address and the user's homes on it,
/// whether CalDAV and CardDAV are served, and the SFTP address of the drives when the SSH server takes them.
fn get_sync_addresses(
    parameters: &BTreeMap<String, Value>,
    _facade: &EntityFacade,
) -> Result<BTreeMap<String, Value>, LibError> {
    let Some(factory) = ExecutionContext::with_current(|ec| std::sync::Arc::clone(ec.factory())) else {
        return Ok(BTreeMap::new());
    };
    let username = crate::user::current_username().ok().flatten().unwrap_or_default();
    let base = crate::dav::public_base_url(&factory);
    let user = crate::dav::encode(&username);
    let dav = format!("{base}{}/", crate::dav::BASE);
    let host = base
        .split("://")
        .nth(1)
        .unwrap_or(&base)
        .split('/')
        .next()
        .unwrap_or_default()
        .to_string();
    let calendar = !factory
        .conf_setting("dav-server", "calendar-services")
        .trim()
        .is_empty();
    let contacts = !factory.conf_setting("dav-server", "contact-services").trim().is_empty();
    let ssh_setting = |name: &str, property: &str| {
        let configured = factory.conf_setting("ssh-server", name);
        if configured.trim().is_empty() {
            moqui_lib::env::get_prop_or_env(property).trim().to_string()
        } else {
            configured.trim().to_string()
        }
    };
    let ssh_port = Some(ssh_setting("public-port", "ssh_public_port"))
        .filter(|port| !port.is_empty())
        .unwrap_or_else(|| ssh_setting("port", "ssh_port"));
    let sftp =
        (!ssh_port.is_empty() && ssh_port != "0" && factory.conf_flag("dav-server", "sftp-drives", true)).then(|| {
            let ssh_host = Some(ssh_setting("public-host", "ssh_public_host"))
                .filter(|host| !host.is_empty())
                .unwrap_or_else(|| host.split(':').next().unwrap_or_default().to_string());
            if ssh_port == "22" {
                format!("sftp://{user}@{ssh_host}/")
            } else {
                format!("sftp://{user}@{ssh_host}:{ssh_port}/")
            }
        });
    let enabled = factory.conf_flag("dav-server", "enabled", true);
    let mut out = BTreeMap::new();
    let mut put = |key: &str, value: Value| {
        out.insert(key.to_string(), value);
    };
    put("enabled", Value::Bool(enabled));
    put("username", Value::string(&username));
    put("serverHost", Value::string(&host));
    put("davUrl", Value::string(&dav));
    put("principalUrl", Value::string(format!("{dav}principals/{user}/")));
    put("filesUrl", Value::string(format!("{dav}files/{user}/")));
    put(
        "calendarsUrl",
        if calendar {
            Value::string(format!("{dav}calendars/{user}/"))
        } else {
            Value::Null
        },
    );
    put(
        "addressBooksUrl",
        if contacts {
            Value::string(format!("{dav}addressbooks/{user}/"))
        } else {
            Value::Null
        },
    );
    // one drive's folder, named as WebDAV and SFTP name it
    let drive_id = parameters
        .get("driveId")
        .filter(|value| !is_empty(value))
        .map(to_plain_string);
    let folder = drive_id.as_deref().and_then(crate::dav::files::drive_folder);
    put(
        "driveUrl",
        folder.as_ref().map_or(Value::Null, |folder| {
            Value::string(format!("{dav}files/{user}/{}/", crate::dav::encode(folder)))
        }),
    );
    put(
        "driveSftpPath",
        folder
            .as_ref()
            .filter(|_| sftp.is_some())
            .map_or(Value::Null, |folder| Value::string(format!("/{folder}"))),
    );
    put("sftpUrl", sftp.map_or(Value::Null, Value::string));
    Ok(out)
}
