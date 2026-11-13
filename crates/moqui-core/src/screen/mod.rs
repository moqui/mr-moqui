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
pub(crate) mod db_form;
mod definition;
mod dispatch;
pub mod ext;
pub mod form;
mod frame;
pub mod gate;
pub mod generate;
pub(crate) mod html_text;
pub mod info;
pub(crate) mod menu;
pub mod overlay;
mod pdf;
mod public_site;
pub use public_site::hosts as public_site_hosts;
pub use public_site::{is_framework_route as is_public_site_framework_route, is_site_host as is_public_site_host};
mod render;
pub(crate) mod saved_finds;
pub(crate) mod scan;
mod schema;
mod screen_test;
pub mod shell;
pub(crate) mod sri;
pub mod stamp;
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
/// The xlsx workbook, for a table written outside a screen render (the Tools data grid's export); in a build without
/// the `xlsx` feature its `to_bytes` reports that xlsx is not built in.
pub use xlsx::{CellHints, XlsxBook};

use std::cell::Cell;
use std::collections::BTreeMap;
use std::rc::Rc;
use std::sync::Arc;
use std::time::Instant;

use moqui_lib::object::Value;

use crate::context::{Error, ExecutionContextFactory};
use crate::user::{self, login_user};
use crate::web::{self, WebRequest, WebResponse};

pub use cache::{canonical_render_mode, ConfScreen, ScreenConf, ScreenFacade, TextOutput};
pub use definition::{
    Actions, Authentication, Condition, FormDef, Implicit, ParameterDef, ResponseDef, ResponseKind, ScreenDef,
    SectionDef, SectionKind, Subscreens, SubscreensItem, TransitionDef, TreeDef, WebSettings, XmlFragment,
};
pub use frame::PAGE_PATH_HEADER;
pub use gate::{
    agent_render, clear_transition_observer, held_write_hash, install_transition_observer, interactive_request,
    HeldWrite, InteractiveRequest, RenderOptions, RenderOrigin, TransitionGate, TransitionObserver, TransitionOutcome,
};
#[cfg(any(test, feature = "test-support"))]
#[doc(hidden)]
pub use pdf::pdf_test_lock;
pub use pdf::{
    clear_pdf_engine, install_pdf_engine, install_pdf_unavailable, pdf_engine_installed, PdfEngine, PdfLibrary,
    PdfPreview, PdfPreviewRequest, PdfRenderer, PDFJS_LOCATION, PDFJS_WORKER_LOCATION, PDF_UNAVAILABLE,
    PREVIEW_UNSUPPORTED,
};
pub(crate) use render::active_depth;
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
    serve_to(factory, request, None)
}

/// [`serve`] with a sink for a response sent as it is written (docs/ui/Screen-Output.md): an html or text render, or a
/// web script, past the conf's `stream-threshold` sends its head and body to `sink` while it runs, and the returned
/// response is then only what the request ended with, which the caller does not send. A smaller one is returned whole,
/// and the sink gets nothing.
pub fn serve_to(
    factory: &Arc<ExecutionContextFactory>,
    request: WebRequest,
    sink: Option<Box<dyn web::stream::OutputSink>>,
) -> WebResponse {
    let host = request.header("host").unwrap_or("");
    let root = match info::root_screen_for_host(factory, host, "webroot") {
        Ok(location) => location,
        Err(err) => return web::early_error(factory, &request, 500, err.message()),
    };
    serve_at_to(factory, request, &root, sink)
}

/// Handle one request whose root screen is `root`.
pub fn serve_at(factory: &Arc<ExecutionContextFactory>, request: WebRequest, root: &str) -> WebResponse {
    serve_at_to(factory, request, root, None)
}

