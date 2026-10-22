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

//! WebDAV over drives: `/dav/files/<username>/` holds each drive the user may open (`FileServices.get#UserDrives`) as a
//! folder named by the drive, and below it the drive's files, through the `FileServices` services as the user, so the
//! drive members, shares, quotas, a records drive's record visibility, versions and the search index all hold as in the
//! Files screen. A PUT is an upload (a new version of a file that is there: one commit on a version-controlled drive),
//! DELETE moves to the drive's trash, MOVE and COPY are the move, rename and copy services (across drives a copy and a
//! trash). The files macOS and Windows write beside each file (`.DS_Store`, `._name`, `Thumbs.db`, `desktop.ini`) are
//! taken and not kept.

use indexmap::IndexMap;
use moqui_lib::object::{HostObject, Value};

use super::{
    call, depth, destination_path, encode, etag_of, flag_of, http_date, locks, maps, millis_of, multistatus, overwrite,
    proppatch, status_of_error, text_of, xml, Context, DavResponse, PropRequest, Resource, BASE,
};
use crate::context::ExecutionContext;
use crate::web::multipart::UploadedFile;

const FILES: &str = "org.moqui.impl.FileServices";

/// A drive as a folder of the user's home.
struct Drive {
    drive_id: String,
    folder: String,
    can_write: bool,
}

/// One file or folder of a drive.
struct Entry {
    path: String,
    name: String,
    is_directory: bool,
    size: Option<i64>,
    modified: Option<i64>,
    version_name: String,
    content_type: String,
    can_write: bool,
}

impl Entry {
    fn from_map(map: &IndexMap<String, Value>, can_write: bool) -> Self {
        Self {
            path: text_of(map, "path"),
            name: text_of(map, "name"),
            is_directory: flag_of(map, "isDirectory"),
            size: map
                .get("size")
                .and_then(|value| moqui_lib::object::to_plain_string(value).parse().ok()),
            modified: millis_of(map, "modified"),
            version_name: text_of(map, "versionName"),
            content_type: text_of(map, "contentType"),
            can_write: map.get("canWrite").map_or(can_write, |_| flag_of(map, "canWrite")),
        }
    }

    fn etag(&self) -> String {
        etag_of(&format!(
            "{}|{}|{:?}|{:?}",
            self.path, self.version_name, self.size, self.modified
        ))
    }
}

/// Files the desktops write beside each file, taken and dropped.
fn is_desktop_litter(name: &str) -> bool {
    name == ".DS_Store"
        || name.starts_with("._")
        || name.eq_ignore_ascii_case("Thumbs.db")
        || name.eq_ignore_ascii_case("desktop.ini")
}

fn home_href(context: &Context<'_>) -> String {
    format!("{BASE}/files/{}/", encode(&context.username))
}

fn entry_href(context: &Context<'_>, drive: &Drive, path: &str, is_directory: bool) -> String {
    let mut href = format!("{}{}/", home_href(context), encode(&drive.folder));
    let encoded: Vec<String> = path.split('/').filter(|part| !part.is_empty()).map(encode).collect();
    href.push_str(&encoded.join("/"));
    if is_directory && !encoded.is_empty() {
        href.push('/');
    }
    href
}

/// The user's drives, each with a folder name of its own: its name, with "/" as "-", and the id after a name two have.
fn drives() -> Result<Vec<Drive>, String> {
    let out = call(&format!("{FILES}.get#UserDrives"), &[])?;
    let rows = maps(out.get("driveList"));
    let base_name = |row: &IndexMap<String, Value>| {
        let name = text_of(row, "driveName").replace('/', "-").trim().to_string();
        if name.is_empty() {
            text_of(row, "driveId")
        } else {
            name
        }
    };
    let mut drives = Vec::new();
    for row in &rows {
        let name = base_name(row);
        let shared = rows
            .iter()
            .filter(|other| base_name(other).eq_ignore_ascii_case(&name))
            .count()
            > 1;
        let drive_id = text_of(row, "driveId");
        drives.push(Drive {
            folder: if shared { format!("{name} ({drive_id})") } else { name },
            can_write: flag_of(row, "canWrite"),
            drive_id,
        });
    }
    Ok(drives)
}

