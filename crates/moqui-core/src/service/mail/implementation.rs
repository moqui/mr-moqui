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

//! Outbound SMTP and inbound IMAP/POP3 for the email services. `mail` registers [`send_template`], [`send_message`],
//! and [`poll_server`] by those function paths.
//!
//! `send#EmailTemplate` follows `sendEmailTemplate.groovy`. A template with `bodyScreenLocation` renders
//! that screen with `makeRender()` (no socket, its own web slot, so a request in progress is not touched)
//! in `html` for the body and in `text` for the alternative part, with the service parameters, the
//! `bodyParameters` and the template values (`fromAddress`, `subject`, `webappName`, `webHostName`,
//! `emailMessageId` and the rest) in the context, and links built from the conf `webapp` named by
//! `webappName`. A `body` or `bodyText` the caller passed is used instead of that render. Screen
//! attachments render the same way. A screen attachment in `xsl-fo` renders `html-print` and attaches its PDF;
//! when PDF is not available it is skipped with a logged warning, as Java does when the PDF transform fails. A render
//! that fails stops the send with the message `Error sending email: ...`, as the Groovy outer catch did.

#[cfg(test)]
use std::cell::RefCell;
use std::collections::BTreeMap;
use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpStream;
#[cfg(test)]
use std::rc::Rc;
use std::sync::Arc;
use std::time::Duration;

use crate::service::util::{field_text, has_error, map_text, now_stamp, param_text};

use indexmap::IndexMap;
use moqui_lib::codec::hex_encode;
use moqui_lib::error::Error as LibError;
use moqui_lib::object::{basic_convert, is_empty, to_plain_string, Value};
#[cfg(test)]
use moqui_lib::sync::lock;

use crate::context::{ExecutionContext, ExecutionContextFactory};
use crate::entity::{EntityFacade, EntityValue};
use crate::screen::{make_render, PdfRenderer, ScreenFacade};
use crate::transaction::TransactionAttribute;

const TEMPLATE: &str = "moqui.basic.email.EmailTemplate";
const SERVER: &str = "moqui.basic.email.EmailServer";
const MESSAGE: &str = "moqui.basic.email.EmailMessage";
const ATTACHMENT: &str = "moqui.basic.email.EmailTemplateAttachment";

pub(super) fn send_template(
    parameters: &BTreeMap<String, Value>,
    facade: &EntityFacade,
) -> Result<BTreeMap<String, Value>, LibError> {
    if facade.sim_active() {
        return Ok(ids("SIM", ""));
    }
    let template_id = param_text(parameters, "emailTemplateId");
    let Some(template) = find_one(facade, TEMPLATE, "emailTemplateId", &template_id)? else {
        return fail(format!("No EmailTemplate record found for ID [{template_id}]"));
    };
    // The script's context: the parameters, then the bodyParameters over them, then the template values.
    let _frame = crate::script::ContextFrame::push();
    for (name, value) in parameters {
        put_context(name, value.clone())?;
    }
    if let Some(map) = body_parameters(parameters) {
        for (name, value) in map {
            put_context(name, value.clone())?;
        }
    }
    let mut cc = param_text(parameters, "ccAddresses");
    let mut bcc = param_text(parameters, "bccAddresses");
    cc = merge_lists(&cc, &field_text(&template, "ccAddresses"));
    bcc = merge_lists(&bcc, &field_text(&template, "bccAddresses"));
    let headers = expand_headers(&template, body_parameters(parameters))?;
    let supplied_body = param_text(parameters, "body");
    let supplied_text = param_text(parameters, "bodyText");
    let screen = field_text(&template, "bodyScreenLocation");
    if screen.is_empty() && supplied_body.is_empty() && supplied_text.is_empty() {
        return fail("email has no body");
    }
    let env = RenderEnv::new(&template, parameters);
    let email_type = {
        let given = param_text(parameters, "emailTypeEnumId");
        if given.is_empty() {
            field_text(&template, "emailTypeEnumId")
        } else {
            given
        }
    };
    put_context("emailTypeEnumId", Value::string(&email_type))?;
    put_context("ccAddresses", Value::string(&cc))?;
    put_context("bccAddresses", Value::string(&bcc))?;
    put_context("fromAddress", Value::string(&headers.from_address))?;
    put_context("fromName", Value::string(&headers.from_name))?;
    put_context("subject", Value::string(&headers.subject))?;
    put_context("webappName", Value::string(&env.webapp_name))?;
    put_context("webHostName", Value::string(&env.web_host_name))?;
    let mut email_message_id = String::new();
    if create_message(parameters) {
        email_message_id = insert_draft(facade, parameters, &template, &headers, &cc, &bcc)?;
        put_context("emailMessageId", Value::string(&email_message_id))?;
    }
    let (body, body_text) = match render_bodies(&env, &screen, supplied_body, supplied_text) {
        Ok(pair) => pair,
        Err(err) => return Ok(send_error(&err, &email_message_id, &template_id)),
    };
    if !email_message_id.is_empty() {
        update_ready(facade, &email_message_id, &body, &body_text)?;
    }
    let server_id = field_text(&template, "emailServerId");
    let Some(server) = find_one(facade, SERVER, "emailServerId", &server_id)? else {
        return fail_with(
            format!("No EmailServer record found for EmailTemplate {template_id}"),
            &email_message_id,
        );
    };
    if headers.from_address.is_empty() {
        return fail_with(
            format!("From address is empty for EmailTemplate {template_id}"),
            &email_message_id,
        );
    }
    let host = field_text(&server, "smtpHost");
    if host.is_empty() {
        tracing::warn!(
            server = %server_id,
            message = %email_message_id,
            template = %template_id,
            "SMTP Host is empty, not sending email"
        );
        return Ok(ids(&email_message_id, ""));
    }
    if let Err(text) = require_host(&host) {
        return fail_with(text, &email_message_id);
    }
    let message_id = match new_message_id(&headers.from_address) {
        Ok(message_id) => message_id,
        Err(text) => return fail_with(text, &email_message_id),
    };
    let mut letter = Letter {
        subject: headers.subject.clone(),
        from_address: headers.from_address.clone(),
        from_name: headers.from_name.clone(),
        to: split_addresses(&param_text(parameters, "toAddresses")),
        cc: split_addresses(&cc),
        bcc: split_addresses(&bcc),
        reply_to: split_addresses(&field_text(&template, "replyToAddresses")),
        bounce: field_text(&template, "bounceAddress"),
        body,
        body_text,
        attachments: Vec::new(),
        message_id,
    };
    if !filter_recipients(&mut letter, &field_text(&server, "allowedToDomains"), &email_message_id) {
        return Ok(ids(&email_message_id, ""));
    }
    letter.attachments = match collect_attachments(facade, parameters, &template_id, &env) {
        Ok(attachments) => attachments,
        Err(err) => return Ok(send_error(&err, &email_message_id, &template_id)),
    };
    if let Err(err) = deliver(&letter, &server) {
        note_message(format!("Error sending email: {err}"));
        return Ok(ids(&email_message_id, ""));
    }
    if !email_message_id.is_empty() {
        update_sent(facade, &email_message_id, &letter.message_id)?;
    }
    Ok(ids(&email_message_id, &letter.message_id))
}

pub(super) fn send_message(
    parameters: &BTreeMap<String, Value>,
    facade: &EntityFacade,
) -> Result<BTreeMap<String, Value>, LibError> {
    let email_message_id = param_text(parameters, "emailMessageId");
    if facade.sim_active() {
        return Ok(ids("SIM", ""));
    }
    let Some(message) = find_one(facade, MESSAGE, "emailMessageId", &email_message_id)? else {
        return fail(format!("No EmailMessage record found for ID {email_message_id}"));
    };
    let status = field_text(&message, "statusId");
    if status == "ES_DRAFT" {
        return fail(format!("Email Message {email_message_id} is in Draft status"));
    }
    if status == "ES_CANCELLED" {
        return fail(format!("Email Message {email_message_id} is Cancelled"));
    }
    let body = field_text(&message, "body");
    let body_text = field_text(&message, "bodyText");
    let from_address = field_text(&message, "fromAddress");
    let to_addresses = field_text(&message, "toAddresses");
    if body.is_empty() && body_text.is_empty() {
        return fail(format!("Email Message {email_message_id} has no body"));
    }
    if from_address.is_empty() {
        return fail(format!("Email Message {email_message_id} has no from address"));
    }
    if to_addresses.is_empty() {
        return fail(format!("Email Message {email_message_id} has no to address"));
    }
    let server_id = field_text(&message, "emailServerId");
    let Some(server) = find_one(facade, SERVER, "emailServerId", &server_id)? else {
        return fail(format!(
            "No Email Server record found for Email Message {email_message_id}"
        ));
    };
    let host = field_text(&server, "smtpHost");
    if host.is_empty() {
        tracing::warn!(server = %server_id, message = %email_message_id, "SMTP Host is empty, not sending email");
        return Ok(ids(&email_message_id, ""));
    }
    if let Err(text) = require_host(&host) {
        return fail(text);
    }
    let template_id = field_text(&message, "emailTemplateId");
    let template = find_one(facade, TEMPLATE, "emailTemplateId", &template_id)?;
    let (reply_to, bounce) = match &template {
        Some(row) => (field_text(row, "replyToAddresses"), field_text(row, "bounceAddress")),
        None => (String::new(), String::new()),
    };
    let message_id = match new_message_id(&from_address) {
        Ok(message_id) => message_id,
        Err(text) => return fail(text),
    };
    let mut letter = Letter {
        subject: field_text(&message, "subject"),
        from_address,
        from_name: field_text(&message, "fromName"),
        to: split_addresses(&to_addresses),
        cc: split_addresses(&field_text(&message, "ccAddresses")),
        bcc: split_addresses(&field_text(&message, "bccAddresses")),
        reply_to: split_addresses(&reply_to),
        bounce,
        body,
        body_text,
        attachments: Vec::new(),
        message_id,
    };
    if !filter_recipients(&mut letter, &field_text(&server, "allowedToDomains"), &email_message_id) {
        return Ok(ids(&email_message_id, ""));
    }
    if let Err(err) = deliver(&letter, &server) {
        note_message(format!("Error sending email: {err}"));
        return Ok(ids(&email_message_id, ""));
    }
    if status == "ES_READY" || status == "ES_BOUNCED" {
        update_sent(facade, &email_message_id, &letter.message_id)?;
    }
    Ok(ids(&email_message_id, &letter.message_id))
}

struct Expanded {
    from_address: String,
    from_name: String,
    subject: String,
}

fn expand_headers(template: &EntityValue, parameters: Option<&IndexMap<String, Value>>) -> Result<Expanded, LibError> {
    let _frame = crate::script::ContextFrame::push();
    if let Some(map) = parameters {
        for (name, value) in map {
            crate::script::set_context_name(name, value.clone()).map_err(|err| LibError::new(err.message()))?;
        }
    }
    let expand = |text: &str| crate::script::expand(text).map_err(|err| LibError::new(err.message()));
    Ok(Expanded {
        from_address: expand(&field_text(template, "fromAddress"))?,
        from_name: expand(&field_text(template, "fromName"))?,
        subject: expand(&field_text(template, "subject"))?,
    })
}

/// What a screen render for this email needs: the factory, the webapp the links are for, and the
/// `bodyParameters`, which the render also sees as its request parameters.
struct RenderEnv {
    factory: Option<Arc<ExecutionContextFactory>>,
    webapp_name: String,
    web_host_name: String,
    base_url: String,
    parameters: BTreeMap<String, Value>,
}

impl RenderEnv {
    fn new(template: &EntityValue, parameters: &BTreeMap<String, Value>) -> Self {
        let factory = ExecutionContext::with_current(|ctx| Arc::clone(ctx.factory()));
        let webapp_name = {
            let named = field_text(template, "webappName");
            if named.is_empty() {
                "webroot".to_string()
            } else {
                named
            }
        };
        let base_url = factory
            .as_ref()
            .map(|factory| factory.webapp_root_url(&webapp_name))
            .unwrap_or_default();
        let parameters = body_parameters(parameters)
            .map(|map| map.iter().map(|(key, value)| (key.clone(), value.clone())).collect())
            .unwrap_or_default();
        Self {
            factory,
            webapp_name,
            web_host_name: field_text(template, "webHostName"),
            base_url,
            parameters,
        }
    }

    /// Render `location` as the root screen, or `screen_path` under the default root screen (Java's
    /// `rootScreenFromHost`, which this port has one of) as the last standalone screen, in `mode`.
    fn render(&self, location: &str, screen_path: &str, mode: &str) -> Result<String, String> {
        self.render_bytes(location, screen_path, mode)
            .map(|bytes| String::from_utf8_lossy(&bytes).into_owned())
    }

    /// The body of the render, which for a binary mode such as `xlsx` is the document.
    fn render_bytes(&self, location: &str, screen_path: &str, mode: &str) -> Result<Vec<u8>, String> {
        let Some(factory) = &self.factory else {
            return Err("no execution context to render a screen in".to_string());
        };
        let mut parameters = self.parameters.clone();
        let mut render = make_render(factory).base_url(self.base_url.clone()).render_mode(mode);
        if screen_path.is_empty() {
            render = render.root_screen(location);
        } else {
            parameters.insert("lastStandalone".to_string(), Value::string("true"));
            render = render.screen_path(screen_path);
        }
        let shown = if screen_path.is_empty() { location } else { screen_path };
        let output = render
            .parameters(parameters)
            .render()
            .map_err(|err| format!("could not render {shown} as {mode}: {}", err.message()))?;
        if output.status != 200 {
            return Err(format!("could not render {shown} as {mode}: status {}", output.status));
        }
        Ok(output.body)
    }

