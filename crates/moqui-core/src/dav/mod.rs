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

//! The DAV server under `/dav/`: WebDAV (RFC 4918, class 1 and 2) over each drive a user may open, and CalDAV (RFC 4791)
//! and CardDAV (RFC 6352) over the calendars and address books an application serves through the adapter services the
//! conf's `dav-server` names. The host (moqui-host `dav_http.rs`) owns the socket, reads the request and calls
//! [`serve`] on a request worker, in a fresh execution context. Guide: `docs/Sync.md`.
//!
//! | Path | |
//! |---|---|
//! | `/dav/` | the root: its `current-user-principal` starts a client's discovery (`/.well-known/caldav` and `carddav` lead here) |
//! | `/dav/principals/<username>/` | the user: `calendar-home-set`, `addressbook-home-set`, `calendar-user-address-set` |
//! | `/dav/files/<username>/<drive>/<path>` | the drives, each by its name ([`files`]) |
//! | `/dav/calendars/<username>/<calendarId>/<name>.ics` | the calendars ([`pim`]) |
//! | `/dav/addressbooks/<username>/<addressBookId>/<name>.vcf` | the address books ([`pim`]) |
//!
//! **Logging in.** HTTP Basic with the username and an app password (`UserAppPassword`, made on the user's Sync settings),
//! on every request; with `dav-server` `account-password="true"` the account password too, for an account with no second
//! factor. Credentials are taken only over HTTPS (the listener, or `X-Forwarded-Proto` from the trusted proxy), from
//! loopback, or with `dav-server` `allow-http="true"`. A path under another user's name is not found. Each request then
//! runs as the user with authorization on, and every read and write goes through the services that check access
//! (`FileServices`, and the adapter's).

pub mod files;
pub(crate) mod locks;
pub(crate) mod pim;
#[cfg(test)]
mod tests;
pub(crate) mod xml;

use std::collections::BTreeMap;

use moqui_lib::object::{is_empty, to_plain_string, Value};

use crate::context::{ExecutionContext, ExecutionContextFactory};
use xml::{Element, CALDAV, CARDDAV, DAV};

/// The path prefix the server is mounted at.
pub const BASE: &str = "/dav";

/// One request, as the host read it.
#[derive(Clone, Debug, Default)]
pub struct DavRequest {
    pub method: String,
    /// The request path as sent, percent-encoded, from `/`.
    pub path: String,
    /// Header names in lower case; a repeated header joined with commas.
    pub headers: Vec<(String, String)>,
    pub body: Vec<u8>,
    /// Credentials may be taken: HTTPS, a trusted proxy's HTTPS, loopback, or `dav-server` `allow-http`.
    pub secure: bool,
    pub client_ip: String,
}

impl DavRequest {
    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(key, _)| key.eq_ignore_ascii_case(name))
            .map(|(_, value)| value.as_str())
    }
}

/// The answer.
#[derive(Clone, Debug, Default)]
pub struct DavResponse {
    pub status: u16,
    pub headers: Vec<(String, String)>,
    pub body: Vec<u8>,
}

impl DavResponse {
    pub(crate) fn status(status: u16) -> Self {
        Self {
            status,
            ..Self::default()
        }
    }

    pub(crate) fn text(status: u16, text: &str) -> Self {
        Self {
            status,
            headers: vec![("Content-Type".into(), "text/plain; charset=utf-8".into())],
            body: text.as_bytes().to_vec(),
        }
    }

    pub(crate) fn xml(status: u16, body: String) -> Self {
        Self {
            status,
            headers: vec![("Content-Type".into(), "application/xml; charset=utf-8".into())],
            body: body.into_bytes(),
        }
    }

    pub(crate) fn with_header(mut self, name: &str, value: impl Into<String>) -> Self {
        self.headers.push((name.to_string(), value.into()));
        self
    }

    /// A precondition or postcondition error (RFC 4918 §16): `<D:error>` with the condition element in it.
    pub(crate) fn condition(status: u16, ns: &str, name: &str) -> Self {
        Self::xml(
            status,
            format!(
                r#"<?xml version="1.0" encoding="utf-8"?><D:error {}>{}</D:error>"#,
                xml::ROOT_NAMESPACES,
                xml::empty_element(ns, name)
            ),
        )
    }
}