fn entry_resource(context: &Context<'_>, drive: &Drive, entry: &Entry) -> Resource {
    let href = entry_href(context, drive, &entry.path, entry.is_directory);
    let mut resource = Resource::new(href.clone());
    if entry.is_directory {
        resource.prop(xml::DAV, "resourcetype", "<D:collection/>");
    } else {
        resource.prop(xml::DAV, "resourcetype", "");
        resource.prop(xml::DAV, "getcontentlength", &entry.size.unwrap_or(0).to_string());
        let content_type = if entry.content_type.is_empty() {
            "application/octet-stream"
        } else {
            &entry.content_type
        };
        resource.prop(xml::DAV, "getcontenttype", &xml::escape(content_type));
        resource.prop(xml::DAV, "getetag", &xml::escape(&entry.etag()));
    }
    let name = if entry.name.is_empty() {
        drive.folder.clone()
    } else {
        entry.name.clone()
    };
    resource.prop(xml::DAV, "displayname", &xml::escape(&name));
    if let Some(modified) = entry.modified {
        resource.prop(xml::DAV, "getlastmodified", &http_date(modified));
    }
    resource.prop(xml::DAV, "supportedlock", locks::SUPPORTED);
    resource.prop(xml::DAV, "lockdiscovery", &locks::discovery(&href));
    resource.privileges(entry.can_write);
    resource.add_user_props(&context.username);
    resource
}

/// The file or folder at `path` of a drive; `None` when it is not there (or the user may not see it).
fn info(drive: &Drive, path: &str) -> Result<Option<Entry>, String> {
    if path.is_empty() {
        let out = call(
            &format!("{FILES}.list#Folder"),
            &[("driveId", Value::string(&drive.drive_id)), ("path", Value::string(""))],
        );
        return match out {
            Ok(out) => {
                let folder = match out.get("folder") {
                    Some(Value::Map(map)) => map.clone(),
                    _ => IndexMap::new(),
                };
                Ok(Some(Entry {
                    path: String::new(),
                    name: drive.folder.clone(),
                    is_directory: true,
                    size: None,
                    modified: None,
                    version_name: String::new(),
                    content_type: String::new(),
                    can_write: flag_of(&folder, "canWrite"),
                }))
            }
            Err(err) if status_of_error(&err) == 404 => Ok(None),
            Err(err) => Err(err),
        };
    }
    match call(
        &format!("{FILES}.get#ResourceInfo"),
        &[
            ("driveId", Value::string(&drive.drive_id)),
            ("path", Value::string(path)),
        ],
    ) {
        Ok(out) => Ok(match out.get("resource") {
            Some(Value::Map(map)) => Some(Entry::from_map(map, drive.can_write)),
            _ => None,
        }),
        Err(err) if status_of_error(&err) == 404 => Ok(None),
        Err(err) => Err(err),
    }
}

fn children(drive: &Drive, path: &str, can_write: bool) -> Result<Vec<Entry>, String> {
    let out = call(
        &format!("{FILES}.list#Folder"),
        &[
            ("driveId", Value::string(&drive.drive_id)),
            ("path", Value::string(path)),
        ],
    )?;
    Ok(maps(out.get("fileList"))
        .iter()
        .map(|row| Entry::from_map(row, can_write))
        .collect())
}

fn split_parent(path: &str) -> (String, String) {
    match path.rsplit_once('/') {
        Some((parent, name)) => (parent.to_string(), name.to_string()),
        None => (String::new(), path.to_string()),
    }
}

fn server_error(err: &str) -> DavResponse {
    tracing::warn!("dav files: {err}");
    DavResponse::text(500, err)
}

pub(crate) fn handle(context: &Context<'_>, method: &str) -> DavResponse {
    let drives = match drives() {
        Ok(drives) => drives,
        Err(err) => return server_error(&err),
    };
    let Some(folder) = context.rest.first() else {
        return home(context, method, &drives);
    };
    let Some(drive) = drives
        .iter()
        .find(|drive| drive.folder.eq_ignore_ascii_case(folder) || &drive.drive_id == folder)
    else {
        return DavResponse::status(404);
    };
    let path = context.rest[1..].join("/");
    let href = entry_href(context, drive, &path, context.trailing_slash);
    match method {
        "PROPFIND" => propfind(context, drive, &path),
        "GET" | "HEAD" => get(context, drive, &path, method == "HEAD"),
        "PUT" => put(context, drive, &path, &href),
        "DELETE" => delete(context, drive, &path, &href),
        "MKCOL" => mkcol(context, drive, &path),
        "MOVE" | "COPY" => move_or_copy(context, &drives, drive, &path, &href, method == "MOVE"),
        "PROPPATCH" => match info(drive, &path) {
            Ok(Some(_)) => proppatch(&href, context.body.as_ref()),
            Ok(None) => DavResponse::status(404),
            Err(err) => server_error(&err),
        },
        "LOCK" => {
            if let Err(refused) = locks::check_write(&href, &context.user_id) {
                return refused;
            }
            let exists = matches!(info(drive, &path), Ok(Some(_)));
            let mut answer = locks::lock(context.request, &href, &context.user_id, context.body.as_ref());
            if !exists && answer.status == 200 {
                answer.status = 201;
            }
            answer
        }
        "UNLOCK" => locks::unlock(context.request, &href, &context.user_id),
        "REPORT" => DavResponse::condition(403, xml::DAV, "supported-report"),
        _ => DavResponse::status(405),
    }
}