    fn is_binary(&self, mode: &str) -> bool {
        self.factory
            .as_ref()
            .is_some_and(|factory| ScreenFacade::of(factory).is_render_mode_binary(mode))
    }

    /// True when the screen at `location` has a `text` render, as `render-modes` says.
    fn allows_text(&self, location: &str) -> bool {
        let Some(factory) = &self.factory else {
            return false;
        };
        ScreenFacade::of(factory)
            .screen_def(factory, location)
            .map(|def| def.allows_render_mode("text"))
            .unwrap_or(false)
    }

    fn mime_type(&self, mode: &str) -> Option<String> {
        let factory = self.factory.as_ref()?;
        ScreenFacade::of(factory).mime_type(mode).map(str::to_string)
    }
}

/// The html body and the text alternative. A part the caller passed wins; the other is rendered from
/// `bodyScreenLocation` (html, and text when the screen has a text render). Blank output is no part. Java renders
/// text whatever `render-modes` says; the screen renderer here refuses a mode it does not list, which would fail the
/// send.
fn render_bodies(env: &RenderEnv, screen: &str, body: String, body_text: String) -> Result<(String, String), String> {
    if screen.is_empty() {
        return Ok((body, body_text));
    }
    let body = if body.is_empty() {
        blank_to_empty(env.render(screen, "", "html")?)
    } else {
        body
    };
    let body_text = if body_text.is_empty() && env.allows_text(screen) {
        blank_to_empty(env.render(screen, "", "text")?)
    } else {
        body_text
    };
    Ok((body, body_text))
}

fn blank_to_empty(text: String) -> String {
    if text.trim().is_empty() {
        String::new()
    } else {
        text
    }
}

/// The Groovy script's outer catch: log, add a message (not an error), and send nothing. A created
/// `EmailMessage` stays as it was.
fn send_error(err: &str, email_message_id: &str, template_id: &str) -> BTreeMap<String, Value> {
    tracing::error!(template = %template_id, message = %email_message_id, error = %err, "Error in sendEmailTemplate");
    note_message(format!("Error sending email: {err}"));
    ids(email_message_id, "")
}

fn put_context(name: &str, value: Value) -> Result<(), LibError> {
    crate::script::set_context_name(name, value).map_err(|err| LibError::new(err.message()))
}

fn create_message(parameters: &BTreeMap<String, Value>) -> bool {
    match parameters.get("createEmailMessage") {
        Some(Value::Bool(flag)) => *flag,
        Some(value) if is_empty(value) => false,
        Some(value) => {
            let text = to_plain_string(value).to_ascii_lowercase();
            !matches!(text.as_str(), "false" | "n" | "no" | "0")
        }
        None => true,
    }
}

fn insert_draft(
    facade: &EntityFacade,
    parameters: &BTreeMap<String, Value>,
    template: &EntityValue,
    headers: &Expanded,
    cc: &str,
    bcc: &str,
) -> Result<String, LibError> {
    let email_type = {
        let given = param_text(parameters, "emailTypeEnumId");
        if given.is_empty() {
            field_text(template, "emailTypeEnumId")
        } else {
            given
        }
    };
    let mut from_user = param_text(parameters, "fromUserId");
    if from_user.is_empty() {
        from_user = crate::user::current_user_id().ok().flatten().unwrap_or_default();
    }
    let to_user = param_text(parameters, "toUserId");
    let to = param_text(parameters, "toAddresses");
    let template_id = field_text(template, "emailTemplateId");
    let server_id = field_text(template, "emailServerId");
    force_new(facade, || {
        let _authz = ExecutionContext::with_current(|ctx| ctx.disable_authz());
        let mut row = facade.make_value(MESSAGE)?;
        row.set_sequenced_id_primary(facade)?;
        let id = field_text(&row, "emailMessageId");
        set_text(&mut row, "statusId", "ES_DRAFT");
        set_text(&mut row, "subject", &headers.subject);
        set_text(&mut row, "fromAddress", &headers.from_address);
        set_text(&mut row, "fromName", &headers.from_name);
        set_text(&mut row, "toAddresses", &to);
        set_text(&mut row, "ccAddresses", cc);
        set_text(&mut row, "bccAddresses", bcc);
        set_text(&mut row, "contentType", "text/html");
        set_text(&mut row, "emailTypeEnumId", &email_type);
        set_text(&mut row, "emailTemplateId", &template_id);
        set_text(&mut row, "emailServerId", &server_id);
        set_text(&mut row, "fromUserId", &from_user);
        set_text(&mut row, "toUserId", &to_user);
        row.create(facade)?;
        Ok(id)
    })
}

fn update_ready(facade: &EntityFacade, id: &str, body: &str, body_text: &str) -> Result<(), LibError> {
    force_new(facade, || {
        let _authz = ExecutionContext::with_current(|ctx| ctx.disable_authz());
        let mut row = facade.make_value(MESSAGE)?;
        set_text(&mut row, "emailMessageId", id);
        set_text(&mut row, "statusId", "ES_READY");
        set_text(&mut row, "body", body);
        set_text(&mut row, "bodyText", body_text);
        row.update(facade)?;
        Ok(())
    })
}

fn update_sent(facade: &EntityFacade, id: &str, message_id: &str) -> Result<(), LibError> {
    force_new(facade, || {
        let _authz = ExecutionContext::with_current(|ctx| ctx.disable_authz());
        let mut row = facade.make_value(MESSAGE)?;
        set_text(&mut row, "emailMessageId", id);
        set_text(&mut row, "statusId", "ES_SENT");
        set_text(&mut row, "messageId", message_id);
        row.set("sentDate", now_stamp());
        row.update(facade)?;
        Ok(())
    })
}

fn force_new<T>(facade: &EntityFacade, body: impl FnOnce() -> Result<T, LibError>) -> Result<T, LibError> {
    let tx = facade.transactions_for(MESSAGE)?;
    tx.run(TransactionAttribute::ForceNew, None, body)
}

fn filter_recipients(letter: &mut Letter, allowed: &str, email_message_id: &str) -> bool {
    let domains: Vec<String> = allowed
        .split(',')
        .map(str::trim)
        .filter(|entry| !entry.is_empty())
        .map(str::to_string)
        .collect();
    if domains.is_empty() {
        letter.to.retain(|address| !address.trim().is_empty());
        return !letter.to.is_empty();
    }
    let mut skipped = Vec::new();
    retain_domains(&mut letter.to, &domains, &mut skipped);
    retain_domains(&mut letter.cc, &domains, &mut skipped);
    retain_domains(&mut letter.bcc, &domains, &mut skipped);
    if letter.to.is_empty() {
        tracing::warn!(
            message = %email_message_id,
            skipped = %skipped.join(","),
            "not sending email with no To address; domains were not allowed"
        );
        note_message(format!(
            "Not sending email message with no To Address; address(es) skipped because domain not allowed: {skipped:?}"
        ));
        return false;
    }
    if !skipped.is_empty() {
        tracing::warn!(skipped = %skipped.join(","), "some email addresses were skipped because the domain is not allowed");
    }
    true
}

fn retain_domains(addresses: &mut Vec<String>, domains: &[String], skipped: &mut Vec<String>) {
    addresses.retain(|address| {
        if domain_allowed(address, domains) {
            true
        } else {
            skipped.push(address.clone());
            false
        }
    });
}

/// Whether the address's domain is one of `domains` or below one, case-insensitive: `example.com` allows
/// `mail.example.com` but not `evilexample.com`. Java's `isDomainAllowed` compared the text after the `@` with
/// `endsWith`, which allowed the second and refused `Name <a@example.com>` for its `>`.
fn domain_allowed(address: &str, domains: &[String]) -> bool {
    let address = bare_address(address);
    if address.is_empty() {
        return false;
    }
    if domains.is_empty() {
        return true;
    }
    let Some((_, domain)) = address.rsplit_once('@') else {
        return false;
    };
    let domain = domain.to_ascii_lowercase();
    domains.iter().any(|entry| {
        let entry = entry.trim().trim_start_matches(['@', '.']).to_ascii_lowercase();
        !entry.is_empty() && (domain == entry || domain.ends_with(&format!(".{entry}")))
    })
}

/// The address in `Name <address>`, or the text as it is.
fn bare_address(text: &str) -> &str {
    let text = text.trim();
    match (text.rfind('<'), text.rfind('>')) {
        (Some(open), Some(close)) if close > open => text[open + 1..close].trim(),
        _ => text,
    }
}

fn require_host(host: &str) -> Result<(), String> {
    if host_allowed(host) {
        Ok(())
    } else {
        Err(format!("Email host {host} is not in email_allowed_hosts"))
    }
}

fn host_allowed(host: &str) -> bool {
    let allowed = moqui_lib::env::get_prop_or_env("email_allowed_hosts");
    if allowed.trim().is_empty() {
        return true;
    }
    let mut name = host.trim().to_ascii_lowercase();
    if name.is_empty() {
        return false;
    }
    if name.ends_with('.') {
        name.pop();
    }
    for part in allowed.split(',') {
        let mut entry = part.trim().to_ascii_lowercase();
        if entry.is_empty() {
            continue;
        }
        if entry.ends_with('.') {
            entry.pop();
        }
        if name == entry {
            return true;
        }
        if dns_suffix(&entry) && name.ends_with(&format!(".{entry}")) {
            return true;
        }
    }
    false
}

fn dns_suffix(entry: &str) -> bool {
    !entry.contains(':') && entry.chars().any(|ch| ch.is_ascii_alphabetic())
}

/// The `attachments` parameter, then the template's `EmailTemplateAttachment` rows. An error is a
/// screen render or an expression that failed, which stops the send.
fn collect_attachments(
    facade: &EntityFacade,
    parameters: &BTreeMap<String, Value>,
    template_id: &str,
    env: &RenderEnv,
) -> Result<Vec<AttachmentBytes>, String> {
    let mut out = Vec::new();
    if let Some(Value::List(items)) = parameters.get("attachments") {
        for item in items {
            if let Value::Map(map) = item {
                push_parameter_attachment(&mut out, map, env)?;
            }
        }
    }
    let rows = facade
        .find(ATTACHMENT)
        .and_then(|find| find.eq("emailTemplateId", Value::string(template_id)).list())
        .map_err(|err| err.to_string())?;
    for row in rows.iter() {
        push_stored_attachment(&mut out, row, env)?;
    }
    Ok(out)
}

fn push_parameter_attachment(
    out: &mut Vec<AttachmentBytes>,
    map: &IndexMap<String, Value>,
    env: &RenderEnv,
) -> Result<(), String> {
    let filename = expand_text(&map_text(map, "fileName"))?;
    let mode = map_text(map, "screenRenderMode");
    let location = map_text(map, "attachmentLocation");
    let screen_path = map_text(map, "screenPath");
    if let Some(text) = map.get("contentText").filter(|value| !is_empty(value)) {
        let mime = fallback_type(&map_text(map, "contentType"), &filename, "text/plain");
        out.push(AttachmentBytes {
            filename,
            content_type: mime,
            bytes: to_plain_string(text).into_bytes(),
        });
        return Ok(());
    }
    if let Some(Value::Bytes(bytes)) = map.get("contentBytes") {
        let mime = fallback_type(&map_text(map, "contentType"), &filename, "application/octet-stream");
        out.push(AttachmentBytes {
            filename,
            content_type: mime,
            bytes: bytes.clone(),
        });
        return Ok(());
    }
    if !mode.is_empty() && (!location.is_empty() || !screen_path.is_empty()) {
        let screen = ScreenAttachment {
            filename: &filename,
            mode: &mode,
            location: &location,
            screen_path: &screen_path,
            content_type: &map_text(map, "contentType"),
        };
        return render_attachment(out, &screen, env);
    }
    if location.is_empty() {
        tracing::error!(file = %filename, "attachment info is invalid: no content, screen or location");
        return Ok(());
    }
    match read_location(&location) {
        Ok(bytes) => {
            let mime = fallback_type(&map_text(map, "contentType"), &filename, "application/octet-stream");
            out.push(AttachmentBytes {
                filename,
                content_type: mime,
                bytes,
            });
        }
        Err(err) => note_error(err),
    }
    Ok(())
}