/// [`serve_at`] with a sink, as [`serve_to`].
pub fn serve_at_to(
    factory: &Arc<ExecutionContextFactory>,
    mut request: WebRequest,
    root: &str,
    sink: Option<Box<dyn web::stream::OutputSink>>,
) -> WebResponse {
    // the request owns its spooled uploads: they go when it ends, whatever still holds a copy (a script's object that
    // waits for a collector, a value kept in the context)
    let _uploads = web::multipart::ReleaseAtEnd(request.multipart.clone());
    if request.body.len() > web::body_limit(&request) {
        return web::early_error(
            factory,
            &request,
            413,
            &format!("request body is larger than {} bytes", web::body_limit(&request)),
        );
    }
    if let Some(origin) = web::refused_origin(factory, &request) {
        tracing::warn!(
            "Origin not allowed: {origin} for {} {}",
            request.method,
            token_route::redact(factory, &request.path)
        );
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
    // The page's trace id, which its later requests send as X-Moqui-Page-Trace: each of them is a trace of its own
    // that names the page's, so a browser session is not one trace.
    crate::context::ExecutionContext::with_current(|context| {
        context.shared_map().insert(
            "telemetryPageTraceId".into(),
            Value::string(
                crate::telemetry::capture()
                    .map(|trace| trace.trace_id.to_string())
                    .unwrap_or_default(),
            ),
        );
    });
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
        sink,
    ) {
        Ok(response) => response,
        Err(err) => web::json_error_of(&err),
    };
    web::finish_response(factory, &request, response, false)
}

