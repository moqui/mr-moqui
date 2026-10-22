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
pub mod gate;
pub(crate) mod html_text;
pub mod info;
pub(crate) mod menu;
mod pdf;
mod render;
pub(crate) mod saved_finds;
mod schema;
mod screen_test;
pub(crate) mod shell;
pub(crate) mod sri;
pub mod static_ast;
pub mod static_files;
pub mod statics;
pub mod token_route;
pub(crate) mod tree;
mod url;
pub(crate) mod urls;
pub mod vue_check;
#[cfg(feature = "xlsx")]
pub(crate) mod xlsx;
#[cfg(not(feature = "xlsx"))]
#[path = "xlsx_off.rs"]
pub(crate) mod xlsx;

use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::Instant;

use moqui_lib::object::Value;

use crate::context::{Error, ExecutionContextFactory};
use crate::user::{self, apply_header_keys, login_user};
use crate::web::{self, WebRequest, WebResponse};

pub use cache::{canonical_render_mode, ConfScreen, ScreenConf, ScreenFacade, TextOutput};
pub use definition::{
    Actions, Authentication, Condition, FormDef, Implicit, ParameterDef, ResponseDef, ResponseKind, ScreenDef,
    SectionDef, SectionKind, Subscreens, SubscreensItem, TransitionDef, TreeDef, WebSettings, XmlFragment,
};
#[cfg(test)]
pub(crate) use pdf::pdf_test_lock;
pub use pdf::{
    clear_pdf_engine, install_pdf_engine, install_pdf_unavailable, PdfEngine, PdfLibrary, PdfPreview,
    PdfPreviewRequest, PdfRenderer, PDFJS_LOCATION, PDFJS_WORKER_LOCATION, PDF_UNAVAILABLE, PREVIEW_UNSUPPORTED,
};
pub use gate::{
    clear_transition_observer, held_write_hash, install_transition_observer, interactive_request, HeldWrite,
    InteractiveRequest, RenderOptions, RenderOrigin, TransitionGate, TransitionObserver, TransitionOutcome,
};
pub use render::{
    make_render, save_screen_last_info, send_redirect_and_stop_render, with_current_render, RenderOutput, RenderState,
    ScreenRender,
};
pub use screen_test::{ScreenTest, ScreenTestRender, ScreenTestStats, TestRender};
pub use sri::encode_html;
pub use url::{parse_last_standalone, resolve_screen_path, ScreenUrlInfo};

/// The default root screen, `webroot.xml`.
pub const ROOT_SCREEN: &str = "component://webroot/screen/webroot.xml";

/// Where a path landed.
#[derive(Clone, Debug)]
pub struct ScreenHit {
    pub transition: Option<String>,
    pub extra: Vec<String>,
    /// Screen file names from the root, joined with `/`.
    pub screen: String,
    /// The location of the last screen of the path (the one a transition belongs to).
    pub location: String,
}

/// Handle one request. The root screen is the `webroot` webapp's `root-screen` for the Host header.
/// `.*` is the fallback, which is `webroot.xml` in the default conf.
pub fn serve(factory: &Arc<ExecutionContextFactory>, request: WebRequest) -> WebResponse {
    let host = request.header("host").unwrap_or("");
    let root = match info::root_screen_for_host(factory, host, "webroot") {
        Ok(location) => location,
        Err(err) => return web::early_error(factory, &request, 500, err.message()),
    };
    serve_at(factory, request, &root)
}

/// Handle one request whose root screen is `root`.
pub fn serve_at(factory: &Arc<ExecutionContextFactory>, mut request: WebRequest, root: &str) -> WebResponse {
    if request.body.len() > web::body_limit(&request) {
        return web::early_error(
            factory,
            &request,
            413,
            &format!("request body is larger than {} bytes", web::body_limit(&request)),
        );
    }
    if let Some(origin) = web::refused_origin(factory, &request) {
        tracing::warn!("Origin not allowed: {origin} for {} {}", request.method, request.path);
        return web::early_error(factory, &request, 401, "Origin not allowed");
    }
    if request.method.eq_ignore_ascii_case("options") {
        return web::finish_response(factory, &request, WebResponse::new(204, Vec::new()), true);
    }
    let parsed = match web::parse_body(&request) {
        Ok(parsed) => parsed,
        Err(web::BodyError::TooLarge) => return web::early_error(factory, &request, 413, "request body is too large"),
        Err(web::BodyError::Multipart(message)) => return web::early_error(factory, &request, 400, &message),
        Err(web::BodyError::Executable(name)) => {
            return web::early_error(
                factory,
                &request,
                415,
                &format!("Executable file {name} upload not allowed"),
            )
        }
    };
    // A request-pool worker already entered a fresh context for this job. Direct callers get one here.
    let context = (!crate::context::ExecutionContext::is_entered()).then(|| factory.new_context());
    let _entered = context.as_ref().map(|context| context.enter());
    let mut parameters = parsed.parameters;
    // Files fill names the query and the form body left empty. `multiPartParameters` stays the file map.
    web::overlay_multipart(&mut parameters, &parsed.multipart);
    // The raw body, for a web script's request (a proxy sends it on), without a copy.
    let body = bytes::Bytes::from(std::mem::take(&mut request.body));
    let request = Arc::new(request);
    let response = match handle(
        factory,
        &request,
        body,
        root,
        &parameters,
        &parsed.multipart,
        parsed.json_list.as_deref(),
    ) {
        Ok(response) => response,
        Err(err) => web::json_error_of(&err),
    };
    web::finish_response(factory, &request, response, false)
}