fn home(context: &Context<'_>, method: &str, drives: &[Drive]) -> DavResponse {
    match method {
        "PROPFIND" => {
            let wanted = PropRequest::from_propfind(context.body.as_ref());
            let mut home = Resource::collection(home_href(context), "Files");
            home.privileges(false);
            home.add_user_props(&context.username);
            let mut resources = vec![home];
            if depth(context.request) > 0 {
                for drive in drives {
                    let mut resource = Resource::collection(entry_href(context, drive, "", true), &drive.folder);
                    resource.privileges(drive.can_write);
                    resource.prop(xml::DAV, "supportedlock", locks::SUPPORTED);
                    resource.add_user_props(&context.username);
                    resources.push(resource);
                }
            }
            multistatus(&resources, &wanted)
        }
        "GET" | "HEAD" => {
            let names: Vec<(String, bool)> = drives.iter().map(|drive| (drive.folder.clone(), true)).collect();
            listing_page(&home_href(context), "Files", &names, method == "HEAD")
        }
        "MKCOL" | "PUT" | "DELETE" | "MOVE" | "COPY" => DavResponse::text(403, "drives are added on the Drives screen"),
        _ => DavResponse::status(405),
    }
}

fn propfind(context: &Context<'_>, drive: &Drive, path: &str) -> DavResponse {
    let wanted = PropRequest::from_propfind(context.body.as_ref());
    let entry = match info(drive, path) {
        Ok(Some(entry)) => entry,
        Ok(None) => return DavResponse::status(404),
        Err(err) => return server_error(&err),
    };
    let mut resources = vec![entry_resource(context, drive, &entry)];
    if entry.is_directory && depth(context.request) > 0 {
        match children(drive, path, entry.can_write) {
            Ok(list) => resources.extend(list.iter().map(|child| entry_resource(context, drive, child))),
            Err(err) => return server_error(&err),
        }
    }
    multistatus(&resources, &wanted)
}

/// A simple page of a folder's names, for a browser that opens a DAV address.
fn listing_page(href: &str, title: &str, names: &[(String, bool)], head: bool) -> DavResponse {
    let mut page = format!(
        "<!doctype html><meta charset=\"utf-8\"><title>{0}</title><h1>{0}</h1><ul>",
        xml::escape(title)
    );
    for (name, is_directory) in names {
        let link = format!("{}{}{}", href, encode(name), if *is_directory { "/" } else { "" });
        page.push_str(&format!(
            "<li><a href=\"{}\">{}</a></li>",
            xml::escape(&link),
            xml::escape(name)
        ));
    }
    page.push_str("</ul>");
    let length = page.len();
    DavResponse {
        status: 200,
        headers: vec![
            ("Content-Type".into(), "text/html; charset=utf-8".into()),
            ("Content-Length".into(), length.to_string()),
        ],
        body: if head { Vec::new() } else { page.into_bytes() },
    }
}