#[allow(clippy::too_many_arguments)]
fn handle(
    factory: &Arc<ExecutionContextFactory>,
    request: &Arc<WebRequest>,
    body: bytes::Bytes,
    root: &str,
    parameters: &BTreeMap<String, Value>,
    multipart: &BTreeMap<String, Value>,
    json_list: Option<&[Value]>,
    sink: Option<Box<dyn web::stream::OutputSink>>,
) -> Result<WebResponse, Error> {
    let session_cookie = web::session_cookie_of(request);
    let visitor_cookie = web::visitor_cookie_of(request);
    let presented = session_cookie
        .as_deref()
        .map(user::verify_session_cookie)
        .transpose()?
        .flatten();
    // a visit this request starts (at the bind, or at a login on a session that put it off) records the request
    // with a token route's token left out of its initialRequest and initialReferrer, as out of the request log
    let mut visit_request = web::visit_request(request);
    for url in [&mut visit_request.url, &mut visit_request.referrer] {
        if let std::borrow::Cow::Owned(redacted) = token_route::redact(factory, url) {
            *url = redacted;
        }
    }
    let _visit_request = user::session::VisitRequestGuard::set(visit_request);
    // A static file under a screen (css, js, an image) is content, not a visit: it starts no Visitor or Visit.
    let just_content =
        ScreenUrlInfo::parse(factory, root, &request.path, 0).is_ok_and(|info| info.file_resource.is_some());
    let _no_visit = user::session::NoVisitGuard::set(just_content);
    // what the request decides about its session (a screen may decline one), from before the bind
    let _request_session = user::session::RequestSessionScope::begin();
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
    // A registered shared device (user/device.rs): its request runs as its display account, only within its route
    // group, and the web session never holds that identity. A device that is no more is refused here, whatever it asks.
    let mut device = match user::device::begin_request(request) {
        Ok(user::device::DeviceRequest::None) => None,
        Ok(user::device::DeviceRequest::Active(device)) => Some(device),
        Ok(user::device::DeviceRequest::Refused(refusal)) => {
            // a browser's page (a wall display) gets a plain page it can show, an API client the JSON error
            let mut response = if takes_html(request) {
                device_page(403, refusal.page_text())
            } else {
                web::json_error(403, refusal.message())
            };
            if refusal.ends_cookie() {
                response
                    .headers
                    .push(("Set-Cookie".to_string(), user::device::expired_cookie(request.https)));
            }
            stamp_http_log(&mut response);
            // a refused request carries no session cookie: one made for it now is never seen again
            if presented.as_deref() != Some(guard.session_id().as_str()) {
                user::session::discard_bound_session();
            }
            return Ok(response);
        }
        Err(err) => {
            let mut response = web::json_error_of(&err);
            stamp_http_log(&mut response);
            if presented.as_deref() != Some(guard.session_id().as_str()) {
                user::session::discard_bound_session();
            }
            return Ok(response);
        }
    };
    // What the last response saved for this request sits under the request's own parameters.
    let saved = user::session::take_saved_parameters();
    let error_parameters = user::session::take_error_parameters();
    let merged;
    // the parameter a device's registration fixed (a wall display's playlist) is the request's, whatever was sent
    let fixed = device.as_ref().and_then(|device| device.scope().parameter.clone());
    let parameters = if saved.is_empty() && fixed.is_none() {
        parameters
    } else {
        let mut all: BTreeMap<String, Value> = saved
            .into_iter()
            .map(|(name, value)| (name, Value::String(value)))
            .collect();
        all.extend(parameters.iter().map(|(name, value)| (name.clone(), value.clone())));
        if let Some((name, value)) = fixed {
            all.insert(name, Value::String(value));
        }
        merged = all;
        &merged
    };
    let _web = web::install(parameters.clone());
    if let Some(sink) = sink {
        web::stream::offer(sink);
    }
    web::set_multipart(multipart.clone());
    web::set_error_parameters(error_parameters);
    web::set_host(request.header("host").unwrap_or(""));
    web::set_request_info(request);
    web::set_request_locale(request.header("accept-language"));
    web::set_request(Arc::clone(request), body);
    // a device's request reads no key, Basic credentials or login parameters: its credential is its only authority
    let (login_error, header_authenticated) = match &device {
        Some(_) => (None, false),
        None => authenticate(request, parameters)?,
    };
    // A session that did not come with the request was made for it, and its token was never sent to the client.
    let new_session = presented.as_deref() != Some(guard.session_id().as_str());
    let scripts = factory.webapp_scripts();
    // As Java's `ExecutionContextImpl.destroy`, the after-request actions run however the request ended, and before
    // the session work ends.
    let end = EndRequest::armed();
    // A response sent as it is written gets its session's cookies and token, and the conf's headers, when its head
    // goes (web::stream); what the end of the request decides about the session is decided then, from the head.
    let decided: Rc<Cell<Option<SessionChoice>>> = Rc::default();
    web::stream::on_head(head_hook(HeadParts {
        factory: Arc::clone(factory),
        request: Arc::clone(request),
        cookies: guard.cookies(),
        presented: presented.clone(),
        visitor_cookie: visitor_cookie.clone(),
        token_before: token_before.clone(),
        new_session,
        decided: Rc::clone(&decided),
    }));
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
    let committed = web::stream::committed();
    let choice = match decided.get() {
        Some(choice) => choice,
        None => session_choice(new_session, matches!(&outcome, Ok(response) if is_public(response)))?,
    };
    if choice.declined {
        user::session::take_back_request_visit();
    }
    if choice.discard {
        user::session::discard_bound_session();
    }
    let finished = end.finish();
    if committed {
        // the client has the head; the body ends once the session work is done, abnormally when the request failed
        if let Err(err) = &outcome {
            web::stream::abort(&format!("the request failed after its response began: {err}"));
        }
        if let Err(err) = finished {
            tracing::error!("Error ending the session work of a request whose response was sent: {err}");
        }
        web::stream::finish();
        drop(device);
        drop(guard);
        return outcome;
    }
    let response = match finished {
        Err(err) if !web::output_sent() && outcome.is_ok() => web::json_error_of(&err),
        _ => outcome?,
    };
    let mut response = if choice.discard || choice.declined {
        response
    } else {
        let mut response = response;
        attach_session(
            &mut response,
            guard.cookies(),
            presented.as_deref(),
            visitor_cookie.as_deref(),
            token_before.as_deref(),
        )?;
        response
    };
    // a device's daily new secret goes on a response that reaches its browser with the header (this one, not streamed,
    // not one a shared cache may keep), else it is not made: a browser that misses it is stopped as a copy (user/device.rs)
    if let Some(device) = device.as_mut().filter(|_| !is_public(&response)) {
        match device.rotate() {
            Ok(Some(cookie)) => response.headers.push(("Set-Cookie".to_string(), cookie)),
            Ok(None) => {}
            Err(err) => tracing::error!(
                "Shared device {}: its secret was not replaced: {err}",
                device.scope().device_id
            ),
        }
    }
    not_modified(request, &mut response);
    // the preferences this request set, for the page to apply at once (docs/ui/Qvt-Client.md, Preferences)
    let changed = web::take_changed_preferences();
    if !changed.is_empty() {
        response
            .headers
            .push(("X-Moqui-Preferences".to_string(), web::preferences_header(&changed)));
    }
    stamp_http_log(&mut response);
    drop(device);
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
    // a restricted host (a site's theme preview host) serves its site and static files, no framework route or servlet
    // (F87); the refusal makes no session
    let servlet_route = scripts
        .match_servlet(&request.path)
        .is_some_and(|found| !matches!(found.servlet.handler, crate::web::servlet::Handler::Screen));
    if public_site::refuses_framework_route(factory, request, servlet_route) {
        user::session::decline_session();
        return Ok(web::json_error(404, "Not found"));
    }
    scripts.run_actions("before-request", parameters)?;
    let Some(found) = scripts.match_servlet(&request.path) else {
        return Ok(web::json_error(404, "Not found"));
    };
    web::set_servlet_match(&found.servlet_path, found.path_info.as_deref());
    let filters = scripts.filters_for(&request.path);
    if !filters.is_empty() {
        // a filter sees and may change the servlet's whole response
        web::stream::withdraw();
    }
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
        Handler::Script(location) => {
            // a script servlet is in no route group: a shared device's request never runs one
            if let Some(scope) = user::device::current_scope() {
                return Ok(web::json_error_of(&user::device::refused_route(&scope)));
            }
            // a script's writer commits as a servlet's does: when its buffer fills, or at `flushBuffer`
            web::stream::arm(ScreenFacade::of(factory).conf().stream_threshold);
            match crate::script::invoke_web_handler(location, "servlet", found.servlet.data()) {
                Ok(()) => Ok(web::take_response()),
                Err(err) => {
                    let status = web::status_for(&err);
                    if status >= 500 {
                        tracing::error!("Error in servlet {} ({location}): {err}", found.servlet.name);
                    }
                    if web::stream::committed() {
                        web::stream::abort(&web::error_text(status, &err));
                        return Ok(WebResponse::new(status, Vec::new()));
                    }
                    web::clear_output();
                    Ok(web::json_error(status, &web::error_text(status, &err)))
                }
            }
        }
    }
}

