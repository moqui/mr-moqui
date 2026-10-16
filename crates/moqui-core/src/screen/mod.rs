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

//! The screen facade: definitions, the URL walk, and rendering.
//!
//! `definition` parses a screen, `cache` holds parsed screens and the `screen-facade` conf,
//! `url` resolves a path against the tree, and `render` runs actions and sends the response.
//! `dispatch` holds the Rust bodies for the `rest.xml` transitions. Widgets are slice 2.

pub(crate) mod aggregate;
mod cache;
mod definition;
mod dispatch;
pub(crate) mod form;
pub(crate) mod generate;
pub mod info;
pub(crate) mod menu;
mod pdf;
mod render;
pub(crate) mod saved_finds;
mod schema;
mod screen_test;
pub(crate) mod sri;
pub mod static_ast;
pub mod static_files;
pub mod statics;
pub(crate) mod tree;
mod url;
pub(crate) mod urls;
pub mod vue_check;
pub(crate) mod xlsx;

use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::Instant;

use moqui_lib::object::Value;

use crate::context::{Error, ErrorKind, ExecutionContextFactory};
use crate::user::{self, apply_header_keys, login_user};
use crate::web::{self, WebRequest, WebResponse};

pub use cache::{canonical_render_mode, ConfScreen, ScreenConf, ScreenFacade, TextOutput};
pub use definition::{
    Actions, Authentication, Condition, FormDef, Implicit, ParameterDef, ResponseDef, ResponseKind, ScreenDef,
    SectionDef, SectionKind, Subscreens, SubscreensItem, TransitionDef, TreeDef, WebSettings, XmlFragment,
};
#[cfg(test)]
pub(crate) use pdf::pdf_test_lock;
pub use pdf::{clear_pdf_engine, install_pdf_engine, install_pdf_unavailable, PdfEngine, PdfRenderer, PDF_UNAVAILABLE};
pub use render::{
    make_render, save_screen_last_info, send_redirect_and_stop_render, with_current_render, RenderOutput, RenderState,
    ScreenRender,
};
pub use screen_test::{ScreenTest, ScreenTestRender, ScreenTestStats, TestRender};
pub use sri::encode_html;
pub use url::{parse_last_standalone, resolve_screen_path, ScreenUrlInfo};

const ROOT_SCREEN: &str = "component://webroot/screen/webroot.xml";

/// Where a path landed.
#[derive(Clone, Debug)]
pub struct ScreenHit {
    pub transition: Option<String>,
    pub extra: Vec<String>,
    /// Screen file names from the root, joined with `/`.
    pub screen: String,
}

/// Handle one request from `component://webroot/screen/webroot.xml`.
pub fn serve(factory: &Arc<ExecutionContextFactory>, request: WebRequest) -> WebResponse {
    serve_at(factory, request, ROOT_SCREEN)
}

/// Handle one request whose root screen is `root`.
pub fn serve_at(factory: &Arc<ExecutionContextFactory>, request: WebRequest, root: &str) -> WebResponse {
    if request.body.len() > web::body_limit(&request) {
        return web::early_error(
            &request,
            413,
            &format!("request body is larger than {} bytes", web::body_limit(&request)),
        );
    }
    if request.method.eq_ignore_ascii_case("options") {
        return web::finish_response(&request, WebResponse::new(204, Vec::new()), true);
    }
    let parsed = match web::parse_body(&request) {
        Ok(parsed) => parsed,
        Err(web::BodyError::TooLarge) => return web::early_error(&request, 413, "request body is too large"),
        Err(web::BodyError::Multipart(message)) => return web::early_error(&request, 400, &message),
        Err(web::BodyError::Executable(name)) => {
            return web::early_error(&request, 415, &format!("Executable file {name} upload not allowed"))
        }
    };
    // A request-pool worker already entered a fresh context for this job. Direct callers get one here.
    let context = (!crate::context::ExecutionContext::is_entered()).then(|| factory.new_context());
    let _entered = context.as_ref().map(|context| context.enter());
    let mut parameters = parsed.parameters;
    parameters.extend(parsed.multipart.clone());
    let response = match handle(
        factory,
        &request,
        root,
        &parameters,
        &parsed.multipart,
        parsed.json_list.as_deref(),
    ) {
        Ok(response) => response,
        Err(err) => web::json_error(500, &web::error_text(500, &err)),
    };
    web::finish_response(&request, response, false)
}