fn get(context: &Context<'_>, drive: &Drive, path: &str, head: bool) -> DavResponse {
    let entry = match info(drive, path) {
        Ok(Some(entry)) => entry,
        Ok(None) => return DavResponse::status(404),
        Err(err) => return server_error(&err),
    };
    if entry.is_directory {
        let names: Vec<(String, bool)> = match children(drive, path, entry.can_write) {
            Ok(list) => list
                .iter()
                .map(|child| (child.name.clone(), child.is_directory))
                .collect(),
            Err(err) => return server_error(&err),
        };
        let href = entry_href(context, drive, path, true);
        let href = if href.ends_with('/') { href } else { format!("{href}/") };
        return listing_page(&href, &entry.name, &names, head);
    }
    let etag = entry.etag();
    if super::tag_list(context.request.header("if-none-match"))
        .iter()
        .any(|tag| tag == &etag || tag == "*")
    {
        return DavResponse::status(304).with_header("ETag", etag);
    }
    let location = match call(
        &format!("{FILES}.get#ResourceInfo"),
        &[
            ("driveId", Value::string(&drive.drive_id)),
            ("path", Value::string(path)),
        ],
    ) {
        Ok(out) => match out.get("resource") {
            Some(Value::Map(map)) => text_of(map, "location"),
            _ => return DavResponse::status(404),
        },
        Err(err) => return DavResponse::text(status_of_error(&err), &err),
    };
    // get#ResourceInfo checked the user's access; the storage is entity rows the user's own authorization may not read
    let bytes = ExecutionContext::with_current(|ec| {
        let _authz = ec.disable_authz();
        ec.factory().resource().location_bytes(&location)
    });
    let bytes = match bytes {
        Some(Ok(Some(bytes))) => bytes,
        Some(Ok(None)) => return DavResponse::status(404),
        Some(Err(err)) => return server_error(err.message()),
        None => return server_error("no execution context"),
    };
    let content_type = if entry.content_type.is_empty() {
        "application/octet-stream".to_string()
    } else {
        entry.content_type.clone()
    };
    let mut response = DavResponse {
        status: 200,
        headers: vec![
            ("Content-Type".into(), content_type),
            ("ETag".into(), etag),
            ("Accept-Ranges".into(), "bytes".into()),
        ],
        body: Vec::new(),
    };
    if let Some(modified) = entry.modified {
        response.headers.push(("Last-Modified".into(), http_date(modified)));
    }
    let total = bytes.len();
    let range = context
        .request
        .header("range")
        .and_then(|range| byte_range(range, total));
    let body = match range {
        Some((start, end)) => {
            response.status = 206;
            response
                .headers
                .push(("Content-Range".into(), format!("bytes {start}-{end}/{total}")));
            bytes[start..=end].to_vec()
        }
        None => bytes,
    };
    response.headers.push(("Content-Length".into(), body.len().to_string()));
    if !head {
        response.body = body;
    }
    response
}

/// One `bytes=start-end` range (or `start-`, or `-suffix`) within `total`; others are served whole.
fn byte_range(header: &str, total: usize) -> Option<(usize, usize)> {
    let spec = header.trim().strip_prefix("bytes=")?;
    if spec.contains(',') || total == 0 {
        return None;
    }
    let (start, end) = spec.split_once('-')?;
    let (start, end) = match (start.trim(), end.trim()) {
        ("", suffix) => {
            let suffix: usize = suffix.parse().ok()?;
            (total.saturating_sub(suffix), total - 1)
        }
        (start, "") => (start.parse().ok()?, total - 1),
        (start, end) => (start.parse().ok()?, end.parse::<usize>().ok()?.min(total - 1)),
    };
    (start <= end && start < total).then_some((start, end))
}

fn put(context: &Context<'_>, drive: &Drive, path: &str, href: &str) -> DavResponse {
    if path.is_empty() {
        return DavResponse::status(405);
    }
    let (parent, name) = split_parent(path);
    if is_desktop_litter(&name) {
        return DavResponse::status(201);
    }
    if let Err(refused) = locks::check_write(href, &context.user_id) {
        return refused;
    }
    let existing = match info(drive, path) {
        Ok(existing) => existing,
        Err(err) => return server_error(&err),
    };
    if existing.as_ref().is_some_and(|entry| entry.is_directory) {
        return DavResponse::status(405);
    }
    let none_match = super::tag_list(context.request.header("if-none-match"));
    if existing.is_some()
        && none_match
            .iter()
            .any(|tag| tag == "*" || existing.as_ref().is_some_and(|entry| &entry.etag() == tag))
    {
        return DavResponse::status(412);
    }
    let if_match = super::tag_list(context.request.header("if-match"));
    if !if_match.is_empty() {
        let current = existing.as_ref().map(Entry::etag);
        if !if_match
            .iter()
            .any(|tag| (tag == "*" && current.is_some()) || current.as_ref() == Some(tag))
        {
            return DavResponse::status(412);
        }
    }
    match info(drive, &parent) {
        Ok(Some(folder)) if folder.is_directory => {}
        Ok(_) => return DavResponse::status(409),
        Err(err) => return server_error(&err),
    }
    let content_type = context
        .request
        .header("content-type")
        .unwrap_or("application/octet-stream")
        .to_string();
    let file = UploadedFile::in_memory("file", &name, &content_type, context.request.body.clone());
    let out = call(
        &format!("{FILES}.upload#Resource"),
        &[
            ("driveId", Value::string(&drive.drive_id)),
            ("path", Value::string(&parent)),
            ("file", Value::Object(HostObject::new(file))),
            ("conflictPolicy", Value::string("replace")),
        ],
    );
    let out = match out {
        Ok(out) => out,
        Err(err) => return DavResponse::text(status_of_error(&err), &err),
    };
    if let Some(row) = maps(out.get("uploadedList")).first() {
        if text_of(row, "outcome") == "failed" {
            let error = text_of(row, "error");
            return DavResponse::text(status_of_error(&error), &error);
        }
    }
    let mut answer = DavResponse::status(if existing.is_some() { 204 } else { 201 });
    if let Ok(Some(entry)) = info(drive, path) {
        answer = answer.with_header("ETag", entry.etag());
    }
    answer
}