/// One `EmailTemplateAttachment`: skipped when `attachmentCondition` is false; a screen render (one for
/// each entry of `forEachIn`, the entry's fields in the context) when `screenRenderMode` is set; else the
/// bytes at `attachmentLocation`.
fn push_stored_attachment(out: &mut Vec<AttachmentBytes>, row: &EntityValue, env: &RenderEnv) -> Result<(), String> {
    let mode = field_text(row, "screenRenderMode");
    let location = field_text(row, "attachmentLocation");
    let filename = field_text(row, "fileName");
    let condition = field_text(row, "attachmentCondition");
    if !condition.is_empty() {
        let holds = crate::script::eval_condition(&condition)
            .map_err(|err| format!("attachmentCondition of {filename}: {}", err.message()))?;
        if !holds {
            return Ok(());
        }
    }
    if !mode.is_empty() {
        let screen = ScreenAttachment {
            filename: &filename,
            mode: &mode,
            location: &location,
            screen_path: &field_text(row, "screenPath"),
            content_type: "",
        };
        let for_each_in = field_text(row, "forEachIn");
        if for_each_in.is_empty() {
            return render_attachment(out, &screen, env);
        }
        let entries = match crate::script::eval_value(&for_each_in)
            .map_err(|err| format!("forEachIn of {filename}: {}", err.message()))?
        {
            Value::List(items) => items,
            Value::Null => Vec::new(),
            other => return Err(format!("forEachIn of {filename} is not a list: {other:?}")),
        };
        for entry in entries {
            let _frame = crate::script::ContextFrame::push();
            match entry {
                Value::Map(map) => {
                    for (name, value) in map {
                        crate::script::set_context_name(&name, value).map_err(|err| err.message().to_string())?;
                    }
                }
                other => {
                    crate::script::set_context_name("forEachEntry", other).map_err(|err| err.message().to_string())?;
                }
            }
            render_attachment(out, &screen, env)?;
        }
        return Ok(());
    }
    if location.is_empty() {
        note_error(format!("attachment {filename} has no location"));
        return Ok(());
    }
    match read_location(&location) {
        Ok(bytes) => out.push(AttachmentBytes {
            filename,
            content_type: mime_from_name(&field_text(row, "fileName")),
            bytes,
        }),
        Err(err) => note_error(err),
    }
    Ok(())
}

struct ScreenAttachment<'a> {
    filename: &'a str,
    mode: &'a str,
    location: &'a str,
    screen_path: &'a str,
    content_type: &'a str,
}

/// `renderScreenAttachment`: the file name defaults to the screen's name with the mode as extension and
/// is expanded in the context; the content type is the caller's or the mode's; blank output is no
/// attachment. `xsl-fo` renders `html-print` and attaches its PDF; when PDF fails it is logged and skipped, as in Java.
/// A binary mode such as `xlsx` attaches the document it renders.
fn render_attachment(
    out: &mut Vec<AttachmentBytes>,
    screen: &ScreenAttachment<'_>,
    env: &RenderEnv,
) -> Result<(), String> {
    let mut filename = screen.filename.to_string();
    if filename.is_empty() {
        // The name less its extension; Java drops 4 characters, the same for a `.xml` screen, and fails on a
        // `screenPath` attachment with no location.
        let extension = if screen.mode == "xsl-fo" { "pdf" } else { screen.mode };
        let source = if screen.location.is_empty() {
            screen.screen_path
        } else {
            screen.location
        };
        let base = source.rsplit('/').next().unwrap_or(source);
        let stem = base.rsplit_once('.').map_or(base, |(stem, _)| stem);
        filename = format!("{stem}.{extension}");
    }
    let filename = expand_text(&filename)?;
    if env.is_binary(screen.mode) {
        // A binary mode (xlsx) is the document as rendered.
        let bytes = env.render_bytes(screen.location, screen.screen_path, screen.mode)?;
        if bytes.is_empty() {
            return Ok(());
        }
        let content_type = if screen.content_type.is_empty() {
            env.mime_type(screen.mode)
                .unwrap_or_else(|| "application/octet-stream".to_string())
        } else {
            screen.content_type.to_string()
        };
        out.push(AttachmentBytes {
            filename,
            content_type,
            bytes,
        });
        return Ok(());
    }
    let text = env.render(screen.location, screen.screen_path, screen.mode)?;
    if text.trim().is_empty() {
        return Ok(());
    }
    if screen.mode == "xsl-fo" {
        // The html-print document becomes the PDF. A failure is logged and the attachment skipped, as in Java.
        let bytes = match PdfRenderer::render(&text) {
            Ok(bytes) => bytes,
            Err(err) => {
                tracing::warn!(file = %filename, "Error generating PDF from XSL-FO: {}", err.message());
                return Ok(());
            }
        };
        let content_type = if screen.content_type.is_empty() {
            "application/pdf".to_string()
        } else {
            screen.content_type.to_string()
        };
        out.push(AttachmentBytes {
            filename,
            content_type,
            bytes,
        });
        return Ok(());
    }
    let content_type = if screen.content_type.is_empty() {
        env.mime_type(screen.mode)
            .unwrap_or_else(|| "application/octet-stream".to_string())
    } else {
        screen.content_type.to_string()
    };
    out.push(AttachmentBytes {
        filename,
        content_type,
        bytes: text.into_bytes(),
    });
    Ok(())
}

fn expand_text(text: &str) -> Result<String, String> {
    crate::script::expand(text).map_err(|err| err.message().to_string())
}

fn read_location(location: &str) -> Result<Vec<u8>, String> {
    let path = crate::context::locate(location).map_err(|err| err.message().to_string())?;
    std::fs::read(&path).map_err(|err| format!("could not read attachment {location}: {err}"))
}

fn fallback_type(given: &str, filename: &str, default: &str) -> String {
    if !given.is_empty() {
        given.to_string()
    } else if filename.is_empty() {
        default.to_string()
    } else {
        mime_from_name(filename)
    }
}

fn mime_from_name(filename: &str) -> String {
    match filename
        .rsplit_once('.')
        .map(|(_, ext)| ext.to_ascii_lowercase())
        .as_deref()
    {
        Some("html" | "htm") => "text/html",
        Some("txt") => "text/plain",
        Some("pdf") => "application/pdf",
        Some("png") => "image/png",
        Some("jpg" | "jpeg") => "image/jpeg",
        Some("csv") => "text/csv",
        Some("xlsx") => "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet",
        Some("xml") => "application/xml",
        Some("json") => "application/json",
        _ => "application/octet-stream",
    }
    .to_string()
}

#[derive(Clone, Debug)]
struct AttachmentBytes {
    filename: String,
    content_type: String,
    bytes: Vec<u8>,
}

#[derive(Clone, Debug)]
struct Letter {
    subject: String,
    from_address: String,
    from_name: String,
    to: Vec<String>,
    cc: Vec<String>,
    bcc: Vec<String>,
    reply_to: Vec<String>,
    bounce: String,
    body: String,
    body_text: String,
    attachments: Vec<AttachmentBytes>,
    message_id: String,
}

fn deliver(letter: &Letter, server: &EntityValue) -> Result<(), String> {
    #[cfg(test)]
    if push_sink(letter) {
        return Ok(());
    }
    lettre_send(letter, server)
}

fn lettre_send(letter: &Letter, server: &EntityValue) -> Result<(), String> {
    let host = field_text(server, "smtpHost");
    let port = parse_port(&field_text(server, "smtpPort"), 25)?;
    let message = build_lettre(letter)?;
    let user = field_text(server, "mailUsername");
    let password = field_text(server, "mailPassword");
    // smtpSsl is TLS from the start. smtpStartTls upgrades when the server offers it, as Java's setStartTLSEnabled
    // (not required), so a server without STARTTLS still takes the mail. Both trust `email_tls_ca_file` too.
    use lettre::transport::smtp::client::Tls;
    let transport = if field_text(server, "smtpSsl") == "Y" {
        creds(
            lettre::SmtpTransport::builder_dangerous(&host)
                .port(port)
                .tls(Tls::Wrapper(smtp_tls(&host)?)),
            &user,
            &password,
        )
        .build()
    } else if field_text(server, "smtpStartTls") == "Y" {
        creds(
            lettre::SmtpTransport::builder_dangerous(&host)
                .port(port)
                .tls(Tls::Opportunistic(smtp_tls(&host)?)),
            &user,
            &password,
        )
        .build()
    } else {
        creds(
            lettre::SmtpTransport::builder_dangerous(&host).port(port),
            &user,
            &password,
        )
        .build()
    };
    lettre::Transport::send(&transport, &message)
        .map(|_| ())
        .map_err(|err| err.to_string())
}

fn creds(
    builder: lettre::transport::smtp::SmtpTransportBuilder,
    user: &str,
    password: &str,
) -> lettre::transport::smtp::SmtpTransportBuilder {
    if user.is_empty() {
        builder
    } else {
        builder.credentials(lettre::transport::smtp::authentication::Credentials::new(
            user.to_string(),
            password.to_string(),
        ))
    }
}

fn build_lettre(letter: &Letter) -> Result<lettre::Message, String> {
    let from = mailbox(&letter.from_address, &letter.from_name)?;
    let mut builder = lettre::Message::builder()
        .from(from)
        .message_id(Some(letter.message_id.clone()));
    // As commons-email, which sets the header only for a subject that is not empty.
    if !letter.subject.is_empty() {
        builder = builder.subject(letter.subject.clone());
    }
    let mut recipients = Vec::new();
    for address in letter.to.iter().chain(letter.cc.iter()).chain(letter.bcc.iter()) {
        let parsed = mailbox(address, "")?;
        recipients.push(parsed.email.clone());
    }
    for address in &letter.to {
        builder = builder.to(mailbox(address, "")?);
    }
    for address in &letter.cc {
        builder = builder.cc(mailbox(address, "")?);
    }
    for address in &letter.bcc {
        builder = builder.bcc(mailbox(address, "")?);
    }
    for address in &letter.reply_to {
        builder = builder.reply_to(mailbox(address, "")?);
    }
    if !letter.bounce.is_empty() {
        let sender = letter
            .bounce
            .trim()
            .parse()
            .map_err(|err| format!("bounce address {} is not valid: {err}", letter.bounce))?;
        let envelope = lettre::address::Envelope::new(Some(sender), recipients).map_err(|err| err.to_string())?;
        builder = builder.envelope(envelope);
    }
    let message = if letter.attachments.is_empty() {
        finish_body(builder, letter)?
    } else {
        builder
            .multipart(with_attachments(letter)?)
            .map_err(|err| err.to_string())?
    };
    Ok(message)
}

fn finish_body(builder: lettre::message::MessageBuilder, letter: &Letter) -> Result<lettre::Message, String> {
    if !letter.body.is_empty() && !letter.body_text.is_empty() {
        return builder
            .multipart(lettre::message::MultiPart::alternative_plain_html(
                letter.body_text.clone(),
                letter.body.clone(),
            ))
            .map_err(|err| err.to_string());
    }
    if !letter.body.is_empty() {
        return builder
            .header(lettre::message::header::ContentType::TEXT_HTML)
            .body(letter.body.clone())
            .map_err(|err| err.to_string());
    }
    builder
        .header(lettre::message::header::ContentType::TEXT_PLAIN)
        .body(letter.body_text.clone())
        .map_err(|err| err.to_string())
}

fn with_attachments(letter: &Letter) -> Result<lettre::message::MultiPart, String> {
    let mut mixed = if !letter.body.is_empty() && !letter.body_text.is_empty() {
        lettre::message::MultiPart::mixed().multipart(lettre::message::MultiPart::alternative_plain_html(
            letter.body_text.clone(),
            letter.body.clone(),
        ))
    } else if !letter.body.is_empty() {
        lettre::message::MultiPart::mixed().singlepart(lettre::message::SinglePart::html(letter.body.clone()))
    } else {
        lettre::message::MultiPart::mixed().singlepart(lettre::message::SinglePart::plain(letter.body_text.clone()))
    };
    for attachment in &letter.attachments {
        let part = lettre::message::Attachment::new(attachment.filename.clone())
            .body(attachment.bytes.clone(), content_type(&attachment.content_type));
        mixed = mixed.singlepart(part);
    }
    Ok(mixed)
}

fn content_type(name: &str) -> lettre::message::header::ContentType {
    lettre::message::header::ContentType::parse(name).unwrap_or(lettre::message::header::ContentType::TEXT_PLAIN)
}

/// An address as Java's `addTo` takes it: `a@example.com` or `Name <a@example.com>`. A `name` given separately wins.
fn mailbox(address: &str, name: &str) -> Result<lettre::message::Mailbox, String> {
    let address = address.trim();
    let parsed: lettre::message::Mailbox = address
        .parse()
        .map_err(|err| format!("email address {address} is not valid: {err}"))?;
    let name = name.trim();
    if name.is_empty() {
        Ok(parsed)
    } else {
        Ok(lettre::message::Mailbox::new(Some(name.to_string()), parsed.email))
    }
}

/// A random `<hex@host>` Message-ID, the host the domain of the from address.
fn new_message_id(from: &str) -> Result<String, String> {
    let mut bytes = [0u8; 12];
    getrandom::fill(&mut bytes).map_err(|err| format!("could not make a Message-ID: {err}"))?;
    let host = mailbox(from, "")
        .map(|parsed| parsed.email.domain().to_string())
        .ok()
        .filter(|host| !host.is_empty())
        .unwrap_or_else(|| "localhost".to_string());
    Ok(format!("<{}@{host}>", hex_encode(&bytes)))
}

#[cfg(test)]
thread_local! {
    static SINK: RefCell<Option<Rc<RefCell<Vec<Letter>>>>> = const { RefCell::new(None) };
    static FAKE: RefCell<Option<Vec<Inbound>>> = const { RefCell::new(None) };
    /// The ids the fake store was asked to delete.
    static FAKE_DELETED: RefCell<Vec<String>> = const { RefCell::new(Vec::new()) };
}

/// Keep `letter` in the installed test sink, if any: this thread's, else the shared one.
#[cfg(test)]
fn push_sink(letter: &Letter) -> bool {
    let kept = SINK.with(|slot| {
        let Some(sink) = slot.borrow().clone() else {
            return false;
        };
        sink.borrow_mut().push(letter.clone());
        true
    });
    kept || push_shared_sink(letter)
}