fn handle(
    factory: &Arc<ExecutionContextFactory>,
    request: &WebRequest,
    root: &str,
    parameters: &BTreeMap<String, Value>,
    multipart: &BTreeMap<String, Value>,
    json_list: Option<&[Value]>,
) -> Result<WebResponse, Error> {
    let session_cookie = web::session_cookie_of(request);
    let visitor_cookie = web::visitor_cookie_of(request);
    let presented = session_cookie
        .as_deref()
        .map(user::verify_session_cookie)
        .transpose()?
        .flatten();
    let guard = user::bind(
        factory,
        session_cookie.as_deref(),
        visitor_cookie.as_deref(),
        request.https,
    )?;
    let token_before = peek_token();
    if let Err(err) = user::session::remember_client_ip(&web::client_ip(request)) {
        return Ok(web::json_error(500, &web::error_text(500, &err)));
    }
    // What the last response saved for this request sits under the request's own parameters.
    let saved = user::session::take_saved_parameters();
    let merged;
    let parameters = if saved.is_empty() {
        parameters
    } else {
        let mut all: BTreeMap<String, Value> = saved
            .into_iter()
            .map(|(name, value)| (name, Value::String(value)))
            .collect();
        all.extend(parameters.iter().map(|(name, value)| (name.clone(), value.clone())));
        merged = all;
        &merged
    };
    let _web = web::install(parameters.clone());
    web::set_multipart(multipart.clone());
    web::set_host(request.header("host").unwrap_or(""));
    web::set_request_info(request);
    web::set_request_locale(request.header("accept-language"));
    let (login_error, header_authenticated) = authenticate(request, parameters)?;
    // A session that did not come with the request was made for it, and its token was never sent to the client.
    let new_session = presented.as_deref() != Some(guard.session_id().as_str());
    let response = dispatch_request(
        factory,
        request,
        root,
        parameters,
        json_list,
        login_error.as_deref(),
        (header_authenticated, new_session),
    )?;
    let response = attach_session(
        response,
        &guard,
        presented.as_deref(),
        visitor_cookie.as_deref(),
        token_before,
    )?;
    drop(guard);
    Ok(response)
}

fn dispatch_request(
    factory: &Arc<ExecutionContextFactory>,
    request: &WebRequest,
    root: &str,
    parameters: &BTreeMap<String, Value>,
    json_list: Option<&[Value]>,
    login_error: Option<&str>,
    (header_authenticated, new_session): (bool, bool),
) -> Result<WebResponse, Error> {
    let job = render::Job {
        request,
        root,
        path: &request.path,
        method: &request.method,
        parameters,
        json_list,
        login_error,
        header_authenticated,
        render_mode: None,
        enforce_request_checks: true,
        new_session,
        started: Instant::now(),
    };
    let end = EndRequest::armed();
    if let Err(err) = render::run_job(factory, &job) {
        // Only a kind names a client error. A script or load failure is a 500 whatever it says.
        let status = if err.kind() == ErrorKind::General {
            500
        } else {
            web::status_for(&err)
        };
        let message = web::error_text(status, &err);
        if let Some(page) = error_page(factory, request, root, parameters, status, &err, &message) {
            return Ok(page);
        }
        return Ok(web::json_error(status, &message));
    }
    if let Err(err) = end.finish() {
        if !web::output_sent() {
            return Ok(web::json_error(500, &web::error_text(500, &err)));
        }
    }
    let (status, headers, body, sent) = web::take_output();
    Ok(WebResponse {
        status: if sent { status } else { 200 },
        headers,
        body,
    })
}

/// Ends the request's session work ([`user::end_request`]) on every path: [`EndRequest::finish`]
/// hands the caller its error, and a drop without it (a failed request) logs the error.
struct EndRequest(bool);

impl EndRequest {
    fn armed() -> Self {
        Self(true)
    }