/// The HTTP log's session, visitor, and username. Read before the session guard drops.
fn stamp_http_log(response: &mut WebResponse) {
    let user::HttpLogIdentity {
        session_id: session,
        visitor_id,
        username: remote_user,
    } = user::http_log_identity();
    let mut attributes = crate::entity::document::Document::default();
    for key in ["type", "name", "action", "sub_type", "slow"] {
        if let Some(value) = web::attribute(&format!("moqui.telemetry.artifact.{key}")) {
            attributes.insert(format!("moqui.artifact.{key}"), value);
        }
    }
    if let Some(error) =
        crate::context::ExecutionContext::with_current(|context| context.message().errors().first().cloned()).flatten()
    {
        attributes.insert("exception.message".into(), Value::string(error));
    }
    response.http_log = web::HttpLogIdentity {
        attributes,
        user_id: user::current_user_id().ok().flatten(),
        visit_id: user::visit_id().ok().flatten(),
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
            // a token route is in no route group: a shared device's request does not reach its token check
            if let Some(scope) = user::device::current_scope() {
                return Ok(web::json_error_of(&user::device::refused_route(&scope)));
            }
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
    if let Some(route) = public_site::resolve(factory, request, root)? {
        if route.root != root {
            let mut routed = request.clone();
            routed.path = route.path.clone();
            let mut bound_parameters = public_site::parameters(&route, parameters);
            bound_parameters.insert("publicPath".to_string(), Value::string(&request.path));
            bound_parameters.insert(
                "publicHost".to_string(),
                Value::string(
                    request
                        .headers
                        .get("host")
                        .map(|h| h.split(':').next().unwrap_or(""))
                        .unwrap_or(""),
                ),
            );
            return dispatch_request(
                factory,
                &routed,
                &route.root,
                &bound_parameters,
                json_list,
                login_error,
                (header_authenticated, new_session),
                fop_path,
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
        if web::stream::committed() {
            // the client has part of the page: say where it stops and why, then end the body abnormally, so the part
            // is not taken for the whole (a render error, or a commit that failed)
            web::write_output(&format!(
                "\n{}\n",
                encode_html(&format!("Error rendering page: {message}"))
            ));
            web::stream::abort(&message);
            return Ok(WebResponse::new(status, Vec::new()));
        }
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
    // The status is 200 unless the screen or transition set one, which holds whether or not a body was written.
    let (status, headers, body, _) = web::take_output();
    Ok(WebResponse {
        status,
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

/// Whether the client takes an html page for its answer (a browser's navigation).
fn takes_html(request: &WebRequest) -> bool {
    request
        .header("accept")
        .is_some_and(|accept| accept.contains("text/html"))
}

/// The plain page of a shared device's browser ([`user::device::refusal_page`]), never cached.
fn device_page(status: u16, text: &str) -> WebResponse {
    let mut response = WebResponse::new(status, user::device::refusal_page(text).into_bytes());
    response
        .headers
        .push(("Content-Type".to_string(), "text/html; charset=utf-8".to_string()));
    response
        .headers
        .push(("Cache-Control".to_string(), "private, no-store".to_string()));
    response
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
    if !takes_html(request) {
        return None;
    }
    let first = request.path.trim_start_matches('/').split('/').next().unwrap_or("");
    if ["rest", "rpc", "status", "error", "health", "metrics"].contains(&first) {
        return None;
    }
    // a shared device's page: the plain one, never the instance's error screen (which its route group refuses anyway)
    // or the error's text
    if user::device::current_scope().is_some() {
        let _ = web::take_output();
        let text = if err.kind() == crate::context::ErrorKind::Unauthorized {
            user::device::OUTSIDE_PAGE_TEXT
        } else {
            user::device::FAILED_PAGE_TEXT
        };
        return Some(device_page(status, text));
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

/// A 200 that names its version with an `ETag` (a screen or script set it) becomes a 304 with no body when the request's
/// `If-None-Match` names the same, as a static file's does (`static_files::respond_opened`). Only for a `GET` or `HEAD`:
/// another method has done its work, and its answer is not one a cache holds.
fn not_modified(request: &WebRequest, response: &mut WebResponse) {
    let method = request.method.as_str();
    if response.status != 200 || !(method.eq_ignore_ascii_case("GET") || method.eq_ignore_ascii_case("HEAD")) {
        return;
    }
    let Some(presented) = request.header("if-none-match") else {
        return;
    };
    let matches = response
        .headers
        .iter()
        .any(|(name, tag)| name.eq_ignore_ascii_case("etag") && static_files::matches_tag(presented, tag));
    if matches {
        response.status = 304;
        response.body.clear();
        response.stream = None;
        response
            .headers
            .retain(|(name, _)| !name.eq_ignore_ascii_case("content-length"));
    }
}

/// Whether the response says a shared cache may keep it: a `Cache-Control` with the `public` directive.
fn is_public(response: &WebResponse) -> bool {
    response.headers.iter().any(|(name, value)| {
        name.eq_ignore_ascii_case("cache-control")
            && value
                .split(',')
                .any(|directive| directive.trim().eq_ignore_ascii_case("public"))
    })
}

/// What the end of a request decides about its session. A page a shared cache may keep, for a visitor with no live
/// session and nobody signed in, gets no session: no cookie or token is sent and the session made for the request is
/// not kept (impl-shop F42). A signed cookie naming a session that is gone counts as none, so a public response never
/// carries a new session's cookie for a cache to hand to others. A screen that declined a session
/// (`ec.web.declineSession()`, impl-commerce F86) gets the same whatever its caching, and a session the request brought
/// is kept but sent nothing new; either way the Visit the request started is taken back.
#[derive(Clone, Copy, Debug)]
struct SessionChoice {
    declined: bool,
    discard: bool,
}

/// The choice for a request whose response is `public` ([`is_public`]), as the request stands now.
fn session_choice(new_session: bool, public: bool) -> Result<SessionChoice, Error> {
    let declined = user::session::session_declined();
    let anonymous = user::session::current_user_id()?.is_none();
    Ok(SessionChoice {
        declined,
        discard: anonymous && new_session && (declined || public),
    })
}

/// What a request's head hook needs of it ([`head_hook`]).
struct HeadParts {
    factory: Arc<ExecutionContextFactory>,
    request: Arc<WebRequest>,
    cookies: user::SessionCookies,
    presented: Option<String>,
    visitor_cookie: Option<String>,
    token_before: Option<String>,
    new_session: bool,
    decided: Rc<Cell<Option<SessionChoice>>>,
}

/// The request's part of a streamed response's head: what [`handle`] and [`serve_at`] add to a whole response once it
/// is made, here from the head as it is when the response commits. The session choice is kept for the end of the
/// request; preferences set after the head goes are not sent.
fn head_hook(parts: HeadParts) -> web::stream::HeadHook {
    Box::new(move |head: &mut WebResponse| {
        let choice = session_choice(parts.new_session, is_public(head))?;
        parts.decided.set(Some(choice));
        if !(choice.discard || choice.declined) {
            attach_session(
                head,
                parts.cookies,
                parts.presented.as_deref(),
                parts.visitor_cookie.as_deref(),
                parts.token_before.as_deref(),
            )?;
        }
        let changed = web::take_changed_preferences();
        if !changed.is_empty() {
            head.headers
                .push(("X-Moqui-Preferences".to_string(), web::preferences_header(&changed)));
        }
        stamp_http_log(head);
        let whole = std::mem::replace(head, WebResponse::new(head.status, Vec::new()));
        *head = web::finish_response(&parts.factory, &parts.request, whole, false);
        Ok(())
    })
}

fn attach_session(
    response: &mut WebResponse,
    cookies: user::SessionCookies,
    presented: Option<&str>,
    visitor_cookie: Option<&str>,
    token_before: Option<&str>,
) -> Result<(), Error> {
    if presented != Some(cookies.session_id().as_str()) {
        response
            .headers
            .push(("Set-Cookie".to_string(), cookies.session_cookie_header()?));
    }
    if cookies.visitor_id().as_deref() != visitor_cookie {
        if let Some(cookie) = cookies.visitor_cookie_header() {
            response.headers.push(("Set-Cookie".to_string(), cookie));
        }
    }
    let token_after = user::csrf_token()?;
    if presented != Some(cookies.session_id().as_str()) || token_before != Some(token_after.as_str()) {
        response
            .headers
            .push(("moquiSessionToken".to_string(), token_after.clone()));
        response.headers.push(("X-CSRF-Token".to_string(), token_after));
    }
    Ok(())
}

fn peek_token() -> Option<String> {
    user::session::peek_csrf_token()
}

/// The login error, if any, and whether this request logged a user in from its own credentials
/// (`api_key`, `login_key`, Basic, or `authUsername`). Such a request carries no ambient cookie
/// authority, so it skips the session-token check as it does in the Java `ScreenRenderImpl`.
///
/// A login the database could not serve is the request's error (a 503), not a failed login answered with 401.
fn authenticate(request: &WebRequest, parameters: &BTreeMap<String, Value>) -> Result<(Option<String>, bool), Error> {
    // A key is this request's authority even when the client also keeps a session for attributes: a rejected key
    // never falls back to a signed-in session's user, and a key for another user never takes over its session.
    // With nobody signed in, a rejected key leaves the request anonymous with the login error, as in Java Moqui.
    let signed_in = user::current_user_id()?.is_some_and(|id| !id.is_empty());
    match user::apply_header_keys(request.header("api_key"), request.header("login_key")) {
        Ok(false) => {}
        Ok(true) => {
            user::note_header_login();
            return Ok((None, true));
        }
        Err(err) if web::is_unavailable(&err) => return Err(err),
        Err(err) => {
            let message = err.message().to_string();
            web::record_login_error(&message);
            if signed_in {
                return Err(Error::of_kind(crate::context::ErrorKind::AuthenticationRequired, message));
            }
            return Ok((Some(message), false));
        }
    }
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
        location: info
            .path_defs
            .last()
            .map(|def| def.location.clone())
            .unwrap_or_default(),
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
