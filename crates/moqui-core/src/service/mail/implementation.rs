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
#[cfg(test)]
use std::rc::Rc;
use std::sync::Arc;
use std::time::Duration;

use crate::service::util::{field_text, map_text, now_stamp, param_text};

use indexmap::IndexMap;
use moqui_lib::codec::hex_encode;
use moqui_lib::error::Error as LibError;
use moqui_lib::object::{is_empty, to_plain_string, Value};
#[cfg(test)]
use moqui_lib::sync::lock;

use crate::context::{Error, ExecutionContext, ExecutionContextFactory};
use crate::entity::{EntityFacade, EntityValue};
use crate::screen::{make_render, PdfRenderer, ScreenFacade};
use crate::transaction::TransactionAttribute;

const TEMPLATE: &str = "moqui.basic.email.EmailTemplate";
pub(super) const SERVER: &str = "moqui.basic.email.EmailServer";
const MESSAGE: &str = "moqui.basic.email.EmailMessage";
const ATTACHMENT: &str = "moqui.basic.email.EmailTemplateAttachment";
const ACCOUNT: &str = "moqui.basic.email.EmailAccount";
const IDENTITY: &str = "moqui.basic.email.EmailIdentity";
const MESSAGE_ATTACHMENT: &str = "moqui.basic.email.EmailMessageAttachment";

pub(super) fn send_template(
    parameters: &BTreeMap<String, Value>,
    facade: &EntityFacade,
) -> Result<BTreeMap<String, Value>, LibError> {
    if facade.sim_active() {
        return Ok(ids("SIM", ""));
    }
    crate::entity::fence::check_effect(crate::entity::fence::Effect::Mail, "send#EmailTemplate")?;
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
    let server_web_base_url = find_one(facade, SERVER, "emailServerId", &field_text(&template, "emailServerId"))?
        .map(|server| field_text(&server, "webBaseUrl"))
        .unwrap_or_default();
    let env = RenderEnv::new(&template, &server_web_base_url, parameters);
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
        inline: Vec::new(),
        message_id,
        in_reply_to: String::new(),
        references: String::new(),
        date: None,
        extra_headers: template_extra_headers(parameters),
    };
    if !filter_recipients(&mut letter, &field_text(&server, "allowedToDomains"), &email_message_id) {
        return Ok(ids(&email_message_id, ""));
    }
    letter.attachments = match collect_attachments(facade, parameters, &template_id, &env) {
        Ok(attachments) => attachments,
        Err(err) => return Ok(send_error(&err, &email_message_id, &template_id)),
    };
    if let Err(err) = deliver(facade, &letter, &server) {
        note_message(format!("Error sending email: {err}"));
        return Ok(ids(&email_message_id, ""));
    }
    if !email_message_id.is_empty() {
        update_sent(facade, &email_message_id, &letter.message_id)?;
    }
    Ok(ids(&email_message_id, &letter.message_id))
}

/// `send#EmailMessage`: one stored message, through its `emailServerId`, else the server of its mail client account
/// (`emailAccountId`), so a template's message and a client's draft each go out the way they came. A client message
/// also brings its identity's reply-to, its `EmailMessageAttachment` files (inline ones by Content-ID) and its
/// In-Reply-To and References headers. A delivery that fails is a message, not an error, as Java's, and its text is
/// the `sendError` result, so the mail client can keep it for a retry.
///
/// A client message is the mail client's to keep in its own transaction: whatever stops it after the status checks
/// (no address, no server, a host not allowed, an attachment that cannot be read, the delivery) is its `sendError`,
/// not an error that would roll back the caller, and its Sent status is written in the caller's transaction, which
/// holds its row, where a template's message is written in a new one as Java does.
pub(super) fn send_message(
    parameters: &BTreeMap<String, Value>,
    facade: &EntityFacade,
) -> Result<BTreeMap<String, Value>, LibError> {
    let email_message_id = param_text(parameters, "emailMessageId");
    if facade.sim_active() {
        return Ok(ids("SIM", ""));
    }
    crate::entity::fence::check_effect(crate::entity::fence::Effect::Mail, "send#EmailMessage")?;
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
    let account_id = field_text(&message, "emailAccountId");
    let client = !account_id.is_empty();
    let refuse = |text: String| -> Result<BTreeMap<String, Value>, LibError> {
        if client {
            Ok(send_failed(&email_message_id, &Error::new(text)))
        } else {
            fail(text)
        }
    };
    let body = field_text(&message, "body");
    let body_text = field_text(&message, "bodyText");
    let from_address = field_text(&message, "fromAddress");
    let to_addresses = field_text(&message, "toAddresses");
    if body.is_empty() && body_text.is_empty() {
        return refuse(format!("Email Message {email_message_id} has no body"));
    }
    if from_address.is_empty() {
        return refuse(format!("Email Message {email_message_id} has no from address"));
    }
    if to_addresses.is_empty() {
        return refuse(format!("Email Message {email_message_id} has no to address"));
    }
    let mut server_id = field_text(&message, "emailServerId");
    if server_id.is_empty() {
        if let Some(account) = find_one(facade, ACCOUNT, "emailAccountId", &account_id)? {
            server_id = field_text(&account, "emailServerId");
        }
    }
    let Some(server) = find_one(facade, SERVER, "emailServerId", &server_id)? else {
        return refuse(format!(
            "No Email Server record found for Email Message {email_message_id}"
        ));
    };
    let host = field_text(&server, "smtpHost");
    if host.is_empty() {
        tracing::warn!(server = %server_id, message = %email_message_id, "SMTP Host is empty, not sending email");
        if client {
            return refuse(format!("Email server {server_id} has no SMTP host"));
        }
        return Ok(ids(&email_message_id, ""));
    }
    if let Err(text) = require_host(&host) {
        return refuse(text.to_string());
    }
    let template_id = field_text(&message, "emailTemplateId");
    let template = find_one(facade, TEMPLATE, "emailTemplateId", &template_id)?;
    let (mut reply_to, bounce) = match &template {
        Some(row) => (field_text(row, "replyToAddresses"), field_text(row, "bounceAddress")),
        None => (String::new(), String::new()),
    };
    if reply_to.is_empty() {
        if let Some(identity) = find_one(
            facade,
            IDENTITY,
            "emailIdentityId",
            &field_text(&message, "emailIdentityId"),
        )? {
            reply_to = field_text(&identity, "replyToAddress");
        }
    }
    // a client message keeps the Message-ID it has (a draft appended to the server's Drafts, or one synced from there),
    // so its copy in the server's Sent is found by it and the draft's copy is the same message
    let kept_id = if client {
        field_text(&message, "messageId")
    } else {
        String::new()
    };
    let message_id = if kept_id.trim().is_empty() {
        match new_message_id(&from_address) {
            Ok(message_id) => message_id,
            Err(text) => return refuse(text.to_string()),
        }
    } else {
        kept_id.trim().to_string()
    };
    let extra_headers = if client {
        client_headers(&field_text(&message, "headersString"))
    } else {
        // a template message (a list email) sent again keeps the list headers it was made with
        stored_headers(&field_text(&message, "headersString"), &LIST_HEADERS)
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
        inline: Vec::new(),
        message_id,
        in_reply_to: field_text(&message, "inReplyTo"),
        references: field_text(&message, "referencesHeader"),
        date: None,
        extra_headers,
    };
    if !filter_recipients(&mut letter, &field_text(&server, "allowedToDomains"), &email_message_id) {
        if client {
            return refuse(format!(
                "No To address of email message {email_message_id} is in a domain email server {server_id} allows"
            ));
        }
        return Ok(ids(&email_message_id, ""));
    }
    if let Err(err) = stored_attachments(facade, &email_message_id, &mut letter) {
        return Ok(send_failed(&email_message_id, &err));
    }
    if let Err(err) = deliver(facade, &letter, &server) {
        return Ok(send_failed(&email_message_id, &err));
    }
    if status == "ES_READY" || status == "ES_BOUNCED" {
        if client {
            mark_sent(facade, &email_message_id, &letter.message_id)?;
        } else {
            update_sent(facade, &email_message_id, &letter.message_id)?;
        }
    }
    Ok(ids(&email_message_id, &letter.message_id))
}