/// What a request reaches below the base: the logged-in user and the decoded path segments after the area name.
pub(crate) struct Context<'a> {
    pub request: &'a DavRequest,
    pub username: String,
    pub user_id: String,
    /// The decoded segments after `/dav/<area>/<username>/`.
    pub rest: Vec<String>,
    /// The request path ended with a slash.
    pub trailing_slash: bool,
    pub body: Option<Element>,
}

/// Serve one request. The caller runs it in a fresh execution context on a request worker.
pub fn serve(factory: &ExecutionContextFactory, request: &DavRequest) -> DavResponse {
    let path = request.path.split('?').next().unwrap_or_default();
    if path == "/.well-known/caldav" || path == "/.well-known/carddav" {
        return DavResponse::status(301).with_header("Location", format!("{BASE}/"));
    }
    let Some(rest) = path
        .strip_prefix(BASE)
        .filter(|rest| rest.is_empty() || rest.starts_with('/'))
    else {
        return DavResponse::status(404);
    };
    let method = request.method.to_ascii_uppercase();
    if method == "OPTIONS" {
        return options();
    }
    if !factory.conf_flag("dav-server", "enabled", true) {
        return DavResponse::text(404, "the DAV server is off");
    }
    match login(factory, request) {
        Ok(true) => {}
        Ok(false) => return unauthorized("log in with your username and an app password"),
        Err(response) => return response,
    }
    let username = crate::user::current_username().ok().flatten().unwrap_or_default();
    let user_id = crate::user::current_user_id().ok().flatten().unwrap_or_default();
    let trailing_slash = rest.ends_with('/');
    let segments: Vec<String> = rest
        .split('/')
        .filter(|part| !part.is_empty())
        .map(|part| decode_segment(part))
        .collect();
    if segments
        .iter()
        .any(|part| part == "." || part == ".." || part.contains('/'))
    {
        return DavResponse::status(400);
    }
    let body = match xml::parse(&request.body) {
        Ok(body) => body,
        // a PUT's body is the file, not XML
        Err(_) if method == "PUT" => None,
        Err(err) => return DavResponse::text(400, &err),
    };
    let Some(area) = segments.first() else {
        return root(request, &method, &username, body.as_ref());
    };
    // every area is under the user's own name; another user's is not found
    let named = segments.get(1).cloned().unwrap_or_default();
    if segments.len() >= 2 && !named.eq_ignore_ascii_case(&username) {
        return DavResponse::status(404);
    }
    let context = Context {
        request,
        username: username.clone(),
        user_id,
        rest: segments.iter().skip(2).cloned().collect(),
        trailing_slash,
        body,
    };
    match area.as_str() {
        "principals" if segments.len() == 1 => collection_of_principals(&context, &method),
        "principals" if segments.len() == 2 => principal(factory, &context, &method),
        "files" if segments.len() == 1 => area_root(&context, &method, "files"),
        "files" => files::handle(&context, &method),
        "calendars" if segments.len() == 1 => area_root(&context, &method, "calendars"),
        "calendars" => pim::handle(factory, &context, &method, pim::Kind::Calendar),
        "addressbooks" if segments.len() == 1 => area_root(&context, &method, "addressbooks"),
        "addressbooks" => pim::handle(factory, &context, &method, pim::Kind::Contacts),
        _ => DavResponse::status(404),
    }
}

fn options() -> DavResponse {
    DavResponse::status(200)
        .with_header("DAV", "1, 2, 3, calendar-access, addressbook, extended-mkcol")
        .with_header(
            "Allow",
            "OPTIONS, GET, HEAD, PUT, DELETE, PROPFIND, PROPPATCH, MKCOL, COPY, MOVE, LOCK, UNLOCK, REPORT",
        )
        .with_header("MS-Author-Via", "DAV")
}