/// The sink of [`SharedSinkGuard`], for a send on a service worker thread.
#[cfg(test)]
static SHARED_SINK: std::sync::Mutex<Option<Arc<std::sync::Mutex<Vec<Letter>>>>> = std::sync::Mutex::new(None);

#[cfg(test)]
fn push_shared_sink(letter: &Letter) -> bool {
    let shared = lock(&SHARED_SINK).clone();
    let Some(shared) = shared else {
        return false;
    };
    lock(&shared).push(letter.clone());
    true
}

/// The test sink: while installed, a send on this thread is kept here instead of going to SMTP.
#[cfg(test)]
pub(crate) struct SinkGuard {
    previous: Option<Rc<RefCell<Vec<Letter>>>>,
    current: Rc<RefCell<Vec<Letter>>>,
}

/// A mail the sink kept, with the MIME text lettre would send.
#[cfg(test)]
#[derive(Clone, Debug)]
pub(crate) struct SentMail {
    pub subject: String,
    pub to: Vec<String>,
    pub body: String,
    pub body_text: String,
    /// File name, content type and bytes.
    pub attachments: Vec<(String, String, Vec<u8>)>,
    pub mime: String,
}

#[cfg(test)]
impl SinkGuard {
    pub(crate) fn install() -> Self {
        let current = Rc::new(RefCell::new(Vec::new()));
        let previous = SINK.with(|slot| slot.borrow_mut().replace(Rc::clone(&current)));
        Self { previous, current }
    }

    fn messages(&self) -> Vec<Letter> {
        self.current.borrow().clone()
    }

    pub(crate) fn sent(&self) -> Vec<SentMail> {
        self.messages().into_iter().map(sent_mail).collect()
    }
}

#[cfg(test)]
fn sent_mail(letter: Letter) -> SentMail {
    SentMail {
        mime: build_lettre(&letter)
            .map(|message| String::from_utf8_lossy(&message.formatted()).into_owned())
            .unwrap_or_else(|err| format!("not a message: {err}")),
        subject: letter.subject,
        to: letter.to,
        body: letter.body,
        body_text: letter.body_text,
        attachments: letter
            .attachments
            .into_iter()
            .map(|part| (part.filename, part.content_type, part.bytes))
            .collect(),
    }
}

/// A test sink for sends on any thread that has no [`SinkGuard`] of its own, so mail an async
/// service sends from a worker is kept. One is installed at a time; a second waits.
#[cfg(test)]
pub(crate) struct SharedSinkGuard {
    current: Arc<std::sync::Mutex<Vec<Letter>>>,
    _turn: std::sync::MutexGuard<'static, ()>,
}

#[cfg(test)]
impl SharedSinkGuard {
    pub(crate) fn install() -> Self {
        static TURN: std::sync::Mutex<()> = std::sync::Mutex::new(());
        let turn = lock(&TURN);
        let current = Arc::new(std::sync::Mutex::new(Vec::new()));
        *lock(&SHARED_SINK) = Some(Arc::clone(&current));
        Self { current, _turn: turn }
    }

    /// The mail kept so far to `address`.
    pub(crate) fn sent_to(&self, address: &str) -> Vec<SentMail> {
        let letters = lock(&self.current).clone();
        letters
            .into_iter()
            .filter(|letter| letter.to.iter().any(|to| to == address))
            .map(sent_mail)
            .collect()
    }

    /// Wait up to `timeout` for mail to `address`, as a worker sends it after the caller returns.
    pub(crate) fn wait_for(&self, address: &str, timeout: Duration) -> Vec<SentMail> {
        let deadline = std::time::Instant::now() + timeout;
        loop {
            let sent = self.sent_to(address);
            if !sent.is_empty() || std::time::Instant::now() > deadline {
                return sent;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
    }
}

#[cfg(test)]
impl Drop for SharedSinkGuard {
    fn drop(&mut self) {
        *lock(&SHARED_SINK) = None;
    }
}

#[cfg(test)]
impl Drop for SinkGuard {
    fn drop(&mut self) {
        SINK.with(|slot| *slot.borrow_mut() = self.previous.take());
    }
}

pub(super) fn poll_server(
    parameters: &BTreeMap<String, Value>,
    facade: &EntityFacade,
) -> Result<BTreeMap<String, Value>, LibError> {
    if facade.sim_active() {
        return fail("Email poll is disabled in LLM sim session");
    }
    let server_id = param_text(parameters, "emailServerId");
    let Some(server) = find_one(facade, SERVER, "emailServerId", &server_id)? else {
        return fail(format!("No EmailServer found for ID [{server_id}]"));
    };
    let host = field_text(&server, "storeHost");
    let user = field_text(&server, "mailUsername");
    let password = field_text(&server, "mailPassword");
    if host.is_empty() {
        note_error(format!("EmailServer [{server_id}] has no storeHost"));
    }
    if user.is_empty() {
        note_error(format!("EmailServer [{server_id}] has no mailUsername"));
    }
    if password.is_empty() {
        note_error(format!("EmailServer [{server_id}] has no mailPassword"));
    }
    if has_error() {
        return Ok(BTreeMap::new());
    }
    if let Err(text) = require_host(&host) {
        return fail(text);
    }
    let protocol = {
        let given = field_text(&server, "storeProtocol");
        if given.is_empty() {
            "imaps".to_string()
        } else {
            given
        }
    };
    let port = match parse_port(&field_text(&server, "storePort"), 993) {
        Ok(port) => port,
        Err(text) => return fail(text),
    };
    let folder = {
        let given = field_text(&server, "storeFolder");
        if given.is_empty() {
            "INBOX".to_string()
        } else {
            given
        }
    };
    let settings = StoreSettings {
        host,
        port,
        user,
        password,
        protocol,
        folder,
        skip_seen: field_text(&server, "storeSkipSeen") == "Y",
        mark_seen: field_text(&server, "storeMarkSeen") == "Y",
        delete: field_text(&server, "storeDelete") == "Y",
    };
    let mut store = match open_store(&settings) {
        Ok(store) => store,
        Err(text) => return fail(text),
    };
    let ids = match store.list_ids() {
        Ok(ids) => ids,
        Err(text) => return fail(text),
    };
    // One message at a time, as Java's poll: fetched, run through the rules, then marked or deleted. Each message's
    // rules run in their own transaction, and the message is marked or deleted only once that has committed, so a later
    // failure or the poll's timeout cannot roll back a message already gone from the server. A message whose rules fail
    // is rolled back and left as it is, so the next poll sees it again. The errors are reported once all are done, so
    // one message's errors do not stop the services the next one's rules call.
    let mut errors = Vec::new();
    for id in ids {
        let message = match store.fetch(&id) {
            Ok(message) => message,
            Err(text) => {
                errors.push(format!("Error reading message {id} from {}: {text}", settings.host));
                break;
            }
        };
        if settings.skip_seen && message.seen {
            continue;
        }
        if let Err(text) = receive(facade, &message, &server_id) {
            tracing::error!("email ECA rules failed for message {id} of {server_id}, left on the server: {text}");
            errors.push(text);
            continue;
        }
        if settings.mark_seen {
            if let Err(err) = store.mark_seen(&id) {
                errors.push(err);
            }
        }
        if settings.delete {
            if let Err(err) = store.delete(&id) {
                errors.push(err);
            }
        }
    }
    if let Err(err) = store.close() {
        errors.push(err);
    }
    for text in errors {
        note_error(text);
    }
    Ok(BTreeMap::new())
}

/// Runs the email ECA rules for one message in a new transaction. A rule that fails, or an error a rule leaves in the
/// message facade, rolls it back; the errors are taken out of the facade and returned.
fn receive(facade: &EntityFacade, message: &Inbound, server_id: &str) -> Result<(), String> {
    let mark = ExecutionContext::with_current(|ctx| ctx.message().error_mark()).unwrap_or_default();
    force_new(facade, || {
        let ran = crate::service::run_email_rules(&eca_fields(message, server_id));
        let mut errors =
            ExecutionContext::with_current(|ctx| ctx.message().take_errors_since(mark)).unwrap_or_default();
        if let Err(err) = ran {
            if !errors.iter().any(|text| text == err.message()) {
                errors.insert(0, err.message().to_string());
            }
        }
        if errors.is_empty() {
            Ok(())
        } else {
            Err(LibError::new(errors.join("\n")))
        }
    })
    .map_err(|err| err.message().to_string())
}

struct StoreSettings {
    host: String,
    port: u16,
    user: String,
    password: String,
    protocol: String,
    folder: String,
    skip_seen: bool,
    mark_seen: bool,
    delete: bool,
}

fn eca_fields(message: &Inbound, server_id: &str) -> BTreeMap<String, Value> {
    let mut fields = IndexMap::new();
    fields.insert("toList".into(), string_list(&message.to));
    fields.insert("ccList".into(), string_list(&message.cc));
    fields.insert("bccList".into(), string_list(&message.bcc));
    fields.insert(
        "from".into(),
        if message.from.is_empty() {
            Value::Null
        } else {
            Value::string(&message.from)
        },
    );
    fields.insert("subject".into(), Value::string(&message.subject));
    fields.insert("sentDate".into(), message.sent.clone());
    fields.insert("receivedDate".into(), message.received.clone());
    let mut flags = IndexMap::new();
    flags.insert("answered".into(), Value::Bool(message.flags.answered));
    flags.insert("deleted".into(), Value::Bool(message.flags.deleted));
    flags.insert("draft".into(), Value::Bool(message.flags.draft));
    flags.insert("flagged".into(), Value::Bool(message.flags.flagged));
    flags.insert("recent".into(), Value::Bool(message.flags.recent));
    flags.insert("seen".into(), Value::Bool(message.seen));
    // Java puts the MimeMessage here; this is the part of it the rules read: its id, size and raw text.
    let mut raw = IndexMap::new();
    raw.insert(
        "messageId".into(),
        Value::string(header_one(&message.headers, "message-id")),
    );
    raw.insert("subject".into(), Value::string(&message.subject));
    raw.insert(
        "size".into(),
        Value::I64(i64::try_from(message.raw.len()).unwrap_or(i64::MAX)),
    );
    raw.insert("raw".into(), Value::string(String::from_utf8_lossy(&message.raw)));
    let mut out = BTreeMap::new();
    out.insert("emailServerId".into(), Value::string(server_id));
    out.insert("message".into(), Value::Map(raw));
    out.insert("fields".into(), Value::Map(fields));
    out.insert("headers".into(), Value::Map(message.headers.clone()));
    out.insert("bodyPartList".into(), Value::List(message.parts.clone()));
    out.insert("flags".into(), Value::Map(flags));
    out
}

fn string_list(values: &[String]) -> Value {
    Value::List(values.iter().cloned().map(Value::string).collect())
}

/// IMAP flags of a message other than seen.
#[derive(Clone, Copy, Debug, Default)]
struct Flags {
    answered: bool,
    deleted: bool,
    draft: bool,
    flagged: bool,
    recent: bool,
}

#[derive(Clone, Debug)]
struct Inbound {
    id: String,
    seen: bool,
    flags: Flags,
    /// The message as it came from the server.
    raw: Vec<u8>,
    headers: IndexMap<String, Value>,
    parts: Vec<Value>,
    subject: String,
    from: String,
    to: Vec<String>,
    cc: Vec<String>,
    bcc: Vec<String>,
    sent: Value,
    received: Value,
}

impl Inbound {
    fn blank() -> Self {
        Self {
            id: String::new(),
            seen: false,
            flags: Flags::default(),
            raw: Vec::new(),
            headers: IndexMap::new(),
            parts: Vec::new(),
            subject: String::new(),
            from: String::new(),
            to: Vec::new(),
            cc: Vec::new(),
            bcc: Vec::new(),
            sent: Value::Null,
            received: Value::Null,
        }
    }
}

trait Mailbox {
    /// The ids of the messages to look at, in server order.
    fn list_ids(&mut self) -> Result<Vec<String>, String>;
    /// One message, with its flags.
    fn fetch(&mut self, id: &str) -> Result<Inbound, String>;
    fn mark_seen(&mut self, id: &str) -> Result<(), String>;
    fn delete(&mut self, id: &str) -> Result<(), String>;
    fn close(&mut self) -> Result<(), String>;
}

#[cfg(test)]
struct FakeBox {
    messages: Vec<Inbound>,
}

#[cfg(test)]
impl Mailbox for FakeBox {
    fn list_ids(&mut self) -> Result<Vec<String>, String> {
        Ok(self.messages.iter().map(|message| message.id.clone()).collect())
    }

    fn fetch(&mut self, id: &str) -> Result<Inbound, String> {
        self.messages
            .iter()
            .find(|message| message.id == id)
            .cloned()
            .ok_or_else(|| format!("no message {id}"))
    }

    fn mark_seen(&mut self, id: &str) -> Result<(), String> {
        if let Some(message) = self.messages.iter_mut().find(|message| message.id == id) {
            message.seen = true;
        }
        Ok(())
    }

    fn delete(&mut self, id: &str) -> Result<(), String> {
        self.messages.retain(|message| message.id != id);
        FAKE_DELETED.with(|deleted| deleted.borrow_mut().push(id.to_string()));
        Ok(())
    }

    fn close(&mut self) -> Result<(), String> {
        Ok(())
    }
}

#[cfg(test)]
struct FakeGuard {
    previous: Option<Vec<Inbound>>,
}

#[cfg(test)]
impl FakeGuard {
    fn install(messages: Vec<Inbound>) -> Self {
        let previous = FAKE.with(|slot| slot.borrow_mut().replace(messages));
        Self { previous }
    }
}

#[cfg(test)]
impl Drop for FakeGuard {
    fn drop(&mut self) {
        FAKE.with(|slot| *slot.borrow_mut() = self.previous.take());
    }
}

/// The fake store a test installed on this thread, if any.
#[cfg(test)]
fn fake_store() -> Option<Box<dyn Mailbox>> {
    let messages = FAKE.with(|slot| slot.borrow().clone())?;
    Some(Box::new(FakeBox { messages }))
}

fn open_store(settings: &StoreSettings) -> Result<Box<dyn Mailbox>, String> {
    #[cfg(test)]
    if let Some(fake) = fake_store() {
        return Ok(fake);
    }
    match settings.protocol.as_str() {
        "pop3" | "pop3s" => Ok(Box::new(Pop3::open(settings)?)),
        "imap" | "imaps" => Ok(Box::new(ImapBox::open(settings)?)),
        other => Err(format!("storeProtocol {other} is not supported")),
    }
}

fn use_tls(protocol: &str, port: u16) -> bool {
    matches!(protocol, "imaps" | "pop3s") || port == 993 || port == 995
}

struct ImapBox {
    session: imap::Session<Net>,
    closed: bool,
    deleted: bool,
    skip_seen: bool,
}

impl ImapBox {
    fn open(settings: &StoreSettings) -> Result<Self, String> {
        let net = connect_net(
            &settings.host,
            settings.port,
            use_tls(&settings.protocol, settings.port),
        )?;
        let mut client = imap::Client::new(net);
        client.read_greeting().map_err(|err| err.to_string())?;
        let mut session = client
            .login(&settings.user, &settings.password)
            .map_err(|(err, _)| err.to_string())?;
        session
            .select(&settings.folder)
            .map_err(|_| format!("No {} folder found", settings.folder))?;
        Ok(Self {
            session,
            closed: false,
            deleted: false,
            skip_seen: settings.skip_seen,
        })
    }
}

impl Mailbox for ImapBox {
    fn list_ids(&mut self) -> Result<Vec<String>, String> {
        let query = if self.skip_seen {
            "UNSEEN NOT DELETED"
        } else {
            "NOT DELETED"
        };
        let mut ids: Vec<u32> = self
            .session
            .search(query)
            .map_err(|err| err.to_string())?
            .into_iter()
            .collect();
        ids.sort_unstable();
        Ok(ids.into_iter().map(|id| id.to_string()).collect())
    }

    fn fetch(&mut self, id: &str) -> Result<Inbound, String> {
        let fetches = self
            .session
            .fetch(id, "BODY.PEEK[] FLAGS INTERNALDATE")
            .map_err(|err| err.to_string())?;
        let fetch = fetches
            .iter()
            .next()
            .ok_or_else(|| format!("message {id} is not on the server"))?;
        let mut message = parse_rfc822(fetch.body().unwrap_or_default());
        message.id = id.to_string();
        for flag in fetch.flags() {
            match flag {
                imap::types::Flag::Seen => message.seen = true,
                imap::types::Flag::Answered => message.flags.answered = true,
                imap::types::Flag::Deleted => message.flags.deleted = true,
                imap::types::Flag::Draft => message.flags.draft = true,
                imap::types::Flag::Flagged => message.flags.flagged = true,
                imap::types::Flag::Recent => message.flags.recent = true,
                _ => {}
            }
        }
        if let Some(date) = fetch.internal_date() {
            message.received = stamp_millis(date.timestamp_millis());
        }
        Ok(message)
    }

    fn mark_seen(&mut self, id: &str) -> Result<(), String> {
        self.session
            .store(id, "+FLAGS (\\Seen)")
            .map(|_| ())
            .map_err(|err| err.to_string())
    }

    fn delete(&mut self, id: &str) -> Result<(), String> {
        self.session
            .store(id, "+FLAGS (\\Deleted)")
            .map(|_| ())
            .map_err(|err| err.to_string())?;
        self.deleted = true;
        Ok(())
    }

    fn close(&mut self) -> Result<(), String> {
        if self.closed {
            return Ok(());
        }
        self.closed = true;
        // Only when this poll flagged a message: Java's close(true) also expunges what another client flagged and
        // could still undelete, on a server that is not set to delete.
        if self.deleted {
            self.session.expunge().map_err(|err| err.to_string())?;
        }
        self.session.logout().map_err(|err| err.to_string())
    }
}

impl Drop for ImapBox {
    fn drop(&mut self) {
        if !self.closed {
            if self.deleted {
                let _ = self.session.expunge();
            }
            let _ = self.session.logout();
            self.closed = true;
        }
    }
}

struct Pop3 {
    reader: BufReader<Net>,
    closed: bool,
}

impl Pop3 {
    fn open(settings: &StoreSettings) -> Result<Self, String> {
        if settings.folder != "INBOX" {
            return Err(format!("No {} folder found", settings.folder));
        }
        // They go into the USER and PASS lines as they are.
        if [&settings.user, &settings.password]
            .iter()
            .any(|text| text.contains(['\r', '\n']))
        {
            return Err("the POP3 username or password has a line break".to_string());
        }
        let net = connect_net(
            &settings.host,
            settings.port,
            use_tls(&settings.protocol, settings.port),
        )?;
        let mut reader = BufReader::new(net);
        let greeting = read_line(&mut reader)?;
        expect_ok(&greeting)?;
        let mut client = Self { reader, closed: false };
        client.command(&format!("USER {}", settings.user))?;
        client.command(&format!("PASS {}", settings.password))?;
        Ok(client)
    }

    fn command(&mut self, line: &str) -> Result<String, String> {
        write!(self.reader.get_mut(), "{line}\r\n").map_err(|err| err.to_string())?;
        self.reader.get_mut().flush().map_err(|err| err.to_string())?;
        let response = read_line(&mut self.reader)?;
        expect_ok(&response)?;
        Ok(response)
    }

    /// A multi-line answer up to the line with only a dot, as bytes: 8-bit mail is not text in any one charset.
    fn read_dot(&mut self) -> Result<Vec<u8>, String> {
        let mut raw = Vec::new();
        loop {
            let line = read_line_bytes(&mut self.reader)?;
            if line == b"." {
                break;
            }
            let line = line
                .strip_prefix(b".")
                .filter(|rest| rest.starts_with(b"."))
                .unwrap_or(&line);
            raw.extend_from_slice(line);
            raw.extend_from_slice(b"\r\n");
        }
        Ok(raw)
    }
}

impl Mailbox for Pop3 {
    fn list_ids(&mut self) -> Result<Vec<String>, String> {
        self.command("LIST")?;
        let listing = self.read_dot()?;
        Ok(String::from_utf8_lossy(&listing)
            .lines()
            .filter_map(|line| line.split_whitespace().next())
            .filter(|id| id.parse::<u32>().is_ok())
            .map(str::to_string)
            .collect())
    }

    fn fetch(&mut self, id: &str) -> Result<Inbound, String> {
        self.command(&format!("RETR {id}"))?;
        let bytes = self.read_dot()?;
        let mut message = parse_rfc822(&bytes);
        message.id = id.to_string();
        Ok(message)
    }

    fn mark_seen(&mut self, _id: &str) -> Result<(), String> {
        Ok(())
    }

    fn delete(&mut self, id: &str) -> Result<(), String> {
        self.command(&format!("DELE {id}")).map(|_| ())
    }

    /// `QUIT` is when a POP3 server removes the deleted messages, so its answer is read.
    fn close(&mut self) -> Result<(), String> {
        if self.closed {
            return Ok(());
        }
        self.closed = true;
        self.command("QUIT").map(|_| ())
    }
}

impl Drop for Pop3 {
    fn drop(&mut self) {
        if !self.closed {
            let _ = self.reader.get_mut().write_all(b"QUIT\r\n");
            let _ = self.reader.get_mut().flush();
            self.closed = true;
        }
    }
}

enum Net {
    Plain(TcpStream),
    Tls(Box<rustls::StreamOwned<rustls::ClientConnection, TcpStream>>),
}

impl Read for Net {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        match self {
            Net::Plain(stream) => stream.read(buf),
            Net::Tls(stream) => stream.read(buf),
        }
    }
}

impl Write for Net {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        match self {
            Net::Plain(stream) => stream.write(buf),
            Net::Tls(stream) => stream.write(buf),
        }
    }

