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

//! A `.qvue` compiled for a page on the runtime build of Vue, which has no template compiler and needs no
//! `unsafe-eval` (SEC-PSF-04; docs/ui/Qvt-Client.md, "Bundled qvue"). A request `<path>.qvue?_qvt=bundle` (a script
//! element sends no header of its choosing) for a screen whose qvue is a `template="false"` text, or for a `.qvue`
//! file under a screen, is answered with the script of [`vue_check::qvue_make`] ([`vue_check::served_qvue`]),
//! `text/javascript` with an `ETag` and `X-Moqui-Qvt: bundled-qvue`. The client loads it with a script element, and it
//! registers in `moqui.qvtBundled` under the name its loader gave, else the public path it was loaded from: not the
//! screen path the server resolved, which a site route or a remote frontend maps from another. The client makes the
//! component with `moqui.bundledComponent(path)`.
//!
//! The screen's render runs as for the plain `.qvue` request (its authorization, actions, status and caching), and its
//! text is compiled once for each text the definition has, kept with the definition as a server-static template is
//! (`statics.rs` `template_for`); a file's once for each text, by location. A text that cannot be compiled is kept as
//! a failure until the text changes.

use std::sync::{Arc, Mutex};

use moqui_lib::codec::fnv1a;

use super::render::RenderState;
use super::vue_check;
use crate::context::{Error, ExecutionContextFactory};
use crate::web::{self, WebRequest};

/// The `X-Moqui-Qvt` of a bundle answer, and the name a client lists in `qvt.handles` when it loads them.
pub(crate) const HANDLER: &str = "bundled-qvue";

/// How many `.qvue` files under screens keep their bundles.
const MAX_FILE_BUNDLES: usize = 256;

/// Whether the request asks for the compiled form: `_qvt=bundle` in its query.
pub(crate) fn asked(request: &WebRequest) -> bool {
    request
        .query
        .iter()
        .any(|(name, value)| name == "_qvt" && value == "bundle")
}

/// What one text compiled to: its `make` object, or why it could not be compiled.
type Made = Result<Arc<str>, String>;

/// The compiled form of one `.qvue` text, with the hash of the text it was made from: a text that changes (an edited
/// file, a screen builder's overlay, a stamped render) is compiled again in its place.
#[derive(Default)]
pub(crate) struct Slot(Mutex<Option<(u64, Made)>>);

impl Slot {
    /// The kept answer for `text`, or the one compiled now. Concurrent first requests wait for one compile.
    fn made(&self, factory: &ExecutionContextFactory, text: &str) -> Made {
        let hash = fnv1a(text.as_bytes());
        let mut kept = moqui_lib::sync::lock(&self.0);
        if let Some((held, made)) = kept.as_ref() {
            if *held == hash {
                return made.clone();
            }
        }
        let made: Made = super::statics::compiler_source(factory)
            .and_then(|compiler| vue_check::qvue_make(&compiler, text))
            .map(Arc::from)
            .map_err(|err| err.message().to_string());
        *kept = Some((hash, made.clone()));
        made
    }
}

/// The bundles of `.qvue` files under screens, by location.
struct FileBundles(Mutex<crate::script::BoundedMap<String, Arc<Slot>>>);

/// After a qvue render that asked for its bundle: the text the render wrote, compiled, in its place. A render that did not
/// answer 200 (a refusal, a redirect, an error page) stays as it is; one that rendered a template has no bundle (404).
pub(crate) fn answer_render(state: &RenderState, request: &WebRequest) {
    if !web::output_sent() || web::stream::committed() || web::output_status() != 200 {
        return;
    }
    let def = &state.url_info.target_screen;
    if state.templated_text.get() {
        web::clear_output();
        web::send_json_error(
            404,
            "This screen's qvue is a template, not a file's text, and has no bundle",
        );
        return;
    }
    let (_, headers, body, _) = web::take_output();
    web::clear_output();
    let text = String::from_utf8_lossy(&body);
    let made = def.generated.qvue_bundle().made(&state.factory, &text);
    respond(request, headers, made, &def.location);
}

/// A `.qvue` file under a screen, asked for as its bundle: its text compiled, with the conf's headers of a text file
/// under a screen.
///
/// # Errors
///
/// Returns an error when the file cannot be read.
pub(crate) fn answer_file(
    factory: &ExecutionContextFactory,
    request: &WebRequest,
    location: &str,
    versioned: bool,
) -> Result<(), Error> {
    let text = factory.resource().get(location)?.read_text()?;
    let bundles = factory.extension(|_| FileBundles(Mutex::new(crate::script::BoundedMap::new(MAX_FILE_BUNDLES))));
    let slot = {
        let mut map = moqui_lib::sync::lock(&bundles.0);
        if let Some(slot) = map.get(location) {
            slot
        } else {
            let slot = Arc::new(Slot::default());
            map.insert(location.to_string(), Arc::clone(&slot));
            slot
        }
    };
    let made = slot.made(factory, &text);
    let mut headers = Vec::new();
    web::apply_conf_headers(Some(factory), "screen-resource-text", &mut headers, false);
    if versioned {
        headers.retain(|(name, _)| !name.eq_ignore_ascii_case("Cache-Control"));
        headers.push((
            "Cache-Control".to_string(),
            "public, max-age=31536000, immutable".to_string(),
        ));
    }
    respond(request, headers, made, location);
    Ok(())
}

/// Send the bundle of `made`, whose script and `ETag` depend on the text alone, with `headers` (what the render or the
/// conf set: `Cache-Control` and the render's own) less those of the text it replaces. A text that did not compile is a
/// 500, logged with `what`.
fn respond(request: &WebRequest, headers: Vec<(String, String)>, made: Made, what: &str) {
    let make = match made {
        Ok(make) => make,
        Err(reason) => {
            tracing::warn!("The .qvue of {what} has no bundle: {reason}");
            web::send_json_error(500, "This .qvue could not be compiled for the runtime build of Vue");
            return;
        }
    };
    let mut cache_control = String::from("private, no-store");
    let mut extra = Vec::new();
    for (header, value) in headers {
        if header.eq_ignore_ascii_case("Cache-Control") {
            cache_control = value;
        } else if !["Content-Type", "Content-Length", "ETag", "Vary", "X-Moqui-Qvt"]
            .iter()
            .any(|replaced| header.eq_ignore_ascii_case(replaced))
        {
            extra.push((header, value));
        }
    }
    extra.push(("X-Moqui-Qvt".to_string(), HANDLER.to_string()));
    let script = vue_check::served_qvue(&make);
    let etag = format!("W/\"{:x}\"", fnv1a(script.as_bytes()));
    super::static_files::respond_bytes(
        request,
        "text/javascript;charset=UTF-8",
        &cache_control,
        &extra,
        &etag,
        script.into_bytes(),
    );
}