fn unauthorized(text: &str) -> DavResponse {
    DavResponse::text(401, text).with_header("WWW-Authenticate", r#"Basic realm="Moqui", charset="UTF-8""#)
}

/// Log the request's user in: false when it carries no credentials, the refusal when they are not taken.
fn login(factory: &ExecutionContextFactory, request: &DavRequest) -> Result<bool, DavResponse> {
    let Some((username, password)) = request.header("authorization").and_then(crate::web::basic_credentials) else {
        return Ok(false);
    };
    if !(request.secure || factory.conf_flag("dav-server", "allow-http", false)) {
        return Err(DavResponse::text(
            403,
            "credentials over plain HTTP are refused; use HTTPS",
        ));
    }
    let account_password = factory.conf_flag("dav-server", "account-password", false);
    let logged_in = crate::user::with_client_ip(&request.client_ip, || {
        if crate::user::looks_like_app_password(&password) || !account_password {
            if crate::user::login_app_password(&username, &password).is_ok() {
                return true;
            }
            if !account_password {
                return false;
            }
            // not an app password: only the account password below counts as a failure
            crate::web::take_auth_failure();
        }
        crate::user::login_user(&username, &password).is_ok()
            && crate::user::current_user_id()
                .ok()
                .flatten()
                .is_some_and(|id| !id.is_empty())
    });
    if logged_in {
        Ok(true)
    } else {
        Err(unauthorized("the username or app password is not valid"))
    }
}

// ---------- the root and the principal ----------

fn root(request: &DavRequest, method: &str, username: &str, body: Option<&Element>) -> DavResponse {
    match method {
        "PROPFIND" => {
            let wanted = PropRequest::from_propfind(body);
            let mut resources = vec![Resource::collection(format!("{BASE}/"), "Moqui")];
            if depth(request) > 0 {
                for (area, title) in [
                    ("principals", "Principals"),
                    ("files", "Files"),
                    ("calendars", "Calendars"),
                    ("addressbooks", "Address books"),
                ] {
                    resources.push(Resource::collection(format!("{BASE}/{area}/"), title));
                }
            }
            for resource in &mut resources {
                resource.add_user_props(username);
            }
            multistatus(&resources, &wanted)
        }
        "GET" | "HEAD" => DavResponse::text(
            200,
            "Moqui DAV server: add an account in your calendar, contacts or files app with this address",
        ),
        _ => DavResponse::status(405),
    }
}

/// `/dav/files/`, `/dav/calendars/` and `/dav/addressbooks/` hold only the user's own home.
fn area_root(context: &Context<'_>, method: &str, area: &str) -> DavResponse {
    if method != "PROPFIND" {
        return DavResponse::status(405);
    }
    let wanted = PropRequest::from_propfind(context.body.as_ref());
    let mut resources = vec![Resource::collection(format!("{BASE}/{area}/"), area)];
    if depth(context.request) > 0 {
        resources.push(Resource::collection(
            format!("{BASE}/{area}/{}/", encode(&context.username)),
            &context.username,
        ));
    }
    for resource in &mut resources {
        resource.add_user_props(&context.username);
    }
    multistatus(&resources, &wanted)
}

fn collection_of_principals(context: &Context<'_>, method: &str) -> DavResponse {
    area_root(context, method, "principals")
}

fn principal(factory: &ExecutionContextFactory, context: &Context<'_>, method: &str) -> DavResponse {
    match method {
        "PROPFIND" => {}
        "REPORT" => return principal_report(factory, context),
        _ => return DavResponse::status(405),
    }
    let wanted = PropRequest::from_propfind(context.body.as_ref());
    multistatus(&[principal_resource(factory, context)], &wanted)
}

fn principal_resource(factory: &ExecutionContextFactory, context: &Context<'_>) -> Resource {
    let user = encode(&context.username);
    let account = user_account(&context.user_id);
    let full_name = account.0.unwrap_or_else(|| context.username.clone());
    let mut resource = Resource::new(format!("{BASE}/principals/{user}/"));
    resource.prop(DAV, "resourcetype", "<D:collection/><D:principal/>");
    resource.prop(DAV, "displayname", &xml::escape(&full_name));
    resource.prop(
        DAV,
        "principal-URL",
        &href_element(&format!("{BASE}/principals/{user}/")),
    );
    resource.prop(
        DAV,
        "principal-collection-set",
        &href_element(&format!("{BASE}/principals/")),
    );
    resource.add_user_props(&context.username);
    if pim::Kind::Calendar.services(factory).is_some() {
        resource.prop(
            CALDAV,
            "calendar-home-set",
            &href_element(&format!("{BASE}/calendars/{user}/")),
        );
        let mut addresses = href_element(&format!("{BASE}/principals/{user}/"));
        if let Some(email) = account.1 {
            addresses.push_str(&href_element(&format!("mailto:{}", email)));
        }
        resource.prop(CALDAV, "calendar-user-address-set", &addresses);
    }
    if pim::Kind::Contacts.services(factory).is_some() {
        resource.prop(
            CARDDAV,
            "addressbook-home-set",
            &href_element(&format!("{BASE}/addressbooks/{user}/")),
        );
    }
    resource
}

/// `principal-search-property-set` and `principal-property-search` find only the user, so a client that searches for
/// the user's own principal (iOS does) finds it.
fn principal_report(factory: &ExecutionContextFactory, context: &Context<'_>) -> DavResponse {
    let Some(body) = context.body.as_ref() else {
        return DavResponse::status(400);
    };
    if body.is(DAV, "principal-search-property-set") {
        return DavResponse::xml(
            200,
            format!(
                r#"<?xml version="1.0" encoding="utf-8"?><D:principal-search-property-set {}><D:principal-search-property><D:prop><D:displayname/></D:prop><D:description>Name</D:description></D:principal-search-property></D:principal-search-property-set>"#,
                xml::ROOT_NAMESPACES
            ),
        );
    }
    if body.is(DAV, "principal-property-search") || body.is(DAV, "expand-property") {
        let wanted = PropRequest::from_parent(body);
        return multistatus(&[principal_resource(factory, context)], &wanted);
    }
    DavResponse::condition(403, DAV, "supported-report")
}

/// The user's full name and email address.
fn user_account(user_id: &str) -> (Option<String>, Option<String>) {
    let row = ExecutionContext::with_current(|ec| {
        let _authz = ec.disable_authz();
        ec.factory()
            .entity()?
            .find("moqui.security.UserAccount")
            .ok()?
            .eq("userId", Value::string(user_id))
            .one()
            .ok()
            .flatten()
    })
    .flatten();
    let field = |name: &str| {
        row.as_ref()
            .and_then(|row| row.get(name).cloned())
            .filter(|value| !is_empty(value))
            .map(|value| to_plain_string(&value))
    };
    (field("userFullName"), field("emailAddress"))
}

// ---------- properties and multistatus ----------

/// One property of a resource: its value as XML, and whether `allprop` includes it.
pub(crate) struct Prop {
    pub ns: &'static str,
    pub name: &'static str,
    pub value: String,
    pub in_allprop: bool,
}

/// A resource in a multistatus: its href and properties.
pub(crate) struct Resource {
    pub href: String,
    pub props: Vec<Prop>,
}

impl Resource {
    pub(crate) fn new(href: String) -> Self {
        Self {
            href,
            props: Vec::new(),
        }
    }

    pub(crate) fn collection(href: String, name: &str) -> Self {
        let mut resource = Self::new(href);
        resource.prop(DAV, "resourcetype", "<D:collection/>");
        resource.prop(DAV, "displayname", &xml::escape(name));
        resource
    }

    pub(crate) fn prop(&mut self, ns: &'static str, name: &'static str, value: &str) {
        self.props.retain(|prop| !(prop.ns == ns && prop.name == name));
        self.props.push(Prop {
            ns,
            name,
            value: value.to_string(),
            in_allprop: true,
        });
    }

    /// A property `allprop` leaves out, as the data of a calendar object.
    pub(crate) fn heavy_prop(&mut self, ns: &'static str, name: &'static str, value: &str) {
        self.prop(ns, name, value);
        if let Some(prop) = self.props.last_mut() {
            prop.in_allprop = false;
        }
    }

    /// `current-user-principal` and `owner`, which every resource answers.
    pub(crate) fn add_user_props(&mut self, username: &str) {
        let principal = href_element(&format!("{BASE}/principals/{}/", encode(username)));
        self.prop(DAV, "current-user-principal", &principal);
        if !self.props.iter().any(|prop| prop.name == "owner") {
            self.prop(DAV, "owner", &principal);
        }
    }

    /// `current-user-privilege-set`: read, and the write privileges when `writable`.
    pub(crate) fn privileges(&mut self, writable: bool) {
        let mut set = String::from(
            "<D:privilege><D:read/></D:privilege><D:privilege><D:read-current-user-privilege-set/></D:privilege>",
        );
        if writable {
            for name in ["write", "write-content", "write-properties", "bind", "unbind"] {
                set.push_str(&format!("<D:privilege><D:{name}/></D:privilege>"));
            }
        }
        self.prop(DAV, "current-user-privilege-set", &set);
    }
}

/// What a PROPFIND or REPORT asks for.
pub(crate) enum PropRequest {
    All,
    Names,
    Some(Vec<(String, String)>),
}

impl PropRequest {
    pub(crate) fn from_propfind(body: Option<&Element>) -> Self {
        match body {
            None => Self::All,
            Some(body) => Self::from_parent(body),
        }
    }

    /// From the `prop`, `propname` or `allprop` child of a PROPFIND or REPORT body.
    pub(crate) fn from_parent(body: &Element) -> Self {
        if body.child(DAV, "propname").is_some() {
            return Self::Names;
        }
        match body.child(DAV, "prop") {
            Some(prop) => Self::Some(
                prop.children
                    .iter()
                    .map(|child| (child.ns.clone(), child.name.clone()))
                    .collect(),
            ),
            None => Self::All,
        }
    }

    pub(crate) fn wants(&self, ns: &str, name: &str) -> bool {
        match self {
            Self::All | Self::Names => true,
            Self::Some(list) => list.iter().any(|(known_ns, known)| known_ns == ns && known == name),
        }
    }
}

/// One `<D:response>` of a resource's properties: those it has in a 200 propstat, the asked ones it has not in a 404.
pub(crate) fn prop_response(resource: &Resource, wanted: &PropRequest) -> String {
    let mut found = String::new();
    let mut missing = String::new();
    match wanted {
        PropRequest::All => {
            for prop in resource.props.iter().filter(|prop| prop.in_allprop) {
                found.push_str(&xml::element(prop.ns, prop.name, &prop.value));
            }
        }
        PropRequest::Names => {
            for prop in &resource.props {
                found.push_str(&xml::empty_element(prop.ns, prop.name));
            }
        }
        PropRequest::Some(list) => {
            for (ns, name) in list {
                match resource.props.iter().find(|prop| prop.ns == ns && prop.name == name) {
                    Some(prop) => found.push_str(&xml::element(prop.ns, prop.name, &prop.value)),
                    None => missing.push_str(&xml::empty_element(ns, name)),
                }
            }
        }
    }
    let mut out = format!("<D:response><D:href>{}</D:href>", xml::escape(&resource.href));
    if !found.is_empty() || missing.is_empty() {
        out.push_str(&format!(
            "<D:propstat><D:prop>{found}</D:prop><D:status>HTTP/1.1 200 OK</D:status></D:propstat>"
        ));
    }
    if !missing.is_empty() {
        out.push_str(&format!(
            "<D:propstat><D:prop>{missing}</D:prop><D:status>HTTP/1.1 404 Not Found</D:status></D:propstat>"
        ));
    }
    out.push_str("</D:response>");
    out
}

/// A `<D:response>` with only a status, for an href that is not there.
pub(crate) fn status_response(href: &str, status: u16) -> String {
    format!(
        "<D:response><D:href>{}</D:href><D:status>HTTP/1.1 {status} {}</D:status></D:response>",
        xml::escape(href),
        reason(status)
    )
}

pub(crate) fn multistatus(resources: &[Resource], wanted: &PropRequest) -> DavResponse {
    let responses: Vec<String> = resources
        .iter()
        .map(|resource| prop_response(resource, wanted))
        .collect();
    multistatus_of(&responses)
}

pub(crate) fn multistatus_of(responses: &[String]) -> DavResponse {
    let mut body = format!(
        r#"<?xml version="1.0" encoding="utf-8"?><D:multistatus {}>"#,
        xml::ROOT_NAMESPACES
    );
    for response in responses {
        body.push_str(response);
    }
    body.push_str("</D:multistatus>");
    DavResponse::xml(207, body)
}

/// PROPPATCH: dead properties are not kept; each is answered as set, so a client that writes its own (Finder, Windows)
/// goes on, except the server's own, which are protected.
pub(crate) fn proppatch(href: &str, body: Option<&Element>) -> DavResponse {
    let Some(body) = body.filter(|body| body.is(DAV, "propertyupdate")) else {
        return DavResponse::status(400);
    };
    let mut accepted = String::new();
    let mut refused = String::new();
    for change in body
        .children
        .iter()
        .filter(|child| child.is(DAV, "set") || child.is(DAV, "remove"))
    {
        for prop in change.children.iter().filter(|child| child.is(DAV, "prop")) {
            for item in &prop.children {
                let protected = item.ns == DAV
                    && matches!(
                        item.name.as_str(),
                        "resourcetype"
                            | "getetag"
                            | "getcontentlength"
                            | "lockdiscovery"
                            | "supportedlock"
                            | "current-user-principal"
                    );
                let element = xml::empty_element(&item.ns, &item.name);
                if protected {
                    refused.push_str(&element);
                } else {
                    accepted.push_str(&element);
                }
            }
        }
    }
    let mut response = format!("<D:response><D:href>{}</D:href>", xml::escape(href));
    if !refused.is_empty() {
        response.push_str(&format!(
            "<D:propstat><D:prop>{refused}</D:prop><D:status>HTTP/1.1 403 Forbidden</D:status></D:propstat>"
        ));
        if !accepted.is_empty() {
            response.push_str(&format!(
                "<D:propstat><D:prop>{accepted}</D:prop><D:status>HTTP/1.1 424 Failed Dependency</D:status></D:propstat>"
            ));
        }
    } else {
        response.push_str(&format!(
            "<D:propstat><D:prop>{accepted}</D:prop><D:status>HTTP/1.1 200 OK</D:status></D:propstat>"
        ));
    }
    response.push_str("</D:response>");
    multistatus_of(&[response])
}

// ---------- helpers ----------

/// The address devices reach this server at: `dav-server` `public-url`, else the `dav_public_url` property, else the
/// scheme, host and port of the web request in progress (the address the user reached the screen at), else the
/// webapp's host and port properties (as the git clone addresses).
pub fn public_base_url(factory: &ExecutionContextFactory) -> String {
    let property = |name: &str| moqui_lib::env::get_prop_or_env(name).trim().to_string();
    let mut configured = factory.conf_setting("dav-server", "public-url").trim().to_string();
    if configured.is_empty() {
        configured = property("dav_public_url");
    }
    if !configured.is_empty() {
        return configured.trim_end_matches('/').to_string();
    }
    if let Some(url) = crate::web::request_url() {
        if let Some((scheme, rest)) = url.split_once("://") {
            let authority = rest.split('/').next().unwrap_or_default();
            if !authority.is_empty() {
                return format!("{scheme}://{authority}");
            }
        }
    }
    let host = Some(property("webapp_http_host"))
        .filter(|host| !host.is_empty())
        .unwrap_or_else(|| "localhost".to_string());
    let (scheme, port, default) = if property("webapp_https_enabled") == "true" {
        ("https", property("webapp_https_port"), "443")
    } else {
        ("http", property("webapp_http_port"), "80")
    };
    if port.is_empty() || port == default {
        format!("{scheme}://{host}")
    } else {
        format!("{scheme}://{host}:{port}")
    }
}

/// The `Depth` header: 0, 1, or 1 for `infinity` (a deep listing is not served).
pub(crate) fn depth(request: &DavRequest) -> u8 {
    match request.header("depth").map(str::trim) {
        Some("0") => 0,
        _ => 1,
    }
}

pub(crate) fn href_element(href: &str) -> String {
    format!("<D:href>{}</D:href>", xml::escape(href))
}

/// A path segment of a request, its `%XX` escapes decoded (a `+` stays a plus in a path).
pub(crate) fn decode_segment(segment: &str) -> String {
    crate::web::percent_decode(&segment.replace('+', "%2B"))
}

/// A path segment percent-encoded for an href.
pub fn encode(segment: &str) -> String {
    let mut out = String::with_capacity(segment.len());
    for byte in segment.as_bytes() {
        match byte {
            b'A'..=b'Z'
            | b'a'..=b'z'
            | b'0'..=b'9'
            | b'-'
            | b'_'
            | b'.'
            | b'~'
            | b'@'
            | b'!'
            | b'$'
            | b'&'
            | b'\''
            | b'('
            | b')'
            | b'*'
            | b'+'
            | b','
            | b';'
            | b'=' => {
                out.push(char::from(*byte));
            }
            _ => out.push_str(&format!("%{byte:02X}")),
        }
    }
    out
}

/// An HTTP date (RFC 7231 IMF-fixdate) of epoch milliseconds.
pub(crate) fn http_date(millis: i64) -> String {
    jiff::Timestamp::from_millisecond(millis)
        .map(|stamp| stamp.strftime("%a, %d %b %Y %H:%M:%S GMT").to_string())
        .unwrap_or_default()
}

/// A strong entity tag of some text.
pub(crate) fn etag_of(text: &str) -> String {
    use sha2::Digest;
    let digest = sha2::Sha256::digest(text.as_bytes());
    let hex: String = digest.iter().take(12).map(|byte| format!("{byte:02x}")).collect();
    format!("\"{hex}\"")
}

/// The tags of an `If-Match` or `If-None-Match` header; `*` is `["*"]`.
pub(crate) fn tag_list(header: Option<&str>) -> Vec<String> {
    header
        .map(|text| {
            text.split(',')
                .map(|tag| tag.trim().trim_start_matches("W/").to_string())
                .filter(|tag| !tag.is_empty())
                .collect()
        })
        .unwrap_or_default()
}

/// The reason phrase of a status.
pub(crate) fn reason(status: u16) -> &'static str {
    match status {
        200 => "OK",
        201 => "Created",
        204 => "No Content",
        207 => "Multi-Status",
        403 => "Forbidden",
        404 => "Not Found",
        409 => "Conflict",
        412 => "Precondition Failed",
        423 => "Locked",
        424 => "Failed Dependency",
        507 => "Insufficient Storage",
        _ => "Error",
    }
}