fn handle(
    factory: &Arc<ExecutionContextFactory>,
    request: &Arc<WebRequest>,
    body: bytes::Bytes,
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
    // a visit this request starts (at the bind, or at a login on a session that put it off) records the request
    let _visit_request = user::session::VisitRequestGuard::set(web::visit_request(request));
    let guard = user::bind(
        factory,
        session_cookie.as_deref(),
        visitor_cookie.as_deref(),
        request.https,
    )?;
    let token_before = peek_token();
    if let Err(err) = user::session::remember_client_ip(&web::client_ip(request)) {
        let mut response = web::json_error_of(&err);
        stamp_http_log(&mut response);
        return Ok(response);
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
    web::set_request(Arc::clone(request), body);
    let (login_error, header_authenticated) = authenticate(request, parameters)?;
    // A session that did not come with the request was made for it, and its token was never sent to the client.
    let new_session = presented.as_deref() != Some(guard.session_id().as_str());
    let scripts = factory.webapp_scripts();
    // As Java's `ExecutionContextImpl.destroy`, the after-request actions run however the request ended, and before
    // the session work ends.
    let end = EndRequest::armed();
    let outcome = run_request(
        factory,
        &scripts,
        request,
        root,
        parameters,
        json_list,
        login_error,
        (header_authenticated, new_session),
    );
    if scripts.has_listeners() {
        scripts.fire("requestDestroyed", Value::Null);
    }
    scripts.run_actions_logged("after-request", parameters);
    let response = match end.finish() {
        Err(err) if !web::output_sent() && outcome.is_ok() => web::json_error_of(&err),
        _ => outcome?,
    };
    let mut response = attach_session(
        response,
        &guard,
        presented.as_deref(),
        visitor_cookie.as_deref(),
        token_before,
    )?;
    // the preferences this request set, for the page to apply at once (js/README.md, Preferences)
    let changed = web::take_changed_preferences();
    if !changed.is_empty() {
        response
            .headers
            .push(("X-Moqui-Preferences".to_string(), web::preferences_header(&changed)));
    }
    stamp_http_log(&mut response);
    drop(guard);
    Ok(response)
}

/// The request after its user is known: the `before-request` actions, then the servlet the path maps to, through the
/// filters on the path. A `before-request` failure fails the request, as its actions may be a check.
#[allow(clippy::too_many_arguments)]
fn run_request(
    factory: &Arc<ExecutionContextFactory>,
    scripts: &crate::web::servlet::WebappScripts,
    request: &Arc<WebRequest>,
    root: &str,
    parameters: &BTreeMap<String, Value>,
    json_list: Option<&[Value]>,
    login_error: Option<String>,
    flags: (bool, bool),
) -> Result<WebResponse, Error> {
    if scripts.has_listeners() {
        scripts.fire("requestInitialized", Value::Null);
    }
    scripts.run_actions("before-request", parameters)?;
    let Some(found) = scripts.match_servlet(&request.path) else {
        return Ok(web::json_error(404, "Not found"));
    };
    web::set_servlet_match(&found.servlet_path, found.path_info.as_deref());
    let filters = scripts.filters_for(&request.path);
    if filters.is_empty() {
        return serve_servlet(
            factory,
            &found,
            request,
            root,
            parameters,
            json_list,
            login_error.as_deref(),
            flags,
        );
    }
    // The servlet at the end of the chain runs from inside the last filter's script, so it takes what it needs along.
    let (factory, request, root) = (Arc::clone(factory), Arc::clone(request), root.to_string());
    let (parameters, json_list) = (parameters.clone(), json_list.map(<[Value]>::to_vec));
    crate::web::servlet::run_chain(
        filters,
        Box::new(move || {
            let response = serve_servlet(
                &factory,
                &found,
                &request,
                &root,
                &parameters,
                json_list.as_deref(),
                login_error.as_deref(),
                flags,
            )?;
            web::put_response(response);
            Ok(())
        }),
    )?;
    Ok(web::take_response())
}

/// One servlet's response: the screens (`MoquiServlet`), a screen as PDF (`MoquiFopServlet`), or a script's.
#[allow(clippy::too_many_arguments)]
fn serve_servlet(
    factory: &Arc<ExecutionContextFactory>,
    found: &crate::web::servlet::Match,
    request: &WebRequest,
    root: &str,
    parameters: &BTreeMap<String, Value>,
    json_list: Option<&[Value]>,
    login_error: Option<&str>,
    flags: (bool, bool),
) -> Result<WebResponse, Error> {
    use crate::web::servlet::Handler;
    match &found.servlet.handler {
        Handler::Screen => dispatch_request(factory, request, root, parameters, json_list, login_error, flags, None),
        Handler::Fop => {
            let path = found.path_info.as_deref().unwrap_or("");
            dispatch_request(
                factory,
                request,
                root,
                parameters,
                json_list,
                login_error,
                flags,
                Some(path),
            )
        }
        Handler::Script(location) => match crate::script::invoke_web_handler(location, "servlet", found.servlet.data())
        {
            Ok(()) => Ok(web::take_response()),
            Err(err) => {
                let status = web::status_for(&err);
                if status >= 500 {
                    tracing::error!("Error in servlet {} ({location}): {err}", found.servlet.name);
                }
                web::clear_output();
                Ok(web::json_error(status, &web::error_text(status, &err)))
            }
        },
    }
}

/// The HTTP log's session, visitor, and username. Read before the session guard drops.
fn stamp_http_log(response: &mut WebResponse) {
    let (session, visitor_id, remote_user) = user::http_log_identity();
    response.http_log = web::HttpLogIdentity {
        session,
        visitor_id,
        remote_user,
    };
}

/// The screens' servlet: a token route, else the screen at the path; with `fop_path` the screen at that path as a PDF.
/// The caller ends the request's session work.
#[allow(clippy::too_many_arguments)]
fn dispatch_request(
    factory: &Arc<ExecutionContextFactory>,
    request: &WebRequest,
    root: &str,
    parameters: &BTreeMap<String, Value>,
    json_list: Option<&[Value]>,
    login_error: Option<&str>,
    (header_authenticated, new_session): (bool, bool),
    fop_path: Option<&str>,
) -> Result<WebResponse, Error> {
    // `/<route>/<token>`: a token route's public screen, after its token check (token_route.rs)
    if let Some(found) = token_route::find(factory, &request.path) {
        // not again under the route's own screen, which the dispatch below renders as the root
        if !token_route::token_routes(factory)
            .iter()
            .any(|route| route.screen_location == root)
        {
            return token_route::dispatch(
                factory,
                request,
                root,
                parameters,
                json_list,
                login_error,
                (header_authenticated, new_session),
                found,
            );
        }
    }
    // `/fop/<path>`, Java's `MoquiFopServlet`: the screen at `<path>` as `html-print`, sent as PDF. The `filename`
    // parameter (else `saveFilename`) makes it a download.
    let fop_parameters;
    let parameters = if fop_path.is_some() && !parameters.contains_key("saveFilename") {
        let mut with_name = parameters.clone();
        if let Some(filename) = parameters.get("filename") {
            with_name.insert("saveFilename".to_string(), filename.clone());
        }
        fop_parameters = with_name;
        &fop_parameters
    } else {
        parameters
    };
    let job = render::Job {
        request,
        root,
        path: fop_path.unwrap_or(&request.path),
        method: &request.method,
        parameters,
        json_list,
        login_error,
        header_authenticated,
        render_mode: fop_path.map(|_| "xsl-fo"),
        enforce_request_checks: true,
        history: true,
        new_session,
        started: Instant::now(),
        options: &BROWSER,
    };
    if let Err(err) = render::run_job(factory, &job) {
        // Only a kind names a client error. A script or load failure is a 500 whatever it says, unless the database
        // was unavailable under it (a 503).
        let status = web::status_for(&err);
        let message = web::error_text(status, &err);
        if let Some(mut page) = error_page(factory, request, root, parameters, status, &err, &message) {
            web::add_retry_after(&mut page.headers, &err);
            return Ok(page);
        }
        return Ok(web::json_error_of(&err));
    }
    // The render's transaction is done; the worker waits on the PDF engine before the session work ends.
    let pdf = if fop_path.is_some() {
        match fop_response() {
            Ok(pdf) => Some(pdf),
            Err(err) => {
                let message = web::error_text(500, &err);
                if let Some(page) = error_page(factory, request, root, parameters, 500, &err, &message) {
                    return Ok(page);
                }
                return Ok(web::json_error(500, &message));
            }
        }
    } else {
        None
    };
    if let Some(pdf) = pdf {
        return Ok(pdf);
    }
    let stream = web::take_stream();
    let (status, headers, body, sent) = web::take_output();
    Ok(WebResponse {
        status: if sent { status } else { 200 },
        headers,
        body,
        stream,
        http_log: web::HttpLogIdentity::default(),
    })
}

/// The response of a `/fop` request whose render succeeded: its `html-print` document as PDF, with the render's headers
/// but the content type, and `inline` when the render named no file. Another outcome, such as a redirect to the login
/// screen, is sent as it is.
fn fop_response() -> Result<WebResponse, Error> {
    let stream = web::take_stream();
    let (status, headers, body, sent) = web::take_output();
    if sent && status != 200 {
        return Ok(WebResponse {
            status,
            headers,
            body,
            stream,
            http_log: web::HttpLogIdentity::default(),
        });
    }
    let bytes = PdfRenderer::render(&String::from_utf8_lossy(&body))?;
    let mut headers: Vec<(String, String)> = headers
        .into_iter()
        .filter(|(name, _)| !name.eq_ignore_ascii_case("content-type"))
        .collect();
    if !headers
        .iter()
        .any(|(name, _)| name.eq_ignore_ascii_case("content-disposition"))
    {
        headers.push(("Content-Disposition".to_string(), "inline".to_string()));
    }
    headers.push(("Content-Type".to_string(), "application/pdf".to_string()));
    Ok(WebResponse {
        status: 200,
        headers,
        body: bytes,
        stream: None,
        http_log: web::HttpLogIdentity::default(),
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
    // The webapp's `error-screen` of this error type, as `MoquiServlet.sendErrorResponse`; the shipped path otherwise.
    let (kind, shipped) = web::error_info::error_screen(status);
    let configured = factory.webapp_child_setting("webroot", &format!("error-screen {kind}"), "screen-path");
    let page = if configured.is_empty() {
        shipped.to_string()
    } else {
        configured.trim_start_matches('/').to_string()
    };
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
        history: false,
        new_session: false,
        started: Instant::now(),
        options: &BROWSER,
    };
    if let Err(failure) = render::run_job(factory, &job) {
        tracing::warn!("could not render the error page {page}: {failure}");
        let _ = web::take_output();
        return None;
    }
    let stream = web::take_stream();
    let (_, headers, body, sent) = web::take_output();
    sent.then_some(WebResponse {
        status,
        headers,
        body,
        stream,
        http_log: web::HttpLogIdentity::default(),
    })
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
///
/// A login the database could not serve is the request's error (a 503), not a failed login answered with 401.
fn authenticate(request: &WebRequest, parameters: &BTreeMap<String, Value>) -> Result<(Option<String>, bool), Error> {
    let mut login_error = None;
    let mut by_header = false;
    if let Some(header) = request.header("authorization") {
        if let Some((username, password)) = web::basic_credentials(header) {
            // A password login that still needs a second factor leaves nobody logged in.
            match login_user(&username, &password) {
                Ok(()) => by_header = !user::session::session_attribute_flag(user::AUTHC_FACTOR_REQUIRED),
                Err(err) if web::is_unavailable(&err) => return Err(err),
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
            if web::is_unavailable(&err) {
                return Err(err);
            }
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
                Err(err) if web::is_unavailable(&err) => return Err(err),
                Err(err) => {
                    let message = err.message().to_string();
                    web::record_login_error(&message);
                    login_error = Some(message);
                }
            }
        }
    }
    // the session the response carries is a program's, never a person's at the login form
    if by_header {
        user::note_header_login();
    }
    Ok((login_error, by_header))
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
        location: info.path_defs.last().map(|def| def.location.clone()).unwrap_or_default(),
    })
}

/// Whether the current user may run the transition (or open the screen) at `path` under `root`: the screens of its path,
/// its single service when it writes, and the transition with its action, as its link is checked. An agent's approval
/// asks this before a card is shown (plans/LLM-A2A-Assist.md 4.8), because the gate holds a write before the
/// transition's own check runs.
///
/// # Errors
///
/// Returns `ErrorKind::NotFound` when the path matches no screen or transition.
pub fn path_permitted(factory: &ExecutionContextFactory, root: &str, path: &str) -> Result<bool, Error> {
    let info = ScreenUrlInfo::parse(factory, root, path, 0)?;
    Ok(urls::path_permitted(info))
}

/// The options of a render an HTTP request asked for: a browser's, with no gate and no fence.
static BROWSER: RenderOptions = RenderOptions {
    origin: RenderOrigin::Browser,
    gate: TransitionGate::Open,
    write_fence: false,
};

fn text_of(value: Option<&Value>) -> String {
    value.map(moqui_lib::object::to_plain_string).unwrap_or_default()
}

#[cfg(test)]
mod tests;