fn delete(context: &Context<'_>, drive: &Drive, path: &str, href: &str) -> DavResponse {
    if path.is_empty() {
        return DavResponse::text(403, "a drive is removed on the Drives screen");
    }
    if let Err(refused) = locks::check_write(href, &context.user_id) {
        return refused;
    }
    match info(drive, path) {
        Ok(Some(_)) => {}
        Ok(None) => return DavResponse::status(404),
        Err(err) => return server_error(&err),
    }
    match trash(drive, path) {
        Ok(()) => {
            locks::release(href);
            DavResponse::status(204)
        }
        Err(response) => response,
    }
}

fn trash(drive: &Drive, path: &str) -> Result<(), DavResponse> {
    let out = call(
        &format!("{FILES}.trash#Resource"),
        &[
            ("driveId", Value::string(&drive.drive_id)),
            ("path", Value::string(path)),
        ],
    )
    .map_err(|err| DavResponse::text(status_of_error(&err), &err))?;
    if let Some(row) = maps(out.get("trashList")).first() {
        if text_of(row, "outcome") == "failed" {
            let error = text_of(row, "error");
            return Err(DavResponse::text(status_of_error(&error), &error));
        }
    }
    Ok(())
}

fn mkcol(context: &Context<'_>, drive: &Drive, path: &str) -> DavResponse {
    if !context.request.body.is_empty() {
        return DavResponse::status(415);
    }
    if path.is_empty() {
        return DavResponse::status(405);
    }
    match info(drive, path) {
        Ok(Some(_)) => return DavResponse::status(405),
        Ok(None) => {}
        Err(err) => return server_error(&err),
    }
    let (parent, name) = split_parent(path);
    match info(drive, &parent) {
        Ok(Some(folder)) if folder.is_directory => {}
        Ok(_) => return DavResponse::status(409),
        Err(err) => return server_error(&err),
    }
    match call(
        &format!("{FILES}.create#Folder"),
        &[
            ("driveId", Value::string(&drive.drive_id)),
            ("path", Value::string(&parent)),
            ("name", Value::string(&name)),
        ],
    ) {
        Ok(_) => DavResponse::status(201),
        Err(err) => DavResponse::text(status_of_error(&err), &err),
    }
}