/// Call a service as the user; the errors it added are the `Err`, and are cleared so the next call runs.
pub(crate) fn call(name: &str, parameters: &[(&str, Value)]) -> Result<BTreeMap<String, Value>, String> {
    let parameters: BTreeMap<String, Value> = parameters
        .iter()
        .map(|(key, value)| ((*key).to_string(), value.clone()))
        .collect();
    let result = crate::service::call::sync_checked(name, &parameters, &crate::service::call::CallOptions::default());
    ExecutionContext::with_current(|ec| ec.message().clear_errors());
    result.map_err(|err| err.message().to_string())
}

/// The HTTP status of a service error, by its text.
pub(crate) fn status_of_error(error: &str) -> u16 {
    let lower = error.to_ascii_lowercase();
    if lower.contains("not found") || lower.contains("no such") {
        404
    } else if lower.contains("quota") {
        507
    } else if lower.contains("already") || lower.contains("exists") {
        412
    } else if lower.contains("may not")
        || lower.contains("permission")
        || lower.contains("read only")
        || lower.contains("not permitted")
    {
        403
    } else {
        409
    }
}

pub(crate) fn text_of(map: &indexmap::IndexMap<String, Value>, key: &str) -> String {
    map.get(key)
        .filter(|value| !is_empty(value))
        .map(to_plain_string)
        .unwrap_or_default()
}