/// A send that did not go out: the message Java adds, and the error as the `sendError` result.
fn send_failed(email_message_id: &str, err: &Error) -> BTreeMap<String, Value> {
    tracing::warn!(email_message = %email_message_id, error = %err, "Error sending email");
    note_message(format!("Error sending email: {err}"));
    let mut out = ids(email_message_id, "");
    out.insert("sendError".into(), Value::string(err.message()));
    out
}

/// The `EmailMessageAttachment` files of a message: an inline one with a Content-ID goes in the related part beside
/// the HTML, the others are attachments. A file that cannot be read stops the send, so nothing goes out without it.
pub(super) fn stored_attachments(
    facade: &EntityFacade,
    email_message_id: &str,
    letter: &mut Letter,
) -> Result<(), Error> {
    let rows = facade
        .find(MESSAGE_ATTACHMENT)
        .and_then(|find| {
            find.eq("emailMessageId", Value::string(email_message_id))
                .order_by("emailMessageAttachmentId", false)
                .list()
        })
        .map_err(|err| Error::new(err.to_string()))?;
    for row in rows.values() {
        let location = field_text(row, "attachmentLocation");
        let mut filename = field_text(row, "fileName");
        if filename.is_empty() {
            filename = location.rsplit('/').next().unwrap_or_default().to_string();
        }
        if location.is_empty() {
            return Err(Error::new(format!("attachment {filename} has no location")));
        }
        let bytes = read_resource(&location)?;
        let part = AttachmentBytes {
            content_type: fallback_type(&field_text(row, "contentType"), &filename, "application/octet-stream"),
            filename,
            bytes,
        };
        let content_id = field_text(row, "contentId");
        if field_text(row, "isInline") == "Y" && !content_id.is_empty() {
            letter
                .inline
                .push((content_id.trim_matches(['<', '>']).to_string(), part));
        } else {
            letter.attachments.push(part);
        }
    }
    Ok(())
}