    fn finish(mut self) -> Result<(), Error> {
        self.0 = false;
        user::end_request()
    }
}

impl Drop for EndRequest {
    fn drop(&mut self) {
        if self.0 {
            if let Err(err) = user::end_request() {
                tracing::error!("Error ending a failed request's session: {err}");
            }
        }
    }
}

/// The html page for a failure, when the client takes html and the path is not an API path: the `error/*`
/// screen with `errorCode`, `errorType`, `errorMessage`, `errorThrowable` and `showErrorDetail` in its context.
/// None when the page cannot be made, and the caller sends the JSON error. `message` is the client's text of the error,
/// from [`web::error_text`].
fn error_page(
    factory: &Arc<ExecutionContextFactory>,
    request: &WebRequest,
    root: &str,
    parameters: &BTreeMap<String, Value>,
    status: u16,
    err: &Error,
    message: &str,
) -> Option<WebResponse> {
    if !request
        .header("accept")
        .is_some_and(|accept| accept.contains("text/html"))
    {
        return None;
    }
    let first = request.path.trim_start_matches('/').split('/').next().unwrap_or("");
    if ["rest", "rpc", "status", "error", "health", "metrics"].contains(&first) {
        return None;
    }
    let (kind, page) = web::error_info::error_screen(status);
    let _ = web::take_output();
    let production = web::is_production();
    let signed_in = user::current_username().ok().flatten().is_some();
    let mut context = parameters.clone();
    context.insert("errorCode".to_string(), Value::I32(i32::from(status)));
    context.insert("errorType".to_string(), Value::string(kind));
    context.insert("errorMessage".to_string(), Value::string(message));
    context.insert(
        "errorThrowable".to_string(),
        Value::Object(moqui_lib::object::HostObject::new(web::error_info::ErrorInfo::new(
            status,
            err,
            &request.path,
        ))),
    );
    context.insert("showErrorDetail".to_string(), Value::Bool(!production && signed_in));
    let path = format!("/{page}");
    let job = render::Job {
        request,
        root,
        path: &path,
        method: "GET",
        parameters: &context,
        json_list: None,
        login_error: None,
        header_authenticated: true,
        render_mode: Some("html"),
        enforce_request_checks: false,
        new_session: false,
        started: Instant::now(),
    };
    if let Err(failure) = render::run_job(factory, &job) {
        tracing::warn!("could not render the error page {page}: {failure}");
        let _ = web::take_output();
        return None;
    }
    let (_, headers, body, sent) = web::take_output();
    sent.then_some(WebResponse { status, headers, body })
}

fn attach_session(
    mut response: WebResponse,
    guard: &user::SessionGuard,
    presented: Option<&str>,
    visitor_cookie: Option<&str>,
    token_before: Option<String>,
) -> Result<WebResponse, Error> {
    if presented != Some(guard.session_id().as_str()) {
        response
            .headers
            .push(("Set-Cookie".to_string(), guard.session_cookie_header()?));
    }
    if guard.visitor_id().as_deref() != visitor_cookie {
        if let Some(cookie) = guard.visitor_cookie_header() {
            response.headers.push(("Set-Cookie".to_string(), cookie));
        }
    }
    let token_after = user::csrf_token()?;
    if presented != Some(guard.session_id().as_str()) || token_before.as_deref() != Some(token_after.as_str()) {
        response
            .headers
            .push(("moquiSessionToken".to_string(), token_after.clone()));
        response.headers.push(("X-CSRF-Token".to_string(), token_after));
    }
    Ok(response)
}

fn peek_token() -> Option<String> {
    user::session::peek_csrf_token()
}