fn move_or_copy(
    context: &Context<'_>,
    drives: &[Drive],
    drive: &Drive,
    path: &str,
    href: &str,
    is_move: bool,
) -> DavResponse {
    if path.is_empty() {
        return DavResponse::text(403, "a drive cannot be moved or copied");
    }
    let Some(destination) = destination_path(context.request) else {
        return DavResponse::status(400);
    };
    let home = home_href(context);
    let Some(rest) = destination.strip_prefix(home.as_str()).or_else(|| {
        // the user's name may be written in another case or escaped otherwise
        let prefix = format!("{BASE}/files/");
        let after = destination.strip_prefix(&prefix)?;
        let (user, rest) = after.split_once('/')?;
        super::decode_segment(user)
            .eq_ignore_ascii_case(&context.username)
            .then_some(rest)
    }) else {
        return DavResponse::text(502, "the destination is not in your files");
    };
    let segments: Vec<String> = rest
        .split('/')
        .filter(|part| !part.is_empty())
        .map(super::decode_segment)
        .collect();
    if segments
        .iter()
        .any(|part| part == "." || part == ".." || part.contains('/'))
    {
        return DavResponse::status(400);
    }
    let Some(target_drive) = segments.first().and_then(|folder| {
        drives
            .iter()
            .find(|drive| drive.folder.eq_ignore_ascii_case(folder) || &drive.drive_id == folder)
    }) else {
        return DavResponse::status(409);
    };
    let target_path = segments[1..].join("/");
    if target_path.is_empty() {
        return DavResponse::status(403);
    }
    if target_drive.drive_id == drive.drive_id && (target_path == path || target_path.starts_with(&format!("{path}/")))
    {
        return DavResponse::status(403);
    }
    let target_href = entry_href(context, target_drive, &target_path, false);
    for checked in [Some(target_href.as_str()), is_move.then_some(href)]
        .into_iter()
        .flatten()
    {
        if let Err(refused) = locks::check_write(checked, &context.user_id) {
            return refused;
        }
    }
    match info(drive, path) {
        Ok(Some(_)) => {}
        Ok(None) => return DavResponse::status(404),
        Err(err) => return server_error(&err),
    }
    let target_exists = match info(target_drive, &target_path) {
        Ok(found) => found.is_some(),
        Err(err) => return server_error(&err),
    };
    if target_exists {
        if !overwrite(context.request) {
            return DavResponse::status(412);
        }
        if let Err(response) = trash(target_drive, &target_path) {
            return response;
        }
    }
    let (source_parent, source_name) = split_parent(path);
    let (target_parent, target_name) = split_parent(&target_path);
    match info(target_drive, &target_parent) {
        Ok(Some(folder)) if folder.is_directory => {}
        Ok(_) => return DavResponse::status(409),
        Err(err) => return server_error(&err),
    }
    let same_drive = target_drive.drive_id == drive.drive_id;
    let placed_path = if is_move && same_drive && source_parent == target_parent {
        path.to_string()
    } else {
        let (service, list) = if is_move {
            ("move#Resource", "movedList")
        } else {
            ("copy#Resource", "copiedList")
        };
        let out = call(
            &format!("{FILES}.{service}"),
            &[
                ("driveId", Value::string(&drive.drive_id)),
                ("path", Value::string(path)),
                ("toDriveId", Value::string(&target_drive.drive_id)),
                ("toPath", Value::string(&target_parent)),
                ("conflictPolicy", Value::string("keepBoth")),
            ],
        );
        let out = match out {
            Ok(out) => out,
            Err(err) => return DavResponse::text(status_of_error(&err), &err),
        };
        let Some(row) = maps(out.get(list)).into_iter().next() else {
            return server_error("the move or copy answered nothing");
        };
        if text_of(&row, "outcome") == "failed" {
            let error = text_of(&row, "error");
            return DavResponse::text(status_of_error(&error), &error);
        }
        let placed = text_of(&row, "newPath");
        if placed.is_empty() {
            if target_parent.is_empty() {
                source_name.clone()
            } else {
                format!("{target_parent}/{source_name}")
            }
        } else {
            placed
        }
    };
    let (_, placed_name) = split_parent(&placed_path);
    if placed_name != target_name {
        if let Err(err) = call(
            &format!("{FILES}.rename#Resource"),
            &[
                ("driveId", Value::string(&target_drive.drive_id)),
                ("path", Value::string(&placed_path)),
                ("newName", Value::string(&target_name)),
            ],
        ) {
            return DavResponse::text(status_of_error(&err), &err);
        }
    }
    if is_move {
        locks::release(href);
    }
    DavResponse::status(if target_exists { 204 } else { 201 })
}

#[cfg(test)]
mod tests {
    use super::byte_range;

    #[test]
    fn a_range_is_one_span_inside_the_file() {
        assert_eq!(byte_range("bytes=0-4", 10), Some((0, 4)));
        assert_eq!(byte_range("bytes=5-", 10), Some((5, 9)));
        assert_eq!(byte_range("bytes=-3", 10), Some((7, 9)));
        assert_eq!(byte_range("bytes=8-100", 10), Some((8, 9)));
        assert_eq!(byte_range("bytes=10-12", 10), None);
        assert_eq!(byte_range("bytes=0-1,4-5", 10), None);
    }
}

/// The folder of drive `drive_id` in the user's WebDAV home and SFTP top (its name, made unique), when the user may open
/// the drive.
pub fn drive_folder(drive_id: &str) -> Option<String> {
    drives()
        .ok()?
        .into_iter()
        .find(|drive| drive.drive_id == drive_id)
        .map(|drive| drive.folder)
}