/// The bytes at a resource location of any kind (a `dbresource://` file among them), through the resource facade.
fn read_resource(location: &str) -> Result<Vec<u8>, Error> {
    let found = ExecutionContext::with_current(|ctx| ctx.factory().resource().get(location))
        .ok_or_else(|| Error::new("no execution context is entered"))?;
    let reference =
        found.map_err(|err| Error::new(format!("could not read attachment {location}: {}", err.message())))?;
    if !reference.exists() {
        return Err(Error::new(format!("attachment {location} is not there")));
    }
    reference
        .read_bytes()
        .map_err(|err| Error::new(format!("could not read attachment {location}: {}", err.message())))
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
    /// The base URL links in the email start with is the first of: `https://` and the template's `webHostName`, the
    /// server's `webBaseUrl`, the conf webapp's root URL. Never a request's host: an email is often sent from a job.
    fn new(template: &EntityValue, server_web_base_url: &str, parameters: &BTreeMap<String, Value>) -> Self {
        let factory = ExecutionContext::with_current(|ctx| Arc::clone(ctx.factory()));
        let webapp_name = {
            let named = field_text(template, "webappName");
            if named.is_empty() {
                "webroot".to_string()
            } else {
                named
            }
        };
        let web_host_name = field_text(template, "webHostName");
        let base_url = if !web_host_name.is_empty() {
            format!("https://{web_host_name}")
        } else if !server_web_base_url.trim().is_empty() {
            server_web_base_url.trim().trim_end_matches('/').to_string()
        } else {
            factory
                .as_ref()
                .map(|factory| factory.webapp_root_url(&webapp_name))
                .unwrap_or_default()
        };
        let parameters = body_parameters(parameters)
            .map(|map| map.iter().map(|(key, value)| (key.clone(), value.clone())).collect())
            .unwrap_or_default();
        Self {
            factory,
            webapp_name,
            web_host_name,
            base_url,
            parameters,
        }
    }

    /// Render `location` as the root screen, or `screen_path` under the default root screen (Java's
    /// `rootScreenFromHost`, which this port has one of) as the last standalone screen, in `mode`.
    fn render(&self, location: &str, screen_path: &str, mode: &str) -> Result<String, Error> {
        self.render_bytes(location, screen_path, mode)
            .map(|bytes| String::from_utf8_lossy(&bytes).into_owned())
    }

    /// The body of the render, which for a binary mode such as `xlsx` is the document.
    fn render_bytes(&self, location: &str, screen_path: &str, mode: &str) -> Result<Vec<u8>, Error> {
        let Some(factory) = &self.factory else {
            return Err(Error::new("no execution context to render a screen in"));
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
            .map_err(|err| Error::new(format!("could not render {shown} as {mode}: {}", err.message())))?;
        if output.status != 200 {
            return Err(Error::new(format!(
                "could not render {shown} as {mode}: status {}",
                output.status
            )));
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
fn render_bodies(env: &RenderEnv, screen: &str, body: String, body_text: String) -> Result<(String, String), Error> {
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
fn send_error(err: &Error, email_message_id: &str, template_id: &str) -> BTreeMap<String, Value> {
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
        let extra = template_extra_headers(parameters);
        if !extra.is_empty() {
            set_text(&mut row, "headersString", &header_block(&extra));
        }
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
    force_new(facade, || mark_sent(facade, id, message_id))
}

/// The message Sent with its Message-ID, in the transaction in place.
fn mark_sent(facade: &EntityFacade, id: &str, message_id: &str) -> Result<(), LibError> {
    let _authz = ExecutionContext::with_current(|ctx| ctx.disable_authz());
    let mut row = facade.make_value(MESSAGE)?;
    set_text(&mut row, "emailMessageId", id);
    set_text(&mut row, "statusId", "ES_SENT");
    set_text(&mut row, "messageId", message_id);
    row.set("sentDate", now_stamp());
    row.update(facade)?;
    Ok(())
}

pub(super) fn force_new<T>(facade: &EntityFacade, body: impl FnOnce() -> Result<T, LibError>) -> Result<T, LibError> {
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

pub(super) fn require_host(host: &str) -> Result<(), Error> {
    if host_allowed(host) {
        Ok(())
    } else {
        Err(Error::new(format!(
            "Email host {host} is not in email-client allowed-hosts"
        )))
    }
}

fn host_allowed(host: &str) -> bool {
    host_in(&super::client_setting("allowed-hosts"), host)
}

/// True when `allowed` (comma-separated hosts and domains) is empty or names `host` or a domain above it.
fn host_in(allowed: &str, host: &str) -> bool {
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
) -> Result<Vec<AttachmentBytes>, Error> {
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
        .map_err(|err| Error::new(err.to_string()))?;
    for row in rows.iter() {
        push_stored_attachment(&mut out, row, env)?;
    }
    Ok(out)
}

fn push_parameter_attachment(
    out: &mut Vec<AttachmentBytes>,
    map: &IndexMap<String, Value>,
    env: &RenderEnv,
) -> Result<(), Error> {
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
fn push_stored_attachment(out: &mut Vec<AttachmentBytes>, row: &EntityValue, env: &RenderEnv) -> Result<(), Error> {
    let mode = field_text(row, "screenRenderMode");
    let location = field_text(row, "attachmentLocation");
    let filename = field_text(row, "fileName");
    let condition = field_text(row, "attachmentCondition");
    if !condition.is_empty() {
        let holds = crate::script::eval_condition(&condition)
            .map_err(|err| Error::new(format!("attachmentCondition of {filename}: {}", err.message())))?;
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
            .map_err(|err| Error::new(format!("forEachIn of {filename}: {}", err.message())))?
        {
            Value::List(items) => items,
            Value::Null => Vec::new(),
            other => return Err(Error::new(format!("forEachIn of {filename} is not a list: {other:?}"))),
        };
        for entry in entries {
            let _frame = crate::script::ContextFrame::push();
            match entry {
                Value::Map(map) => {
                    for (name, value) in map {
                        crate::script::set_context_name(&name, value).map_err(|err| Error::new(err.message()))?;
                    }
                }
                other => {
                    crate::script::set_context_name("forEachEntry", other).map_err(|err| Error::new(err.message()))?;
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
) -> Result<(), Error> {
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

fn expand_text(text: &str) -> Result<String, Error> {
    crate::script::expand(text).map_err(|err| Error::new(err.message()))
}

fn read_location(location: &str) -> Result<Vec<u8>, Error> {
    let path = crate::context::locate(location).map_err(|err| Error::new(err.message()))?;
    std::fs::read(&path).map_err(|err| Error::new(format!("could not read attachment {location}: {err}")))
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
pub(super) struct AttachmentBytes {
    pub(super) filename: String,
    pub(super) content_type: String,
    pub(super) bytes: Vec<u8>,
}

#[derive(Clone, Debug)]
pub(super) struct Letter {
    pub(super) subject: String,
    pub(super) from_address: String,
    pub(super) from_name: String,
    pub(super) to: Vec<String>,
    pub(super) cc: Vec<String>,
    pub(super) bcc: Vec<String>,
    pub(super) reply_to: Vec<String>,
    pub(super) bounce: String,
    pub(super) body: String,
    pub(super) body_text: String,
    pub(super) attachments: Vec<AttachmentBytes>,
    /// Parts the HTML refers to by `cid:`, each with its Content-ID.
    pub(super) inline: Vec<(String, AttachmentBytes)>,
    pub(super) message_id: String,
    /// The In-Reply-To and References headers of a reply, empty for none.
    pub(super) in_reply_to: String,
    pub(super) references: String,
    /// The Date header in epoch milliseconds; now when None. The copy of sent mail the sync appends to Sent has the
    /// time it was sent.
    pub(super) date: Option<i64>,
    /// Headers a client message carries in its `headersString` that go out with it ([`CLIENT_HEADERS`]): the
    /// automatic reply's `Auto-Submitted: auto-replied` (RFC 3834) and its loop guards.
    pub(super) extra_headers: Vec<(String, String)>,
}

/// The headers of a client message's `headersString` that are sent: an automatic reply's (RFC 3834 and the Microsoft
/// and list conventions), so the mail it answers is not answered back. Any other header there is the received mail's.
pub(super) const CLIENT_HEADERS: [&str; 3] = ["Auto-Submitted", "X-Auto-Response-Suppress", "Precedence"];

/// The headers of a template message's `headersString` that go out again when it is sent again: a list email's
/// (RFC 2369, RFC 8058).
pub(super) const LIST_HEADERS: [&str; 3] = ["List-Unsubscribe", "List-Unsubscribe-Post", "List-Id"];

/// Headers the letter's own fields write, which `extraHeaders` cannot set (lower case).
const LETTER_HEADERS: [&str; 16] = [
    "from",
    "sender",
    "to",
    "cc",
    "bcc",
    "reply-to",
    "subject",
    "date",
    "message-id",
    "in-reply-to",
    "references",
    "return-path",
    "mime-version",
    "content-type",
    "content-transfer-encoding",
    "content-disposition",
];

/// The `extraHeaders` map of `send#EmailTemplate` (a list email's List-Unsubscribe and List-Unsubscribe-Post): each name a
/// header field name (letters, digits and `-`) that is not one the letter writes itself, each value one line of under
/// 998 characters with no line breaks (so a value cannot add a header). Anything else is logged and left out.
pub(super) fn template_extra_headers(parameters: &BTreeMap<String, Value>) -> Vec<(String, String)> {
    let Some(Value::Map(map)) = parameters.get("extraHeaders") else {
        return Vec::new();
    };
    let mut out: Vec<(String, String)> = Vec::new();
    for (name, value) in map {
        let name = name.trim();
        let value = to_plain_string(value).trim().to_string();
        let name_ok = !name.is_empty()
            && name.len() <= 76
            && name.bytes().all(|byte| byte.is_ascii_alphanumeric() || byte == b'-')
            && !LETTER_HEADERS.contains(&name.to_ascii_lowercase().as_str());
        let value_ok = !value.is_empty() && value.len() < 998 && !value.contains(['\r', '\n']);
        if !name_ok || !value_ok {
            tracing::warn!(header = %name, "extraHeaders: {name} is not a header this send may add, left out");
            continue;
        }
        if out.iter().any(|(kept, _)| kept.eq_ignore_ascii_case(name)) {
            continue;
        }
        out.push((name.to_string(), value));
    }
    out
}

/// A header block of `headers` for `EmailMessage.headersString`, a line each.
fn header_block(headers: &[(String, String)]) -> String {
    headers
        .iter()
        .map(|(name, value)| format!("{name}: {value}\r\n"))
        .collect()
}

/// The [`CLIENT_HEADERS`] of a stored header block, in order, each once.
pub(super) fn client_headers(headers: &str) -> Vec<(String, String)> {
    stored_headers(headers, &CLIENT_HEADERS)
}

/// The `names` of a stored header block that are sent, in order, each once.
pub(super) fn stored_headers(headers: &str, names: &[&str]) -> Vec<(String, String)> {
    let mut out: Vec<(String, String)> = Vec::new();
    for name in names {
        let value = header_value(headers, name);
        let value = value.trim();
        // a header value is one line of printable text
        if !value.is_empty() && !value.contains(['\r', '\n']) && value.len() < 998 {
            out.push((name.to_string(), value.to_string()));
        }
    }
    out
}

/// The unfolded value of the first header `name` in a raw header block.
pub(super) fn header_value(headers: &str, name: &str) -> String {
    let mut found: Option<String> = None;
    for line in headers.split('\n') {
        let line = line.trim_end_matches('\r');
        if let Some(value) = found.as_mut() {
            if line.starts_with([' ', '\t']) {
                value.push(' ');
                value.push_str(line.trim());
                continue;
            }
            break;
        }
        if let Some((field, value)) = line.split_once(':') {
            if field.trim().eq_ignore_ascii_case(name) {
                found = Some(value.trim().to_string());
            }
        }
    }
    found.unwrap_or_default()
}

fn deliver(facade: &EntityFacade, letter: &Letter, server: &EntityValue) -> Result<(), Error> {
    if captured(letter) {
        return Ok(());
    }
    lettre_send(facade, letter, server)
}

/// A message built whole (the SMTP listener's delivered notice, `delivery.rs`), sent from the null reverse path to
/// `to` through the SMTP of server `server_id`, within half a minute. A test sink or `mail_capture_sends` keeps it as
/// a letter with the message as its text.
#[cfg(feature = "imap")]
pub(super) fn send_built(
    facade: &EntityFacade,
    server_id: &str,
    from: &str,
    to: &str,
    subject: &str,
    raw: &[u8],
) -> Result<(), Error> {
    let letter = Letter {
        subject: subject.to_string(),
        from_address: from.to_string(),
        from_name: String::new(),
        to: vec![to.to_string()],
        cc: Vec::new(),
        bcc: Vec::new(),
        reply_to: Vec::new(),
        bounce: String::new(),
        body: String::new(),
        body_text: String::from_utf8_lossy(raw).into_owned(),
        attachments: Vec::new(),
        inline: Vec::new(),
        message_id: String::new(),
        in_reply_to: String::new(),
        references: String::new(),
        date: None,
        extra_headers: Vec::new(),
    };
    if captured(&letter) {
        return Ok(());
    }
    let server = find_one(facade, SERVER, "emailServerId", server_id)
        .map_err(|err| Error::new(err.to_string()))?
        .ok_or_else(|| Error::new(format!("no email server {server_id}")))?;
    let host = field_text(&server, "smtpHost");
    if host.is_empty() {
        return Err(Error::new(format!("email server {server_id} has no SMTP host")));
    }
    require_host(&host)?;
    let recipient = to
        .parse::<lettre::Address>()
        .map_err(|err| Error::new(format!("{to} is not an address: {err}")))?;
    let envelope = lettre::address::Envelope::new(None, vec![recipient]).map_err(|err| Error::new(err.to_string()))?;
    let auth = smtp_auth(facade, &server, None)?;
    let transport = smtp_transport(&server, &auth, Some(Duration::from_secs(30)))?;
    lettre::Transport::send_raw(&transport, &envelope, raw)
        .map(|_| ())
        .map_err(|err| Error::new(err.to_string()))
}

/// Whether a test sink or `mail_capture_sends` kept `letter` instead of sending it.
fn captured(letter: &Letter) -> bool {
    #[cfg(test)]
    if push_sink(letter) {
        return true;
    }
    // mail_capture_sends (moqui test): kept for the spec instead of sent (service/mail_capture.rs)
    if crate::service::mail_capture::enabled() {
        crate::service::mail_capture::record(crate::service::mail_capture::CapturedMail {
            subject: letter.subject.clone(),
            from_address: letter.from_address.clone(),
            from_name: letter.from_name.clone(),
            to: letter.to.clone(),
            cc: letter.cc.clone(),
            bcc: letter.bcc.clone(),
            reply_to: letter.reply_to.clone(),
            body: letter.body.clone(),
            body_text: letter.body_text.clone(),
            message_id: letter.message_id.clone(),
            in_reply_to: letter.in_reply_to.clone(),
            references: letter.references.clone(),
            attachment_names: letter.attachments.iter().map(|part| part.filename.clone()).collect(),
            headers: letter.extra_headers.clone(),
        });
        return true;
    }
    false
}

fn lettre_send(facade: &EntityFacade, letter: &Letter, server: &EntityValue) -> Result<(), Error> {
    let message = build_lettre(letter)?;
    let auth = smtp_auth(facade, server, None)?;
    let transport = smtp_transport(server, &auth, None)?;
    match lettre::Transport::send(&transport, &message) {
        Ok(_) => Ok(()),
        // an access token the server refused (revoked, or ended early): once more with a new one, which is safe
        // before the server took the message
        Err(err) if matches!(auth, SmtpAuth::OAuth(..)) && smtp_auth_refused(&err) => {
            let SmtpAuth::OAuth(_, rejected) = &auth else {
                return Err(Error::new(err.to_string()));
            };
            let fresh = smtp_auth(facade, server, Some(rejected))?;
            if matches!(&fresh, SmtpAuth::OAuth(_, token) if token == rejected) {
                return Err(Error::new(err.to_string()));
            }
            let transport = smtp_transport(server, &fresh, None)?;
            lettre::Transport::send(&transport, &message)
                .map(|_| ())
                .map_err(|err| Error::new(err.to_string()))
        }
        Err(err) => Err(Error::new(err.to_string())),
    }
}

/// How SMTP signs in: not at all (no `mailUsername`), with the password, or for an EatOAuth2 server with XOAUTH2 and
/// an access token (user and token).
enum SmtpAuth {
    None,
    Password(String, String),
    #[cfg_attr(not(feature = "imap"), allow(dead_code))]
    OAuth(String, String),
}

/// The user SMTP signs in as: `smtpUsername` when the server has its own SMTP sign-in, else `mailUsername`.
pub(super) fn smtp_user(server: &EntityValue) -> String {
    let own = field_text(server, "smtpUsername");
    if own.trim().is_empty() {
        field_text(server, "mailUsername")
    } else {
        own
    }
}

/// The SMTP password: with an SMTP user of its own `smtpPassword` (else `mailPassword`), else `mailPassword`.
fn smtp_password(server: &EntityValue) -> String {
    let own = field_text(server, "smtpPassword");
    if field_text(server, "smtpUsername").trim().is_empty() || own.is_empty() {
        field_text(server, "mailPassword")
    } else {
        own
    }
}

/// The SMTP sign-in of a server row ([`smtp_user`]); for EatOAuth2 a current access token (`rejected` is one the
/// server just refused). The provider sign-in is part of `imap`; a build without it refuses an EatOAuth2 server.
#[cfg_attr(not(feature = "imap"), allow(unused_variables))]
fn smtp_auth(facade: &EntityFacade, server: &EntityValue, rejected: Option<&str>) -> Result<SmtpAuth, Error> {
    let user = smtp_user(server);
    if field_text(server, "authTypeEnumId") == "EatOAuth2" {
        #[cfg(feature = "imap")]
        {
            let token = super::oauth::access_token(facade, &field_text(server, "emailServerId"), rejected)
                .map_err(|err| Error::new(err.to_string()))?;
            return Ok(SmtpAuth::OAuth(user, token));
        }
        #[cfg(not(feature = "imap"))]
        return Err(Error::new(
            "the OAuth sign-in of mail accounts (imap) is not built into this host",
        ));
    }
    if user.is_empty() {
        return Ok(SmtpAuth::None);
    }
    Ok(SmtpAuth::Password(user, smtp_password(server)))
}

/// A refusal of the sign-in: 530, 534, 535 or 454, or Gmail's XOAUTH2 error challenge, which lettre cannot answer.
fn smtp_auth_refused(err: &lettre::transport::smtp::Error) -> bool {
    let code = err.status().map(u16::from);
    matches!(code, Some(454 | 530 | 534 | 535)) || (code.is_none() && err.to_string().contains("challenge"))
}

/// The SMTP transport of a server row, signing in as `auth` says. `timeout` replaces lettre's default of a minute.
fn smtp_transport(
    server: &EntityValue,
    auth: &SmtpAuth,
    timeout: Option<Duration>,
) -> Result<lettre::SmtpTransport, Error> {
    let host = field_text(server, "smtpHost");
    // Every SMTP connection starts here, so nothing reaches a mail server under the read fence.
    crate::entity::fence::check_effect(crate::entity::fence::Effect::Mail, &host)?;
    let port = parse_port(&field_text(server, "smtpPort"), 25)?;
    // smtpSsl is TLS from the start. smtpStartTls upgrades when the server offers it, as Java's setStartTLSEnabled
    // (not required), so a server without STARTTLS still takes the mail. Both trust `email-client` `tls-ca-file` too.
    use lettre::transport::smtp::authentication::{Credentials, Mechanism};
    use lettre::transport::smtp::client::Tls;
    let mut builder = lettre::SmtpTransport::builder_dangerous(&host).port(port);
    if field_text(server, "smtpSsl") == "Y" {
        builder = builder.tls(Tls::Wrapper(smtp_tls(&host)?));
    } else if field_text(server, "smtpStartTls") == "Y" {
        builder = builder.tls(Tls::Opportunistic(smtp_tls(&host)?));
    }
    if timeout.is_some() {
        builder = builder.timeout(timeout);
    }
    builder = match auth {
        SmtpAuth::None => builder,
        SmtpAuth::Password(user, password) => builder.credentials(Credentials::new(user.clone(), password.clone())),
        SmtpAuth::OAuth(user, token) => builder
            .credentials(Credentials::new(user.clone(), token.clone()))
            .authentication(vec![Mechanism::Xoauth2]),
    };
    Ok(builder.build())
}

pub(super) fn build_lettre(letter: &Letter) -> Result<lettre::Message, Error> {
    let from = mailbox(&letter.from_address, &letter.from_name)?;
    let mut builder = lettre::Message::builder()
        .from(from)
        .message_id(Some(letter.message_id.clone()));
    if let Some(millis) = letter.date.and_then(|millis| u64::try_from(millis).ok()) {
        builder = builder.date(std::time::UNIX_EPOCH + Duration::from_millis(millis));
    }
    // As commons-email, which sets the header only for a subject that is not empty.
    if !letter.subject.is_empty() {
        builder = builder.subject(letter.subject.clone());
    }
    if !letter.in_reply_to.trim().is_empty() {
        builder = builder.in_reply_to(letter.in_reply_to.trim().to_string());
    }
    if !letter.references.trim().is_empty() {
        builder = builder.references(letter.references.split_whitespace().collect::<Vec<_>>().join(" "));
    }
    for (name, value) in &letter.extra_headers {
        let header = lettre::message::header::HeaderName::new_from_ascii(name.clone())
            .map_err(|err| Error::new(format!("header {name} is not valid: {err}")))?;
        builder = builder.raw_header(lettre::message::header::HeaderValue::new(header, value.clone()));
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
            .map_err(|err| Error::new(format!("bounce address {} is not valid: {err}", letter.bounce)))?;
        let envelope =
            lettre::address::Envelope::new(Some(sender), recipients).map_err(|err| Error::new(err.to_string()))?;
        builder = builder.envelope(envelope);
    }
    let message = if letter.attachments.is_empty() && letter.inline.is_empty() {
        finish_body(builder, letter)?
    } else {
        builder
            .multipart(with_attachments(letter)?)
            .map_err(|err| Error::new(err.to_string()))?
    };
    Ok(message)
}

fn finish_body(builder: lettre::message::MessageBuilder, letter: &Letter) -> Result<lettre::Message, Error> {
    if !letter.body.is_empty() && !letter.body_text.is_empty() {
        return builder
            .multipart(lettre::message::MultiPart::alternative_plain_html(
                letter.body_text.clone(),
                letter.body.clone(),
            ))
            .map_err(|err| Error::new(err.to_string()));
    }
    if !letter.body.is_empty() {
        return builder
            .header(lettre::message::header::ContentType::TEXT_HTML)
            .body(letter.body.clone())
            .map_err(|err| Error::new(err.to_string()));
    }
    builder
        .header(lettre::message::header::ContentType::TEXT_PLAIN)
        .body(letter.body_text.clone())
        .map_err(|err| Error::new(err.to_string()))
}

fn with_attachments(letter: &Letter) -> Result<lettre::message::MultiPart, Error> {
    let mut mixed = if letter.inline.is_empty() {
        with_body(lettre::message::MultiPart::mixed(), letter)
    } else {
        // multipart/related: the body, then the parts it refers to by cid:
        let mut related = with_body(lettre::message::MultiPart::related(), letter);
        for (content_id, part) in &letter.inline {
            related = related.singlepart(
                lettre::message::Attachment::new_inline(content_id.clone())
                    .body(part.bytes.clone(), content_type(&part.content_type)),
            );
        }
        lettre::message::MultiPart::mixed().multipart(related)
    };
    for attachment in &letter.attachments {
        let part = lettre::message::Attachment::new(attachment.filename.clone())
            .body(attachment.bytes.clone(), content_type(&attachment.content_type));
        mixed = mixed.singlepart(part);
    }
    Ok(mixed)
}

/// `parts` with the body added: plain and HTML as alternatives when there are both.
fn with_body(parts: lettre::message::MultiPartBuilder, letter: &Letter) -> lettre::message::MultiPart {
    if !letter.body.is_empty() && !letter.body_text.is_empty() {
        parts.multipart(lettre::message::MultiPart::alternative_plain_html(
            letter.body_text.clone(),
            letter.body.clone(),
        ))
    } else if !letter.body.is_empty() {
        parts.singlepart(lettre::message::SinglePart::html(letter.body.clone()))
    } else {
        parts.singlepart(lettre::message::SinglePart::plain(letter.body_text.clone()))
    }
}

fn content_type(name: &str) -> lettre::message::header::ContentType {
    lettre::message::header::ContentType::parse(name).unwrap_or(lettre::message::header::ContentType::TEXT_PLAIN)
}

/// An address as Java's `addTo` takes it: `a@example.com` or `Name <a@example.com>`. A `name` given separately wins.
fn mailbox(address: &str, name: &str) -> Result<lettre::message::Mailbox, Error> {
    let address = address.trim();
    let parsed: lettre::message::Mailbox = address
        .parse()
        .map_err(|err| Error::new(format!("email address {address} is not valid: {err}")))?;
    let name = name.trim();
    if name.is_empty() {
        Ok(parsed)
    } else {
        Ok(lettre::message::Mailbox::new(Some(name.to_string()), parsed.email))
    }
}

/// A random `<hex@host>` Message-ID, the host the domain of the from address.
pub(super) fn new_message_id(from: &str) -> Result<String, Error> {
    let mut bytes = [0u8; 12];
    getrandom::fill(&mut bytes).map_err(|err| Error::new(format!("could not make a Message-ID: {err}")))?;
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

/// `test#EmailServer`: whether a server's settings work, step by step, for the mail account wizard. SMTP connects,
/// upgrades to TLS as configured and signs in (then QUIT); the store connects and signs in, and IMAP lists its folders
/// with their special-use attributes and the folder kind each maps to. Each step is `{step, ok, message}`; a step that
/// fails ends its protocol's steps, and the other protocol is still tried. Nothing is changed.
pub(super) fn test_server(
    parameters: &BTreeMap<String, Value>,
    facade: &EntityFacade,
) -> Result<BTreeMap<String, Value>, LibError> {
    if facade.sim_active() {
        return fail("Email server test is disabled in LLM sim session");
    }
    crate::entity::fence::check_effect(crate::entity::fence::Effect::Mail, "test#EmailServer")?;
    let server_id = param_text(parameters, "emailServerId");
    let Some(mut server) = find_one(facade, SERVER, "emailServerId", &server_id)? else {
        return fail(format!("No EmailServer found for ID [{server_id}]"));
    };
    // an SMTP sign-in to try before it is stored: smtpUsername as given (empty for the store's), smtpPassword when given
    if let Some(user) = parameters.get("smtpUsername").filter(|value| !value.is_null()) {
        server.set("smtpUsername", Value::string(to_plain_string(user).trim()));
    }
    let password = param_text(parameters, "smtpPassword");
    if !password.is_empty() {
        server.set("smtpPassword", Value::string(password));
    }
    let wanted = |name: &str| match parameters.get(name) {
        Some(Value::Bool(flag)) => *flag,
        Some(value) if !is_empty(value) => {
            !matches!(to_plain_string(value).to_ascii_lowercase().as_str(), "false" | "n")
        }
        _ => true,
    };
    let mut steps = Vec::new();
    #[cfg_attr(not(feature = "imap"), allow(unused_mut))]
    let mut folders = Vec::new();
    if wanted("testSmtp") {
        steps.push(smtp_step(facade, &server));
    }
    if wanted("testStore") && !field_text(&server, "storeHost").is_empty() {
        #[cfg(feature = "imap")]
        super::inbound::store_steps(facade, &server, &mut steps, &mut folders);
        #[cfg(not(feature = "imap"))]
        steps.push(("store", false, "imap is not built into this host".to_string()));
    }
    let success = !steps.is_empty() && steps.iter().all(|step| step.1);
    let mut out = BTreeMap::new();
    out.insert(
        "stepList".into(),
        Value::List(
            steps
                .into_iter()
                .map(|(step, ok, message)| {
                    let mut map = IndexMap::new();
                    map.insert("step".to_string(), Value::string(step));
                    map.insert("ok".to_string(), Value::Bool(ok));
                    map.insert("message".to_string(), Value::string(message));
                    Value::Map(map)
                })
                .collect(),
        ),
    );
    out.insert("folderList".into(), Value::List(folders));
    out.insert("success".into(), Value::Bool(success));
    Ok(out)
}

const TEST_TIMEOUT: Duration = Duration::from_secs(20);

fn smtp_step(facade: &EntityFacade, server: &EntityValue) -> (&'static str, bool, String) {
    let host = field_text(server, "smtpHost");
    if host.is_empty() {
        return ("smtp", false, "The server has no SMTP host".to_string());
    }
    let port = field_text(server, "smtpPort");
    let place = if port.is_empty() {
        host.clone()
    } else {
        format!("{host}:{port}")
    };
    let connect = |auth: &SmtpAuth| {
        smtp_transport(server, auth, Some(TEST_TIMEOUT))
            .and_then(|transport| transport.test_connection().map_err(|err| Error::new(err.to_string())))
    };
    let checked = require_host(&host)
        .and_then(|()| smtp_auth(facade, server, None))
        .and_then(|auth| match (connect(&auth), &auth) {
            // an access token refused: once more with a new one, as a send does
            (Err(_), SmtpAuth::OAuth(_, rejected)) => connect(&smtp_auth(facade, server, Some(rejected))?),
            (result, _) => result,
        });
    let user = smtp_user(server);
    match checked {
        Ok(true) if !user.is_empty() => ("smtp", true, format!("Connected to {place} and signed in as {user}")),
        Ok(true) => ("smtp", true, format!("Connected to {place}")),
        Ok(false) => ("smtp", false, format!("{place} closed the connection")),
        Err(err) => ("smtp", false, format!("SMTP {place}: {err}")),
    }
}

/// The certificates of the PEM file `email-client` `tls-ca-file` names, for a mail server with its own CA. Empty when
/// unset.
pub(super) fn extra_ca_certificates() -> Result<Vec<rustls::pki_types::CertificateDer<'static>>, Error> {
    use rustls::pki_types::pem::PemObject;
    let location = super::client_setting("tls-ca-file");
    if location.trim().is_empty() {
        return Ok(Vec::new());
    }
    let pem = std::fs::read(location.trim())
        .map_err(|err| Error::new(format!("could not read tls-ca-file {location}: {err}")))?;
    rustls::pki_types::CertificateDer::pem_slice_iter(&pem)
        .collect::<Result<Vec<_>, _>>()
        .map_err(|err| Error::new(format!("tls-ca-file {location} is not PEM certificates: {err}")))
}

fn smtp_tls(host: &str) -> Result<lettre::transport::smtp::client::TlsParameters, Error> {
    let mut builder = lettre::transport::smtp::client::TlsParameters::builder(host.to_string());
    for der in extra_ca_certificates()? {
        let certificate = lettre::transport::smtp::client::Certificate::from_der(der.to_vec())
            .map_err(|err| Error::new(err.to_string()))?;
        builder = builder.add_root_certificate(certificate);
    }
    builder.build().map_err(|err| Error::new(err.to_string()))
}

pub(super) fn find_one(
    facade: &EntityFacade,
    entity: &str,
    field: &str,
    value: &str,
) -> Result<Option<EntityValue>, LibError> {
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
pub(super) fn split_addresses(value: &str) -> Vec<String> {
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

pub(super) fn parse_port(text: &str, default: u16) -> Result<u16, Error> {
    if text.trim().is_empty() {
        return Ok(default);
    }
    text.trim()
        .parse()
        .map_err(|_| Error::new(format!("port {text} is not a number")))
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

pub(super) fn fail(text: impl std::fmt::Display) -> Result<BTreeMap<String, Value>, LibError> {
    note_error(text);
    Ok(BTreeMap::new())
}

fn fail_with(text: impl std::fmt::Display, email_message_id: &str) -> Result<BTreeMap<String, Value>, LibError> {
    note_error(text);
    Ok(ids(email_message_id, ""))
}

pub(super) fn note_error(text: impl std::fmt::Display) {
    let text = text.to_string();
    ExecutionContext::with_current(|ctx| ctx.message().add_error(text));
}

pub(super) fn note_message(text: impl Into<String>) {
    let text = text.into();
    ExecutionContext::with_current(|ctx| ctx.message().add_message(text));
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use indexmap::IndexMap;

    use super::{field_text, host_in, stored_headers, SinkGuard, LIST_HEADERS, MESSAGE};
    use crate::context::ExecutionContextFactory;
    use crate::entity::EntityFacade;
    use crate::screen::PDF_UNAVAILABLE;
    #[cfg(feature = "imap")]
    use crate::service::eca_rules;
    #[cfg(feature = "imap")]
    use crate::service::mail::inbound::{header_one, parse_rfc822, read_line_bytes, FakeGuard, Inbound, FAKE_DELETED};
    use moqui_lib::object::Value;

    /// The tests set `email-client` `allowed-hosts`, a process property, so they share the property lock.
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

    fn entered() -> (EntityFacade, crate::context::ExecutionContext) {
        entered_with("")
    }

    /// [`entered`] with the conf text `conf`.
    fn entered_with(conf: &str) -> (EntityFacade, crate::context::ExecutionContext) {
        let facade = crate::testing::open();
        let context = ExecutionContextFactory::test_context_in(
            conf,
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
        assert!(host_in("", "smtp.example.com"));
        assert!(host_in("example.com", "smtp.example.com"));
        assert!(host_in("example.com", "example.com"));
        assert!(!host_in("example.com", "notexample.com"));
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

    /// A list email's extra headers go out and stay on the message for a send again; one the letter writes itself, or a
    /// value with a line break (an injected header), is left out.
    #[test]
    fn a_template_send_adds_the_list_headers_and_keeps_them() {
        let _lock = lock_tests();
        let (facade, context) = entered();
        let _entered = context.enter();
        crate::service::load_definitions().unwrap();
        seed(
            &facade,
            &context,
            "moqui.basic.email.EmailServer",
            &[("emailServerId", "TEST_LIST"), ("smtpHost", "smtp.example.com")],
        );
        seed(
            &facade,
            &context,
            "moqui.basic.email.EmailTemplate",
            &[
                ("emailTemplateId", "LIST"),
                ("emailServerId", "TEST_LIST"),
                ("fromAddress", "news@example.com"),
                ("subject", "News"),
            ],
        );
        let sink = SinkGuard::install();
        let mut extra = indexmap::IndexMap::new();
        extra.insert(
            "List-Unsubscribe".into(),
            Value::string("<https://example.com/u/TOKEN/oneClick>, <mailto:leave@example.com?subject=unsubscribe>"),
        );
        extra.insert(
            "List-Unsubscribe-Post".into(),
            Value::string("List-Unsubscribe=One-Click"),
        );
        extra.insert("Subject".into(), Value::string("Not this one"));
        extra.insert("X-Note".into(), Value::string("a\r\nBcc: someone@example.net"));
        let mut params = std::collections::BTreeMap::new();
        params.insert("emailTemplateId".into(), Value::string("LIST"));
        params.insert("toAddresses".into(), Value::string("member@example.com"));
        params.insert("body".into(), Value::string("<p>News</p>"));
        params.insert("extraHeaders".into(), Value::Map(extra));
        let out = crate::service::sync("org.moqui.impl.EmailServices.send#EmailTemplate", &params).expect("send");
        let errors = context.message().errors().join("\n");
        assert!(errors.is_empty(), "{errors}");
        let sent = sink.messages();
        assert_eq!(sent.len(), 1);
        let names: Vec<&str> = sent[0].extra_headers.iter().map(|(name, _)| name.as_str()).collect();
        assert_eq!(names, ["List-Unsubscribe", "List-Unsubscribe-Post"]);
        assert_eq!(sent[0].subject, "News");
        assert!(sent[0].bcc.is_empty());
        // kept on the message, and what a send again of it takes
        let id = moqui_lib::object::to_plain_string(out.get("emailMessageId").expect("id"));
        let row = facade
            .find(MESSAGE)
            .unwrap()
            .eq("emailMessageId", Value::string(&id))
            .one()
            .unwrap()
            .unwrap();
        let stored = field_text(&row, "headersString");
        let again = stored_headers(&stored, &LIST_HEADERS);
        assert_eq!(again, sent[0].extra_headers);
    }

    #[test]
    fn host_rejected_by_allowed_hosts_does_not_touch_the_sink() {
        let _lock = lock_tests();
        let (facade, context) = entered_with(r#"<moqui-conf><email-client allowed-hosts="example.com"/></moqui-conf>"#);
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

    #[cfg(feature = "xlsx")]
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

    #[cfg(feature = "imap")]
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

    #[cfg(feature = "imap")]
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

    #[cfg(feature = "imap")]
    #[test]
    fn a_pop3_line_with_8_bit_bytes_is_read() {
        let mut reader = std::io::Cursor::new(b"Caf\xe9\r\n.\r\n".to_vec());
        assert_eq!(read_line_bytes(&mut reader).unwrap(), b"Caf\xe9".to_vec());
        assert_eq!(read_line_bytes(&mut reader).unwrap(), b".".to_vec());
    }

    #[cfg(feature = "imap")]
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

    #[cfg(feature = "imap")]
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