/// The login error, if any, and whether this request logged a user in from its own credentials
/// (Basic, `api_key`, `login_key`, or `authUsername`). Such a request carries no ambient cookie
/// authority, so it skips the session-token check as it does in the Java `ScreenRenderImpl`.
fn authenticate(request: &WebRequest, parameters: &BTreeMap<String, Value>) -> Result<(Option<String>, bool), Error> {
    let mut login_error = None;
    let mut by_header = false;
    if let Some(header) = request.header("authorization") {
        if let Some((username, password)) = web::basic_credentials(header) {
            // A password login that still needs a second factor leaves nobody logged in.
            match login_user(&username, &password) {
                Ok(()) => by_header = !user::session::session_attribute_flag(user::AUTHC_FACTOR_REQUIRED),
                Err(err) => {
                    let message = err.message().to_string();
                    web::record_login_error(&message);
                    login_error = Some(message);
                }
            }
        }
    }
    if user::current_username()?.is_none() {
        let keyed = request.header("api_key").is_some_and(|text| !text.is_empty())
            || request.header("login_key").is_some_and(|text| !text.is_empty());
        if let Err(err) = apply_header_keys(request.header("api_key"), request.header("login_key")) {
            let message = err.message().to_string();
            web::record_login_error(&message);
            login_error = Some(message);
        } else if keyed && user::current_username()?.is_some() {
            by_header = true;
        }
    }
    if user::current_username()?.is_none() {
        let username = text_of(parameters.get("authUsername"));
        let password = text_of(parameters.get("authPassword"));
        if !username.is_empty() && !password.is_empty() {
            match login_user(&username, &password) {
                Ok(()) => by_header = !user::session::session_attribute_flag(user::AUTHC_FACTOR_REQUIRED),
                Err(err) => {
                    let message = err.message().to_string();
                    web::record_login_error(&message);
                    login_error = Some(message);
                }
            }
        }
    }
    Ok((login_error, by_header))
}

/// What [`check_permission`] found for a request.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PermissionCheck {
    Allowed,
    /// Nobody is logged in: Java's `MoquiAuthFilter` answers 401 with a Basic challenge.
    Unauthenticated,
    /// Logged in without the permission: 403.
    Forbidden,
}

/// Java's `MoquiAuthFilter`, for a path that is not a screen (the `/elastic` and `/kibana` proxies):
/// the session cookie, Basic, `api_key`/`login_key`, or `authUsername`/`authPassword` login a screen
/// request makes, then whether the user has `permission`. The session is not handed back to the client.
///
/// # Errors
///
/// Returns an error when the session or the permission rows cannot be read.
pub fn check_permission(
    factory: &Arc<ExecutionContextFactory>,
    request: &WebRequest,
    permission: &str,
) -> Result<PermissionCheck, Error> {
    let context = (!crate::context::ExecutionContext::is_entered()).then(|| factory.new_context());
    let _entered = context.as_ref().map(|context| context.enter());
    let session_cookie = web::session_cookie_of(request);
    let visitor_cookie = web::visitor_cookie_of(request);
    let guard = user::bind(
        factory,
        session_cookie.as_deref(),
        visitor_cookie.as_deref(),
        request.https,
    )?;
    let parameters: BTreeMap<String, Value> = request
        .query
        .iter()
        .map(|(name, value)| (name.clone(), Value::string(value)))
        .collect();
    let _web = web::install(parameters.clone());
    let end = EndRequest::armed();
    authenticate(request, &parameters)?;
    let check = if user::current_username()?.is_none() {
        PermissionCheck::Unauthenticated
    } else if user::has_permission(permission)? {
        PermissionCheck::Allowed
    } else {
        PermissionCheck::Forbidden
    };
    end.finish()?;
    drop(guard);
    Ok(check)
}

/// Walk `path` from `root` without binding a session.
///
/// # Errors
///
/// Returns `ErrorKind::NotFound` when the path matches no screen or transition.
pub fn locate(factory: &ExecutionContextFactory, root: &str, path: &str) -> Result<ScreenHit, Error> {
    let info = ScreenUrlInfo::parse(factory, root, path, 0)?;
    Ok(ScreenHit {
        transition: info.target_transition_name.clone(),
        extra: info.extra_path.clone(),
        screen: info
            .path_defs
            .iter()
            .map(|def| def.screen_name.as_str())
            .collect::<Vec<_>>()
            .join("/"),
    })
}

fn text_of(value: Option<&Value>) -> String {
    value.map(moqui_lib::object::to_plain_string).unwrap_or_default()
}

#[cfg(test)]
mod tests;