pub(crate) fn flag_of(map: &indexmap::IndexMap<String, Value>, key: &str) -> bool {
    map.get(key)
        .is_some_and(|value| matches!(value, Value::Bool(true)) || to_plain_string(value) == "true")
}

pub(crate) fn millis_of(map: &indexmap::IndexMap<String, Value>, key: &str) -> Option<i64> {
    map.get(key).and_then(crate::user::value_millis)
}

/// The maps of a list a service returned.
pub(crate) fn maps(value: Option<&Value>) -> Vec<indexmap::IndexMap<String, Value>> {
    match value {
        Some(Value::List(list)) => list
            .iter()
            .filter_map(|item| match item {
                Value::Map(map) => Some(map.clone()),
                Value::Object(object) => object.downcast::<crate::entity::EntityValue>().map(|row| {
                    row.fields()
                        .iter()
                        .map(|(key, value)| (key.to_string(), value.clone()))
                        .collect()
                }),
                _ => None,
            })
            .collect(),
        _ => Vec::new(),
    }
}

/// The path of a `Destination` header (an absolute URL or a path), percent-encoded as sent.
pub(crate) fn destination_path(request: &DavRequest) -> Option<String> {
    let destination = request.header("destination")?.trim();
    let path = match destination.find("://") {
        Some(scheme) => {
            let after = &destination[scheme + 3..];
            after.find('/').map_or("/", |slash| &after[slash..])
        }
        None => destination,
    };
    Some(path.split('?').next().unwrap_or_default().to_string())
}

/// The `Overwrite` header: true unless it is `F`.
pub(crate) fn overwrite(request: &DavRequest) -> bool {
    !request
        .header("overwrite")
        .is_some_and(|value| value.trim().eq_ignore_ascii_case("F"))
}