/// The drives of the logged-in user as one file tree, for the SFTP server: `/` holds a folder per drive, named as in
/// WebDAV, and each path below goes through the same services (a write is an upload, a removal goes to the trash, a
/// rename across folders is a move). The desktops' litter is not dropped here, as SFTP clients write none.
pub mod tree {
    use super::{children, drives, info, split_parent, trash, Drive, Entry, FILES};
    use crate::context::{Error, ExecutionContext, ResourceStat};
    use crate::web::multipart::UploadedFile;
    use moqui_lib::object::{HostObject, Value};

    fn refused(text: String) -> Error {
        Error::new(text)
    }

    fn stat_of(entry: &Entry) -> ResourceStat {
        ResourceStat {
            is_file: !entry.is_directory,
            size: entry.size.and_then(|size| u64::try_from(size).ok()).unwrap_or(0),
            last_modified: entry.modified,
        }
    }

    /// The drive and the path inside it of `/<drive>/<path>`; no drive for `/`.
    fn locate(path: &str) -> Result<(Option<Drive>, String), Error> {
        let parts: Vec<&str> = path.split('/').filter(|part| !part.is_empty()).collect();
        let Some(folder) = parts.first() else {
            return Ok((None, String::new()));
        };
        if parts.iter().any(|part| *part == "." || *part == "..") {
            return Err(refused("not found: a path climbs out".into()));
        }
        let drive = drives()
            .map_err(refused)?
            .into_iter()
            .find(|drive| drive.folder.eq_ignore_ascii_case(folder) || &drive.drive_id == folder)
            .ok_or_else(|| refused(format!("not found: no drive {folder}")))?;
        Ok((Some(drive), parts[1..].join("/")))
    }

    /// The file or folder at `path`; `None` when nothing is there.
    pub fn stat(path: &str) -> Result<Option<ResourceStat>, Error> {
        let (drive, inner) = match locate(path) {
            Ok(found) => found,
            Err(err) if err.message().starts_with("not found") => return Ok(None),
            Err(err) => return Err(err),
        };
        let Some(drive) = drive else {
            return Ok(Some(ResourceStat {
                is_file: false,
                size: 0,
                last_modified: None,
            }));
        };
        Ok(info(&drive, &inner).map_err(refused)?.as_ref().map(stat_of))
    }

    /// The names and stats in a folder.
    pub fn list(path: &str) -> Result<Vec<(String, ResourceStat)>, Error> {
        let (drive, inner) = locate(path)?;
        let Some(drive) = drive else {
            return Ok(drives()
                .map_err(refused)?
                .into_iter()
                .map(|drive| {
                    (
                        drive.folder,
                        ResourceStat {
                            is_file: false,
                            size: 0,
                            last_modified: None,
                        },
                    )
                })
                .collect());
        };
        let can_write = drive.can_write;
        Ok(children(&drive, &inner, can_write)
            .map_err(refused)?
            .iter()
            .map(|entry| (entry.name.clone(), stat_of(entry)))
            .collect())
    }

    /// The bytes of a file.
    pub fn read(path: &str) -> Result<Vec<u8>, Error> {
        let (Some(drive), inner) = locate(path)? else {
            return Err(refused("not a file: the top folder holds the drives".into()));
        };
        let out = crate::dav::call(
            &format!("{FILES}.get#ResourceInfo"),
            &[
                ("driveId", Value::string(&drive.drive_id)),
                ("path", Value::string(&inner)),
            ],
        )
        .map_err(refused)?;
        let location = match out.get("resource") {
            Some(Value::Map(map)) if !crate::dav::flag_of(map, "isDirectory") => crate::dav::text_of(map, "location"),
            _ => return Err(refused(format!("not a file: {path}"))),
        };
        // get#ResourceInfo checked the access; the storage is rows the user's own authorization may not read
        ExecutionContext::with_current(|ec| {
            let _authz = ec.disable_authz();
            ec.factory().resource().location_bytes(&location)
        })
        .ok_or_else(|| refused("no execution context".into()))??
        .ok_or_else(|| refused(format!("not found: {path}")))
    }