    fn flush(&mut self) -> std::io::Result<()> {
        match self {
            Net::Plain(stream) => stream.flush(),
            Net::Tls(stream) => stream.flush(),
        }
    }
}

fn connect_net(host: &str, port: u16, tls: bool) -> Result<Net, String> {
    // A blackholed host fails in 30 seconds instead of the system default of minutes.
    let address = std::net::ToSocketAddrs::to_socket_addrs(&(host, port))
        .map_err(|err| format!("could not resolve {host}:{port}: {err}"))?
        .next()
        .ok_or_else(|| format!("could not resolve {host}:{port}"))?;
    let tcp = TcpStream::connect_timeout(&address, Duration::from_secs(30))
        .map_err(|err| format!("could not connect to {host}:{port}: {err}"))?;
    let _ = tcp.set_read_timeout(Some(Duration::from_secs(60)));
    let _ = tcp.set_write_timeout(Some(Duration::from_secs(60)));
    if !tls {
        return Ok(Net::Plain(tcp));
    }
    let name = rustls::pki_types::ServerName::try_from(host.to_string())
        .map_err(|_| format!("email host {host} is not a DNS name"))?;
    let config = tls_config()?;
    let conn = rustls::ClientConnection::new(config, name).map_err(|err| err.to_string())?;
    Ok(Net::Tls(Box::new(rustls::StreamOwned::new(conn, tcp))))
}

/// The certificates of the PEM file `email_tls_ca_file` names, for a mail server with its own CA. Empty when unset.
fn extra_ca_certificates() -> Result<Vec<rustls::pki_types::CertificateDer<'static>>, String> {
    use rustls::pki_types::pem::PemObject;
    let location = moqui_lib::env::get_prop_or_env("email_tls_ca_file");
    if location.trim().is_empty() {
        return Ok(Vec::new());
    }
    let pem =
        std::fs::read(location.trim()).map_err(|err| format!("could not read email_tls_ca_file {location}: {err}"))?;
    rustls::pki_types::CertificateDer::pem_slice_iter(&pem)
        .collect::<Result<Vec<_>, _>>()
        .map_err(|err| format!("email_tls_ca_file {location} is not PEM certificates: {err}"))
}

fn smtp_tls(host: &str) -> Result<lettre::transport::smtp::client::TlsParameters, String> {
    let mut builder = lettre::transport::smtp::client::TlsParameters::builder(host.to_string());
    for der in extra_ca_certificates()? {
        let certificate =
            lettre::transport::smtp::client::Certificate::from_der(der.to_vec()).map_err(|err| err.to_string())?;
        builder = builder.add_root_certificate(certificate);
    }
    builder.build().map_err(|err| err.to_string())
}

fn tls_config() -> Result<Arc<rustls::ClientConfig>, String> {
    let mut roots = rustls::RootCertStore::from_iter(webpki_roots::TLS_SERVER_ROOTS.iter().cloned());
    for der in extra_ca_certificates()? {
        roots.add(der).map_err(|err| format!("email_tls_ca_file: {err}"))?;
    }
    let provider = Arc::new(rustls::crypto::aws_lc_rs::default_provider());
    let config = rustls::ClientConfig::builder_with_provider(provider)
        .with_safe_default_protocol_versions()
        .map_err(|err| err.to_string())?
        .with_root_certificates(roots)
        .with_no_client_auth();
    Ok(Arc::new(config))
}

/// A status line, which is ASCII; any other byte is replaced.
fn read_line(reader: &mut impl BufRead) -> Result<String, String> {
    read_line_bytes(reader).map(|line| String::from_utf8_lossy(&line).into_owned())
}

fn read_line_bytes(reader: &mut impl BufRead) -> Result<Vec<u8>, String> {
    let mut line = Vec::new();
    let read = reader.read_until(b'\n', &mut line).map_err(|err| err.to_string())?;
    if read == 0 {
        return Err("mail server closed the connection".to_string());
    }
    if line.ends_with(b"\n") {
        line.pop();
    }
    if line.ends_with(b"\r") {
        line.pop();
    }
    Ok(line)
}

fn expect_ok(line: &str) -> Result<(), String> {
    if line.starts_with("+OK") {
        Ok(())
    } else {
        Err(format!("mail server rejected the command: {line}"))
    }
}