    /// Write a file: a new one, or a new version of the one there.
    pub fn write(path: &str, bytes: Vec<u8>) -> Result<(), Error> {
        let (Some(drive), inner) = locate(path)? else {
            return Err(refused("permission denied: the top folder holds the drives".into()));
        };
        let (parent, name) = split_parent(&inner);
        if name.is_empty() {
            return Err(refused("permission denied: a drive is not a file".into()));
        }
        let content_type = moqui_lib::resource::content_type_for_name(&name)
            .map_or_else(|| "application/octet-stream".to_string(), str::to_string);
        let file = UploadedFile::in_memory("file", &name, &content_type, bytes);
        let out = crate::dav::call(
            &format!("{FILES}.upload#Resource"),
            &[
                ("driveId", Value::string(&drive.drive_id)),
                ("path", Value::string(&parent)),
                ("file", Value::Object(HostObject::new(file))),
                ("conflictPolicy", Value::string("replace")),
            ],
        )
        .map_err(refused)?;
        if let Some(row) = crate::dav::maps(out.get("uploadedList")).first() {
            if crate::dav::text_of(row, "outcome") == "failed" {
                return Err(refused(crate::dav::text_of(row, "error")));
            }
        }
        Ok(())
    }

    pub fn make_directory(path: &str) -> Result<(), Error> {
        let (Some(drive), inner) = locate(path)? else {
            return Err(refused(
                "permission denied: drives are added on the Drives screen".into(),
            ));
        };
        let (parent, name) = split_parent(&inner);
        if name.is_empty() {
            return Err(refused("exists: the drive is there".into()));
        }
        crate::dav::call(
            &format!("{FILES}.create#Folder"),
            &[
                ("driveId", Value::string(&drive.drive_id)),
                ("path", Value::string(&parent)),
                ("name", Value::string(&name)),
            ],
        )
        .map(|_| ())
        .map_err(refused)
    }

    /// Move a file or folder to its drive's trash; false when nothing is there.
    pub fn remove(path: &str) -> Result<bool, Error> {
        let (Some(drive), inner) = locate(path)? else {
            return Err(refused("permission denied: the top folder holds the drives".into()));
        };
        if inner.is_empty() {
            return Err(refused(
                "permission denied: a drive is removed on the Drives screen".into(),
            ));
        }
        if info(&drive, &inner).map_err(refused)?.is_none() {
            return Ok(false);
        }
        trash(&drive, &inner).map_err(|response| refused(String::from_utf8_lossy(&response.body).into_owned()))?;
        Ok(true)
    }

    /// Rename or move a file or folder, within a drive or to another.
    pub fn rename(from: &str, to: &str) -> Result<(), Error> {
        let (Some(source), source_path) = locate(from)? else {
            return Err(refused("permission denied: the top folder holds the drives".into()));
        };
        let (Some(target), target_path) = locate(to)? else {
            return Err(refused("permission denied: the top folder holds the drives".into()));
        };
        if source_path.is_empty() || target_path.is_empty() {
            return Err(refused(
                "permission denied: a drive is renamed on the Drives screen".into(),
            ));
        }
        if info(&target, &target_path).map_err(refused)?.is_some() {
            return Err(refused(format!("exists: {to}")));
        }
        let (source_parent, source_name) = split_parent(&source_path);
        let (target_parent, target_name) = split_parent(&target_path);
        let mut placed = source_path.clone();
        if source.drive_id != target.drive_id || source_parent != target_parent {
            let out = crate::dav::call(
                &format!("{FILES}.move#Resource"),
                &[
                    ("driveId", Value::string(&source.drive_id)),
                    ("path", Value::string(&source_path)),
                    ("toDriveId", Value::string(&target.drive_id)),
                    ("toPath", Value::string(&target_parent)),
                ],
            )
            .map_err(refused)?;
            let row = crate::dav::maps(out.get("movedList"))
                .into_iter()
                .next()
                .unwrap_or_default();
            if crate::dav::text_of(&row, "outcome") == "failed" {
                return Err(refused(crate::dav::text_of(&row, "error")));
            }
            placed = crate::dav::text_of(&row, "newPath");
            if placed.is_empty() {
                placed = if target_parent.is_empty() {
                    source_name.clone()
                } else {
                    format!("{target_parent}/{source_name}")
                };
            }
        }
        if source_name != target_name {
            crate::dav::call(
                &format!("{FILES}.rename#Resource"),
                &[
                    ("driveId", Value::string(&target.drive_id)),
                    ("path", Value::string(&placed)),
                    ("newName", Value::string(&target_name)),
                ],
            )
            .map_err(refused)?;
        }
        Ok(())
    }
}