/// A message as the email-ECA rules see it, as Java's EmailEcaRule makes it from a MimeMessage: the decoded subject,
/// the addresses as `Name <address>`, the header values as they were written (by lower-case name, a list when one
/// repeats), and the leaf parts of every multipart in order, text in its charset as `contentText` and anything else as
/// `contentBytes`.
fn parse_rfc822(bytes: &[u8]) -> Inbound {
    let mut message = Inbound::blank();
    message.raw = bytes.to_vec();
    let Some(parsed) = mail_parser::MessageParser::default().parse(bytes) else {
        return message;
    };
    for header in parsed.headers() {
        push_header(&mut message.headers, header.name(), raw_header_value(bytes, header));
    }
    message.subject = parsed.subject().unwrap_or_default().to_string();
    message.from = parsed
        .from()
        .and_then(mail_parser::Address::first)
        .map(address_text)
        .unwrap_or_default();
    let list = |address: Option<&mail_parser::Address<'_>>| -> Vec<String> {
        address
            .map(|address| address.iter().map(address_text).collect())
            .unwrap_or_default()
    };
    message.to = list(parsed.to());
    message.cc = list(parsed.cc());
    message.bcc = list(parsed.bcc());
    message.sent = parsed.date().map_or(Value::Null, |date| {
        stamp_millis(date.to_timestamp().saturating_mul(1000))
    });
    leaf_parts(&parsed, bytes, 0, &mut message.parts, 0);
    message
}

/// A header's value as written, unfolded: what JavaMail's `Header.getValue` gives.
fn raw_header_value(bytes: &[u8], header: &mail_parser::Header<'_>) -> String {
    let start = usize::try_from(header.offset_start).unwrap_or(0).min(bytes.len());
    let end = usize::try_from(header.offset_end)
        .unwrap_or(0)
        .clamp(start, bytes.len());
    let text = String::from_utf8_lossy(&bytes[start..end]);
    let mut out = String::new();
    for (index, line) in text.split('\n').enumerate() {
        let line = line.trim_end_matches('\r');
        if index == 0 {
            out.push_str(line.trim());
        } else if !line.trim().is_empty() {
            out.push(' ');
            out.push_str(line.trim());
        }
    }
    out
}

/// `InternetAddress.toString`: the name and the address in angle brackets, or the address alone.
fn address_text(addr: &mail_parser::Addr<'_>) -> String {
    let address = addr.address().unwrap_or_default();
    match addr.name().filter(|name| !name.is_empty()) {
        Some(name) if name.contains([',', '"', '<', '>', '@', ';', ':']) => {
            format!("\"{}\" <{address}>", name.replace('"', "\\\""))
        }
        Some(name) => format!("{name} <{address}>"),
        None => address.to_string(),
    }
}

/// The parts with content under `id`, depth first, as Java's `makeBodyPartList`. A nested message is skipped, as in Java.
fn leaf_parts(message: &mail_parser::Message<'_>, raw: &[u8], id: u32, out: &mut Vec<Value>, depth: usize) {
    use mail_parser::{MimeHeaders, PartType};
    let Some(part) = message.part(id) else {
        return;
    };
    if depth > 32 {
        return;
    }
    match &part.body {
        PartType::Multipart(children) => {
            for child in children {
                leaf_parts(message, raw, *child, out, depth + 1);
            }
            return;
        }
        PartType::Message(_) => return,
        _ => {}
    }
    let mut map = IndexMap::new();
    let content_type = part
        .headers()
        .iter()
        .find(|header| header.name().eq_ignore_ascii_case("content-type"))
        .map_or_else(|| "text/plain".to_string(), |header| raw_header_value(raw, header));
    map.insert("contentType".into(), Value::string(content_type));
    if let Some(name) = part.attachment_name() {
        map.insert("filename".into(), Value::string(name));
    }
    if let Some(disposition) = part.content_disposition() {
        map.insert(
            "disposition".into(),
            Value::string(disposition.ctype().to_ascii_lowercase()),
        );
    }
    match &part.body {
        PartType::Text(text) | PartType::Html(text) => {
            map.insert("contentText".into(), Value::string(text.as_ref()));
        }
        PartType::Binary(bytes) | PartType::InlineBinary(bytes) => {
            map.insert("contentBytes".into(), Value::Bytes(bytes.to_vec()));
        }
        PartType::Multipart(_) | PartType::Message(_) => {}
    }
    out.push(Value::Map(map));
}

fn push_header(headers: &mut IndexMap<String, Value>, name: &str, value: String) {
    let key = name.to_ascii_lowercase();
    if let Some(existing) = headers.shift_remove(&key) {
        let list = match existing {
            Value::List(mut items) => {
                items.push(Value::string(value));
                items
            }
            other => vec![other, Value::string(value)],
        };
        headers.insert(key, Value::List(list));
    } else {
        headers.insert(key, Value::string(value));
    }
}

fn header_one(headers: &IndexMap<String, Value>, name: &str) -> String {
    match headers.get(name) {
        Some(Value::List(items)) => items.first().map(to_plain_string).unwrap_or_default(),
        Some(value) => to_plain_string(value),
        None => String::new(),
    }
}

fn find_one(facade: &EntityFacade, entity: &str, field: &str, value: &str) -> Result<Option<EntityValue>, LibError> {
    if value.is_empty() {
        return Ok(None);
    }
    facade.find(entity)?.eq(field, Value::string(value)).one()
}

fn body_parameters(parameters: &BTreeMap<String, Value>) -> Option<&IndexMap<String, Value>> {
    match parameters.get("bodyParameters") {
        Some(Value::Map(map)) => Some(map),
        _ => None,
    }
}

fn set_text(row: &mut EntityValue, name: &str, value: &str) {
    if !value.is_empty() {
        row.set(name, Value::string(value));
    }
}

fn merge_lists(given: &str, template: &str) -> String {
    if !given.is_empty() {
        if template.is_empty() {
            given.to_string()
        } else {
            format!("{given},{template}")
        }
    } else {
        template.to_string()
    }
}

/// The addresses of a comma separated list. A comma inside quotes or `<>` does not split, where Java splits on
/// every comma and so breaks `"Doe, Jane" <jane@example.com>` in two; an empty item is left out.
fn split_addresses(value: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut current = String::new();
    let mut angle = 0i32;
    let mut quote = false;
    for ch in value.chars() {
        match ch {
            '"' => quote = !quote,
            '<' if !quote => angle += 1,
            '>' if !quote && angle > 0 => angle -= 1,
            ',' if !quote && angle == 0 => {
                let trimmed = current.trim().to_string();
                if !trimmed.is_empty() {
                    out.push(trimmed);
                }
                current.clear();
                continue;
            }
            _ => {}
        }
        current.push(ch);
    }
    let trimmed = current.trim().to_string();
    if !trimmed.is_empty() {
        out.push(trimmed);
    }
    out
}

fn parse_port(text: &str, default: u16) -> Result<u16, String> {
    if text.trim().is_empty() {
        return Ok(default);
    }
    text.trim().parse().map_err(|_| format!("port {text} is not a number"))
}

fn ids(email_message_id: &str, message_id: &str) -> BTreeMap<String, Value> {
    let mut out = BTreeMap::new();
    if !email_message_id.is_empty() {
        out.insert("emailMessageId".into(), Value::string(email_message_id));
    }
    if !message_id.is_empty() {
        out.insert("messageId".into(), Value::string(message_id));
    }
    out
}

fn fail(text: impl Into<String>) -> Result<BTreeMap<String, Value>, LibError> {
    note_error(text);
    Ok(BTreeMap::new())
}

fn fail_with(text: impl Into<String>, email_message_id: &str) -> Result<BTreeMap<String, Value>, LibError> {
    note_error(text);
    Ok(ids(email_message_id, ""))
}

fn note_error(text: impl Into<String>) {
    let text = text.into();
    ExecutionContext::with_current(|ctx| ctx.message().add_error(text));
}

fn note_message(text: impl Into<String>) {
    let text = text.into();
    ExecutionContext::with_current(|ctx| ctx.message().add_message(text));
}

fn stamp_millis(millis: i64) -> Value {
    basic_convert(&Value::I64(millis), "java.sql.Timestamp").value
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use indexmap::IndexMap;

    use super::{header_one, host_allowed, parse_rfc822, read_line_bytes, FakeGuard, Inbound, SinkGuard, FAKE_DELETED};
    use crate::context::ExecutionContextFactory;
    use crate::entity::EntityFacade;
    use crate::screen::PDF_UNAVAILABLE;
    use crate::service::eca_rules;
    use moqui_lib::object::Value;

    /// The tests set `email_allowed_hosts`, a process property, so they share the property lock.
    fn lock_tests() -> std::sync::MutexGuard<'static, ()> {
        crate::user::session::cookie_property_lock()
    }

    fn fixture(name: &str) -> String {
        format!("{}/src/service/testdata/mail/{name}", env!("CARGO_MANIFEST_DIR"))
    }

    fn smtp_server(facade: &EntityFacade, context: &crate::context::ExecutionContext, id: &str) {
        seed(
            facade,
            context,
            "moqui.basic.email.EmailServer",
            &[
                ("emailServerId", id),
                ("smtpHost", "smtp.example.com"),
                ("smtpPort", "25"),
            ],
        );
    }

    /// `send#ScreenRenderEmail` requires a user, as in Java.
    fn log_in(facade: &EntityFacade, context: &crate::context::ExecutionContext) {
        seed(
            facade,
            context,
            "moqui.security.UserAccount",
            &[
                ("userId", "EMAILER"),
                ("username", "emailer"),
                ("emailAddress", "emailer@example.com"),
            ],
        );
        crate::user::internal_login_user("emailer").expect("login");
    }

    fn map(pairs: Vec<(&str, Value)>) -> Value {
        Value::Map(
            pairs
                .into_iter()
                .map(|(key, value)| (key.to_string(), value))
                .collect::<IndexMap<_, _>>(),
        )
    }

    struct HostList;

    impl HostList {
        fn set(value: &'static str) -> Self {
            moqui_lib::env::set_property("email_allowed_hosts", value);
            Self
        }
    }

    impl Drop for HostList {
        fn drop(&mut self) {
            moqui_lib::env::clear_property("email_allowed_hosts");
        }
    }

    fn entered() -> (EntityFacade, crate::context::ExecutionContext) {
        let facade = crate::testing::open();
        let context = ExecutionContextFactory::test_context_in(
            "",
            crate::service::ServicePools::idle(),
            crate::context::framework_dir(),
        );
        context.factory().set_entity(facade.clone());
        (facade, context)
    }

    fn seed(facade: &EntityFacade, context: &crate::context::ExecutionContext, entity: &str, fields: &[(&str, &str)]) {
        let _authz = context.disable_authz();
        let mut row = facade.make_value(entity).expect("make");
        for (name, value) in fields {
            row.set(*name, Value::string(*value));
        }
        row.create(facade).expect("create");
    }

    #[test]
    fn host_suffix_matches_on_a_dot_boundary() {
        assert!(host_allowed("smtp.example.com") || !moqui_lib::env::get_prop_or_env("email_allowed_hosts").is_empty());
        let _lock = lock_tests();
        let _list = HostList::set("example.com");
        assert!(host_allowed("smtp.example.com"));
        assert!(host_allowed("example.com"));
        assert!(!host_allowed("notexample.com"));
    }

    #[test]
    fn sink_receives_a_template_send_and_expands_the_subject() {
        let _lock = lock_tests();
        let (facade, context) = entered();
        let _entered = context.enter();
        crate::service::load_definitions().unwrap();
        seed(
            &facade,
            &context,
            "moqui.basic.email.EmailServer",
            &[
                ("emailServerId", "TEST_SMTP"),
                ("smtpHost", "smtp.example.com"),
                ("smtpPort", "25"),
            ],
        );
        seed(
            &facade,
            &context,
            "moqui.basic.email.EmailTemplate",
            &[
                ("emailTemplateId", "HELLO"),
                ("emailServerId", "TEST_SMTP"),
                ("fromAddress", "from@example.com"),
                ("subject", "Hello ${title}"),
            ],
        );
        let sink = SinkGuard::install();
        let mut body_parameters = indexmap::IndexMap::new();
        body_parameters.insert("title".into(), Value::string("there"));
        let mut params = std::collections::BTreeMap::new();
        params.insert("emailTemplateId".into(), Value::string("HELLO"));
        params.insert("toAddresses".into(), Value::string("to@example.com"));
        params.insert("body".into(), Value::string("Hi"));
        params.insert("bodyText".into(), Value::string("Hi"));
        params.insert("bodyParameters".into(), Value::Map(body_parameters));
        let out = crate::service::sync("org.moqui.impl.EmailServices.send#EmailTemplate", &params).expect("send");
        let errors = context.message().errors().join("\n");
        assert!(errors.is_empty(), "{errors}");
        let sent = sink.messages();
        assert_eq!(sent.len(), 1, "{errors}");
        assert_eq!(sent[0].subject, "Hello there");
        assert!(out.contains_key("emailMessageId"));
    }

    #[test]
    fn host_rejected_by_email_allowed_hosts_does_not_touch_the_sink() {
        let _lock = lock_tests();
        let _list = HostList::set("example.com");
        let (facade, context) = entered();
        let _entered = context.enter();
        crate::service::load_definitions().unwrap();
        seed(
            &facade,
            &context,
            "moqui.basic.email.EmailServer",
            &[("emailServerId", "TEST_BAD"), ("smtpHost", "notexample.com")],
        );
        seed(
            &facade,
            &context,
            "moqui.basic.email.EmailTemplate",
            &[
                ("emailTemplateId", "BADHOST"),
                ("emailServerId", "TEST_BAD"),
                ("fromAddress", "from@example.com"),
                ("subject", "No"),
            ],
        );
        let sink = SinkGuard::install();
        let mut params = std::collections::BTreeMap::new();
        params.insert("emailTemplateId".into(), Value::string("BADHOST"));
        params.insert("toAddresses".into(), Value::string("to@example.com"));
        params.insert("body".into(), Value::string("Hi"));
        params.insert("bodyText".into(), Value::string("Hi"));
        let _ = crate::service::sync("org.moqui.impl.EmailServices.send#EmailTemplate", &params);
        let errors = context.message().errors().join("\n");
        assert!(errors.contains("notexample.com"), "{errors}");
        assert!(sink.messages().is_empty(), "{errors}");
    }

    /// Gate 4: a screen body renders in html and text and goes out as multipart/alternative, with
    /// screen attachments (forEachIn, attachmentCondition, csv/text/html), a plain file, and an xsl-fo
    /// attachment that is skipped because no PDF engine is installed.
    #[test]
    fn screen_body_renders_html_and_text_and_sends_with_attachments() {
        let _lock = lock_tests();
        let _pdf = crate::screen::pdf_test_lock();
        crate::screen::clear_pdf_engine();
        let (facade, context) = entered();
        let _entered = context.enter();
        crate::service::load_definitions().unwrap();
        smtp_server(&facade, &context, "GATE_SMTP");
        seed(
            &facade,
            &context,
            "moqui.basic.email.EmailTemplate",
            &[
                ("emailTemplateId", "GATE_BODY"),
                ("emailServerId", "GATE_SMTP"),
                ("fromAddress", "from@example.com"),
                ("subject", "Report for ${name}"),
                ("bodyScreenLocation", &fixture("MailBody.xml")),
            ],
        );
        let report = fixture("MailReport.xml");
        let attachments: [&[(&str, &str)]; 5] = [
            &[
                ("fileName", "report-${rowName}.csv"),
                ("attachmentLocation", &report),
                ("screenRenderMode", "csv"),
                ("forEachIn", "reportRows"),
            ],
            &[
                ("fileName", "report.txt"),
                ("attachmentLocation", &report),
                ("screenRenderMode", "text"),
                ("attachmentCondition", "includeText"),
            ],
            &[
                ("fileName", "report.xml"),
                ("attachmentLocation", &report),
                ("screenRenderMode", "xml"),
                ("attachmentCondition", "!includeText"),
            ],
            &[
                ("fileName", "report.pdf"),
                ("attachmentLocation", &report),
                ("screenRenderMode", "xsl-fo"),
            ],
            &[("fileName", "plain.txt"), ("attachmentLocation", &fixture("plain.txt"))],
        ];
        for fields in attachments {
            let mut row = vec![("emailTemplateId", "GATE_BODY")];
            row.extend_from_slice(fields);
            seed(&facade, &context, "moqui.basic.email.EmailTemplateAttachment", &row);
        }
        let sink = SinkGuard::install();
        let rows = Value::List(vec![
            map(vec![("rowName", Value::string("one"))]),
            map(vec![("rowName", Value::string("two"))]),
        ]);
        let mut params = BTreeMap::new();
        params.insert("emailTemplateId".into(), Value::string("GATE_BODY"));
        params.insert("toAddresses".into(), Value::string("to@example.com"));
        params.insert(
            "bodyParameters".into(),
            map(vec![
                ("name", Value::string("Ada")),
                ("includeText", Value::Bool(true)),
                ("reportRows", rows),
            ]),
        );
        params.insert(
            "attachments".into(),
            Value::List(vec![map(vec![
                ("fileName", Value::string("inline.html")),
                ("attachmentLocation", Value::string(&report)),
                ("screenRenderMode", Value::string("html")),
            ])]),
        );
        let out = crate::service::sync("org.moqui.impl.EmailServices.send#EmailTemplate", &params).expect("send");
        let errors = context.message().errors().join("\n");
        let messages = context.message().messages().join("\n");
        assert!(errors.is_empty(), "{errors}");
        assert!(!messages.contains("Error sending email"), "{messages}");
        let sent = sink.sent();
        assert_eq!(sent.len(), 1, "{messages}");
        let mail = &sent[0];
        assert_eq!(mail.subject, "Report for Ada");
        let email_message_id = out
            .get("emailMessageId")
            .and_then(Value::as_str)
            .expect("emailMessageId");
        assert!(
            mail.body.contains(r#"<p class="greeting">Hello Ada</p>"#),
            "{}",
            mail.body
        );
        assert!(mail.body.contains("Report for Ada via webroot"), "{}", mail.body);
        assert!(
            mail.body.contains(&format!("message {email_message_id}")),
            "{}",
            mail.body
        );
        assert!(
            mail.body_text.contains("Hello Ada\nSubject: Report for Ada"),
            "{}",
            mail.body_text
        );
        assert!(!mail.body_text.contains('<'), "{}", mail.body_text);

        let names: Vec<&str> = mail.attachments.iter().map(|(name, _, _)| name.as_str()).collect();
        assert_eq!(
            names,
            [
                "inline.html",
                "report-one.csv",
                "report-two.csv",
                "report.txt",
                "plain.txt"
            ]
        );
        let part = |name: &str| mail.attachments.iter().find(|(file, _, _)| file == name).unwrap();
        assert_eq!(part("inline.html").1, "text/html");
        assert!(String::from_utf8_lossy(&part("inline.html").2).contains("<p>Report all</p>"));
        assert_eq!(part("report-one.csv").1, "text/csv");
        assert_eq!(
            String::from_utf8_lossy(&part("report-one.csv").2).trim(),
            "name,greeting\none,Ada"
        );
        assert!(String::from_utf8_lossy(&part("report-two.csv").2).contains("two,Ada"));
        assert_eq!(part("report.txt").1, "text/plain");
        assert_eq!(String::from_utf8_lossy(&part("report.txt").2).trim(), "Report all");
        assert_eq!(String::from_utf8_lossy(&part("plain.txt").2), "plain attachment text\n");

        assert!(mail.mime.contains("multipart/mixed"), "{}", mail.mime);
        assert!(mail.mime.contains("multipart/alternative"), "{}", mail.mime);
        assert!(mail.mime.contains("Content-Type: text/plain"), "{}", mail.mime);
        assert!(mail.mime.contains("Content-Type: text/html"), "{}", mail.mime);
        assert!(mail.mime.contains("report-one.csv"), "{}", mail.mime);
        assert!(!mail.mime.contains("report.pdf"), "{}", mail.mime);

        let _authz = context.disable_authz();
        let row = facade
            .find("moqui.basic.email.EmailMessage")
            .unwrap()
            .eq("emailMessageId", Value::string(email_message_id))
            .one()
            .unwrap()
            .expect("EmailMessage row");
        assert_eq!(row.get("statusId").and_then(Value::as_str), Some("ES_SENT"));
        assert!(row
            .get("bodyText")
            .and_then(Value::as_str)
            .unwrap_or("")
            .contains("Hello Ada"));
        assert!(row
            .get("body")
            .and_then(Value::as_str)
            .unwrap_or("")
            .contains("Hello Ada"));
    }

    #[test]
    fn screen_render_email_sends_the_rendered_screen_as_an_attachment() {
        let _lock = lock_tests();
        let (facade, context) = entered();
        let _entered = context.enter();
        crate::service::load_definitions().unwrap();
        smtp_server(&facade, &context, "SYSTEM");
        seed(
            &facade,
            &context,
            "moqui.basic.email.EmailTemplate",
            &[
                ("emailTemplateId", "SCREEN_RENDER"),
                ("emailServerId", "SYSTEM"),
                ("emailTypeEnumId", "EMT_SCREEN_RENDER"),
                (
                    "bodyScreenLocation",
                    "component://framework/screen/ScreenRenderEmail.xml",
                ),
                ("webappName", "webroot"),
                ("fromAddress", "reports@example.com"),
                ("subject", "${title}"),
            ],
        );
        log_in(&facade, &context);
        let sink = SinkGuard::install();
        let mut params = BTreeMap::new();
        params.insert("screenLocation".into(), Value::string(fixture("MailReport.xml")));
        params.insert("screenRenderMode".into(), Value::string("csv"));
        params.insert("toAddresses".into(), Value::string("boss@example.com"));
        params.insert("title".into(), Value::string("Monthly Report"));
        params.insert("bodyParameters".into(), map(vec![("rowName", Value::string("March"))]));
        crate::service::sync("org.moqui.impl.EmailServices.send#ScreenRenderEmail", &params).expect("send");
        let errors = context.message().errors().join("\n");
        let messages = context.message().messages().join("\n");
        assert!(errors.is_empty(), "{errors}");
        let sent = sink.sent();
        assert_eq!(sent.len(), 1, "{messages}");
        let mail = &sent[0];
        assert_eq!(mail.subject, "Monthly Report");
        assert!(mail.body.contains("<h2>Monthly Report</h2>"), "{}", mail.body);
        assert!(mail.body.contains("<strong>rowName</strong>: March"), "{}", mail.body);
        assert!(!mail.body.contains("lastStandalone"), "{}", mail.body);
        assert!(mail.body_text.contains("Monthly Report"), "{}", mail.body_text);
        assert!(mail.body_text.contains("rowName: March"), "{}", mail.body_text);
        assert_eq!(mail.attachments.len(), 1);
        let (name, content_type, bytes) = &mail.attachments[0];
        assert_eq!(name, "MailReport.csv");
        assert_eq!(content_type, "text/csv");
        assert!(String::from_utf8_lossy(bytes).contains("March"));
        assert!(mail.mime.contains("multipart/alternative"), "{}", mail.mime);
    }

    /// The Email Export dialog's default: the screen as xlsx, attached as the workbook it renders.
    #[test]
    fn screen_render_email_as_xlsx_attaches_the_workbook() {
        use calamine::{Data, Reader};
        let _lock = lock_tests();
        let (facade, context) = entered();
        let _entered = context.enter();
        crate::service::load_definitions().unwrap();
        smtp_server(&facade, &context, "SYSTEM");
        seed(
            &facade,
            &context,
            "moqui.basic.email.EmailTemplate",
            &[
                ("emailTemplateId", "SCREEN_RENDER"),
                ("emailServerId", "SYSTEM"),
                (
                    "bodyScreenLocation",
                    "component://framework/screen/ScreenRenderEmail.xml",
                ),
                ("fromAddress", "reports@example.com"),
                ("subject", "${title}"),
            ],
        );
        log_in(&facade, &context);
        let sink = SinkGuard::install();
        let mut params = BTreeMap::new();
        params.insert("screenLocation".into(), Value::string(fixture("MailList.xml")));
        params.insert("screenRenderMode".into(), Value::string("xlsx"));
        params.insert("toAddresses".into(), Value::string("boss@example.com"));
        params.insert("title".into(), Value::string("Monthly Report"));
        params.insert("bodyParameters".into(), map(vec![("rowName", Value::string("March"))]));
        crate::service::sync("org.moqui.impl.EmailServices.send#ScreenRenderEmail", &params).expect("send");
        let errors = context.message().errors().join("\n");
        assert!(errors.is_empty(), "{errors}");
        let sent = sink.sent();
        assert_eq!(sent.len(), 1);
        let (name, content_type, bytes) = &sent[0].attachments[0];
        assert_eq!(name, "MailList.xlsx");
        assert_eq!(
            content_type,
            "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet"
        );
        let mut book = calamine::open_workbook_auto_from_rs(std::io::Cursor::new(bytes.clone())).unwrap();
        let range = book.worksheet_range("ReportList").unwrap();
        assert_eq!(range.get_value((1, 0)), Some(&Data::String("March".to_string())));
        assert_eq!(range.get_value((1, 1)), Some(&Data::Float(12.5)));
    }

    #[test]
    fn screen_render_email_as_xsl_fo_attaches_the_pdf_or_sends_without_it() {
        let _lock = lock_tests();
        let _pdf = crate::screen::pdf_test_lock();
        crate::screen::clear_pdf_engine();
        let (facade, context) = entered();
        let _entered = context.enter();
        crate::service::load_definitions().unwrap();
        smtp_server(&facade, &context, "SYSTEM");
        seed(
            &facade,
            &context,
            "moqui.basic.email.EmailTemplate",
            &[
                ("emailTemplateId", "SCREEN_RENDER"),
                ("emailServerId", "SYSTEM"),
                (
                    "bodyScreenLocation",
                    "component://framework/screen/ScreenRenderEmail.xml",
                ),
                ("fromAddress", "reports@example.com"),
                ("subject", "${title}"),
            ],
        );
        log_in(&facade, &context);
        let sink = SinkGuard::install();
        let mut params = BTreeMap::new();
        params.insert("screenLocation".into(), Value::string(fixture("MailReport.xml")));
        params.insert("screenRenderMode".into(), Value::string("xsl-fo"));
        params.insert("toAddresses".into(), Value::string("boss@example.com"));
        params.insert("title".into(), Value::string("Printed"));
        crate::service::sync("org.moqui.impl.EmailServices.send#ScreenRenderEmail", &params).expect("send");
        let errors = context.message().errors().join("\n");
        assert!(errors.is_empty(), "{errors}");
        assert!(!errors.contains(PDF_UNAVAILABLE), "{errors}");
        let sent = sink.sent();
        assert_eq!(sent.len(), 1);
        assert!(sent[0].attachments.is_empty());
        assert!(sent[0].body.contains("Printed"), "{}", sent[0].body);

        crate::screen::install_pdf_engine(std::sync::Arc::new(FakePdf));
        crate::service::sync("org.moqui.impl.EmailServices.send#ScreenRenderEmail", &params).expect("send");
        crate::screen::clear_pdf_engine();
        let sent = sink.sent();
        assert_eq!(sent.len(), 2);
        let (filename, content_type, bytes) = &sent[1].attachments[0];
        assert_eq!(content_type, "application/pdf");
        assert!(filename.ends_with(".pdf"), "{filename}");
        assert!(bytes.starts_with(b"%PDF-fake"));
    }

    struct FakePdf;

    impl crate::screen::PdfEngine for FakePdf {
        fn render(&self, html: &str) -> Result<Vec<u8>, crate::context::Error> {
            assert!(html.contains("@page"), "the attachment is the html-print document");
            Ok(b"%PDF-fake".to_vec())
        }
    }

    #[test]
    fn a_body_screen_that_fails_to_render_does_not_send() {
        let _lock = lock_tests();
        let (facade, context) = entered();
        let _entered = context.enter();
        crate::service::load_definitions().unwrap();
        smtp_server(&facade, &context, "BROKEN_SMTP");
        seed(
            &facade,
            &context,
            "moqui.basic.email.EmailTemplate",
            &[
                ("emailTemplateId", "BROKEN"),
                ("emailServerId", "BROKEN_SMTP"),
                ("fromAddress", "from@example.com"),
                ("subject", "Broken"),
                ("bodyScreenLocation", &fixture("NoSuchScreen.xml")),
            ],
        );
        let sink = SinkGuard::install();
        let mut params = BTreeMap::new();
        params.insert("emailTemplateId".into(), Value::string("BROKEN"));
        params.insert("toAddresses".into(), Value::string("to@example.com"));
        let _ = crate::service::sync("org.moqui.impl.EmailServices.send#EmailTemplate", &params);
        let messages = context.message().messages().join("\n");
        assert!(messages.contains("Error sending email"), "{messages}");
        assert!(messages.contains("NoSuchScreen.xml"), "{messages}");
        assert!(sink.sent().is_empty());
    }

    #[test]
    fn sim_skip_returns_email_message_id_sim() {
        let _lock = lock_tests();
        let (facade, context) = entered();
        let _entered = context.enter();
        crate::service::load_definitions().unwrap();
        facade.set_sim_active(true);
        let sink = SinkGuard::install();
        let mut params = std::collections::BTreeMap::new();
        params.insert("emailTemplateId".into(), Value::string("MISSING"));
        params.insert("toAddresses".into(), Value::string("to@example.com"));
        let out = crate::service::sync("org.moqui.impl.EmailServices.send#EmailTemplate", &params).expect("sim");
        assert_eq!(out.get("emailMessageId").and_then(Value::as_str), Some("SIM"));
        assert!(sink.messages().is_empty());
        assert!(context.message().errors().is_empty());
    }

    #[test]
    fn fake_store_poll_runs_one_eca_and_skips_seen() {
        let _lock = lock_tests();
        let (facade, context) = entered();
        let _entered = context.enter();
        crate::service::load_definitions().unwrap();
        eca_rules::install(
            context.factory(),
            eca_rules::from_xml(
                "",
                r#"<emecas>
                <emeca rule-name="Notice">
                    <condition><expression>true</expression></condition>
                    <actions><script>ec.message.addMessage(fields.subject);</script></actions>
                </emeca>
            </emecas>"#,
            ),
        );
        seed(
            &facade,
            &context,
            "moqui.basic.email.EmailServer",
            &[
                ("emailServerId", "TEST_POLL"),
                ("storeHost", "imap.example.com"),
                ("mailUsername", "inbox"),
                ("mailPassword", "secret"),
                ("storeProtocol", "imap"),
                ("storeSkipSeen", "Y"),
            ],
        );
        let mut fresh = Inbound::blank();
        fresh.id = "1".to_string();
        fresh.subject = "new mail".to_string();
        fresh.seen = false;
        let mut seen = Inbound::blank();
        seen.id = "2".to_string();
        seen.subject = "old mail".to_string();
        seen.seen = true;
        let _store = FakeGuard::install(vec![fresh, seen]);
        let mut params = std::collections::BTreeMap::new();
        params.insert("emailServerId".into(), Value::string("TEST_POLL"));
        crate::service::sync("org.moqui.impl.EmailServices.poll#EmailServer", &params).expect("poll");
        let errors = context.message().errors().join("\n");
        assert!(errors.is_empty(), "{errors}");
        let messages = context.message().messages().to_vec();
        assert!(messages.iter().any(|text| text == "new mail"), "{messages:?}");
        assert!(!messages.iter().any(|text| text == "old mail"), "{messages:?}");
    }

    #[test]
    fn an_inbound_message_reads_like_java_mail() {
        let raw = b"From: =?UTF-8?B?SsO8cmdlbg==?= <j@example.com>\r\n\
To: a@example.com, \"Two, B\" <b@example.com>\r\n\
Subject: =?UTF-8?Q?Gr=C3=BC=C3=9Fe?= from\r\n the team\r\n\
Message-ID: <m1@example.com>\r\n\
Date: Tue, 5 Mar 2024 10:20:30 -0800\r\n\
MIME-Version: 1.0\r\n\
Content-Type: multipart/mixed; boundary=\"outer\"\r\n\
\r\n\
--outer\r\n\
Content-Type: multipart/alternative; boundary=\"inner\"\r\n\
\r\n\
--inner\r\n\
Content-Type: text/plain; charset=ISO-8859-1\r\n\
Content-Transfer-Encoding: quoted-printable\r\n\
\r\n\
Caf=E9 au lait\r\n\
--inner\r\n\
Content-Type: text/html; charset=UTF-8\r\n\
\r\n\
<p>Caf\xc3\xa9</p>\r\n\
--inner--\r\n\
--outer\r\n\
Content-Type: application/octet-stream; name=\"data.bin\"\r\n\
Content-Disposition: attachment; filename=\"data.bin\"\r\n\
Content-Transfer-Encoding: base64\r\n\
\r\n\
AAEC/w==\r\n\
--outer--\r\n";
        let message = parse_rfc822(raw);
        assert_eq!(message.subject, "Gr\u{fc}\u{df}e from the team");
        assert_eq!(message.from, "J\u{fc}rgen <j@example.com>");
        assert_eq!(
            message.to,
            vec!["a@example.com".to_string(), "\"Two, B\" <b@example.com>".to_string()]
        );
        assert_eq!(header_one(&message.headers, "message-id"), "<m1@example.com>");
        assert!(!message.sent.is_null());
        // The leaf parts of both multiparts, in order: text in its charset, the attachment as bytes.
        assert_eq!(message.parts.len(), 3, "{:?}", message.parts);
        let part = |index: usize, key: &str| match &message.parts[index] {
            Value::Map(map) => map.get(key).cloned().unwrap_or(Value::Null),
            other => panic!("{other:?}"),
        };
        assert_eq!(part(0, "contentText").as_str(), Some("Caf\u{e9} au lait"));
        assert!(part(1, "contentType").as_str().unwrap().starts_with("text/html"));
        assert_eq!(part(2, "filename").as_str(), Some("data.bin"));
        assert_eq!(part(2, "disposition").as_str(), Some("attachment"));
        assert!(matches!(part(2, "contentBytes"), Value::Bytes(bytes) if bytes == vec![0, 1, 2, 255]));
    }

    #[test]
    fn a_pop3_line_with_8_bit_bytes_is_read() {
        let mut reader = std::io::Cursor::new(b"Caf\xe9\r\n.\r\n".to_vec());
        assert_eq!(read_line_bytes(&mut reader).unwrap(), b"Caf\xe9".to_vec());
        assert_eq!(read_line_bytes(&mut reader).unwrap(), b".".to_vec());
    }

    #[test]
    fn a_message_whose_rule_fails_is_not_marked_or_deleted() {
        let _lock = lock_tests();
        let (facade, context) = entered();
        let _entered = context.enter();
        crate::service::load_definitions().unwrap();
        eca_rules::install(
            context.factory(),
            eca_rules::from_xml(
                "",
                r#"<emecas>
                <emeca rule-name="Breaks"><condition><expression>fields.subject == 'bad'</expression></condition>
                    <actions><script>throw new Error('rule broke')</script></actions></emeca>
                <emeca rule-name="Notes"><actions><script>ec.message.addMessage('saw ' + fields.subject);</script></actions></emeca>
            </emecas>"#,
            ),
        );
        seed(
            &facade,
            &context,
            "moqui.basic.email.EmailServer",
            &[
                ("emailServerId", "TEST_KEEP"),
                ("storeHost", "imap.example.com"),
                ("mailUsername", "inbox"),
                ("mailPassword", "secret"),
                ("storeProtocol", "imap"),
                ("storeDelete", "Y"),
            ],
        );
        let mut bad = Inbound::blank();
        bad.id = "1".to_string();
        bad.subject = "bad".to_string();
        let mut good = Inbound::blank();
        good.id = "2".to_string();
        good.subject = "good".to_string();
        let _store = FakeGuard::install(vec![bad, good]);
        FAKE_DELETED.with(|deleted| deleted.borrow_mut().clear());
        let mut params = std::collections::BTreeMap::new();
        params.insert("emailServerId".into(), Value::string("TEST_KEEP"));
        let _ = crate::service::sync("org.moqui.impl.EmailServices.poll#EmailServer", &params);
        let messages = context.message().messages().to_vec();
        // The second rule still ran for the failing message, and the next message was processed.
        assert!(messages.iter().any(|text| text == "saw bad"), "{messages:?}");
        assert!(messages.iter().any(|text| text == "saw good"), "{messages:?}");
        let left = FAKE_DELETED.with(|deleted| deleted.borrow().clone());
        assert_eq!(left, vec!["2".to_string()]);
    }

    #[test]
    fn a_failed_message_keeps_the_ones_before_it_committed_and_deleted() {
        let _lock = lock_tests();
        let (facade, context) = entered();
        let _entered = context.enter();
        crate::service::load_definitions().unwrap();
        eca_rules::install(
            context.factory(),
            eca_rules::from_xml(
                "",
                r#"<emecas>
                <emeca rule-name="Save"><actions>
                    <service-call name="org.moqui.impl.EmailServices.save#EcaEmailMessage" in-map="context"/></actions></emeca>
                <emeca rule-name="Breaks"><condition><expression>fields.subject == 'bad'</expression></condition>
                    <actions><script>throw new Error('rule broke')</script></actions></emeca>
            </emecas>"#,
            ),
        );
        seed(
            &facade,
            &context,
            "moqui.basic.email.EmailServer",
            &[
                ("emailServerId", "TEST_EACH"),
                ("storeHost", "imap.example.com"),
                ("mailUsername", "inbox"),
                ("mailPassword", "secret"),
                ("storeProtocol", "imap"),
                ("storeDelete", "Y"),
            ],
        );
        let inbound = |id: &str, subject: &str| {
            let mut message = Inbound::blank();
            message.id = id.to_string();
            message.subject = subject.to_string();
            message
                .headers
                .insert("message-id".into(), Value::string(format!("<each-{id}@example.com>")));
            message
        };
        let _store = FakeGuard::install(vec![inbound("1", "first"), inbound("2", "bad"), inbound("3", "third")]);
        FAKE_DELETED.with(|deleted| deleted.borrow_mut().clear());
        let mut params = std::collections::BTreeMap::new();
        params.insert("emailServerId".into(), Value::string("TEST_EACH"));
        let _ = crate::service::sync("org.moqui.impl.EmailServices.poll#EmailServer", &params);
        let errors = context.message().errors().join("\n");
        assert!(errors.contains("rule broke"), "{errors}");
        // The poll's own transaction rolled back on that error; the saved messages were committed before it.
        let saved = |id: &str| {
            let _authz = context.disable_authz();
            facade
                .find("moqui.basic.email.EmailMessage")
                .unwrap()
                .eq("messageId", Value::string(format!("<each-{id}@example.com>")))
                .one()
                .unwrap()
                .is_some()
        };
        assert!(saved("1"));
        assert!(!saved("2"), "the failed message's row rolled back");
        assert!(saved("3"));
        let deleted = FAKE_DELETED.with(|deleted| deleted.borrow().clone());
        assert_eq!(deleted, vec!["1".to_string(), "3".to_string()]);
    }

    #[test]
    fn the_message_id_host_is_the_domain_of_the_from_address() {
        let id = super::new_message_id("Ann Smith <ann@example.com>").unwrap();
        assert!(id.starts_with('<') && id.ends_with("@example.com>"), "{id}");
        assert_eq!(id.matches('>').count(), 1, "{id}");
        assert!(super::new_message_id("not an address")
            .unwrap()
            .ends_with("@localhost>"));
    }

    #[test]
    fn allowed_domains_match_the_domain_or_below_and_a_display_name_is_an_address() {
        let domains = vec!["example.com".to_string()];
        assert!(super::domain_allowed("a@example.com", &domains));
        assert!(super::domain_allowed("Ann <a@Mail.Example.com>", &domains));
        assert!(!super::domain_allowed("a@evilexample.com", &domains));
        assert!(!super::domain_allowed("a@example.com.evil.org", &domains));
        let mailbox = super::mailbox("Ann Smith <ann@example.com>", "").unwrap();
        assert_eq!(mailbox.email.to_string(), "ann@example.com");
        assert_eq!(mailbox.name.as_deref(), Some("Ann Smith"));
        assert_eq!(
            super::mailbox("ann@example.com", "Named").unwrap().name.as_deref(),
            Some("Named")
        );
    }
}
