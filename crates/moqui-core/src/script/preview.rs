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

//! The host a preview's proposed screen text runs on (plans/Security-Review-Low.md SEC-AI-102): an allow-list.
//!
//! While a proposed screen's code runs ([`crate::screen::proposed_running`]), each run of its text (an action block, a
//! widget or condition expression, a `${}` template) is a [`PreviewCode`] region. Inside it a script calls only the host
//! members [`allowed`] names: entity finds and values and service calls, with authorization applying; `ec.message`,
//! `ec.l10n` and `ec.logger`; the context; the user's reads; the pure helper objects; `sri`'s reads and URLs; and
//! `ec.resource`'s `expand`, `expression` and `condition`, which run more of the same text in the same region. Every
//! other member throws "not available in a preview": every `ec.web` member, the rest of `ec.resource`, `ec.cache`,
//! `ec.factory`, `ec.artifactExecution` but `authzLocked`, raw SQL and the other `tools`, the user's switches and the
//! effective time, `ec.screen` and `sri`'s renders and writes.
//!
//! The restriction holds while the region's code is what runs: the top artifact frame is a screen's. Work the text
//! calls into pushes a frame of its own (an entity find's, a service call's), and the trusted code that work runs (an
//! entity filter, a service's body, an ECA rule) has the whole host again; a service call also lifts the region
//! ([`lift`]). The framework's own render code (the templates), which runs around the regions, is not restricted.

use std::cell::Cell;

thread_local! {
    /// How many [`PreviewCode`] regions are open on this thread, zero inside a [`lift`].
    static DEPTH: Cell<u32> = const { Cell::new(0) };
}

thread_local! {
    /// Set while a preview holds the thread with no runtime of its own ([`PreviewHeld`]): its text runs no code.
    static SHARED: Cell<bool> = const { Cell::new(false) };
}

/// A preview's hold of the thread's script runtime ([`crate::screen::overlay::with`]): a fresh one with its built-in
/// objects frozen, or, when a script runs on the thread now, no code of the proposed text at all.
pub(crate) struct PreviewHeld {
    _runtime: Option<crate::script::oxide::PreviewRuntime>,
    shared_before: bool,
}

impl PreviewHeld {
    pub(crate) fn hold() -> Self {
        let runtime = crate::script::oxide::PreviewRuntime::enter();
        let shared = runtime.is_none();
        if shared {
            tracing::warn!("a preview of proposed screen text renders inside a script: its text runs no code");
        }
        Self {
            _runtime: runtime,
            shared_before: SHARED.with(|held| held.replace(shared)),
        }
    }
}

impl Drop for PreviewHeld {
    fn drop(&mut self) {
        SHARED.with(|held| held.set(self.shared_before));
    }
}

/// A run of a preview's proposed text, while it lives; nothing when no proposed screen's code runs.
#[must_use = "dropping the guard ends the region"]
pub(crate) struct PreviewCode(Option<u32>);

/// Open a region when a proposed screen's code runs now ([`crate::screen::proposed_running`]).
///
/// # Errors
///
/// When the preview has no runtime of its own ([`PreviewHeld`]): the text runs no code then.
pub(crate) fn enter() -> Result<PreviewCode, crate::context::Error> {
    if !crate::screen::proposed_running() {
        return Ok(PreviewCode(None));
    }
    if SHARED.with(Cell::get) {
        return Err(crate::context::Error::new(
            "a preview of proposed screen text inside a script runs none of its code",
        ));
    }
    Ok(PreviewCode(Some(DEPTH.with(|depth| depth.replace(depth.get() + 1)))))
}

/// Lift the regions open on this thread while the guard lives: trusted code the text called (a service's own code, an
/// ECA rule's actions) has the whole host.
pub(crate) fn lift() -> PreviewCode {
    let before = DEPTH.with(|depth| depth.replace(0));
    PreviewCode((before > 0).then_some(before))
}

impl Drop for PreviewCode {
    fn drop(&mut self) {
        if let Some(before) = self.0 {
            DEPTH.with(|depth| depth.set(before));
        }
    }
}

impl PreviewCode {
    /// Whether this guard opened a region.
    pub(crate) fn opened(&self) -> bool {
        self.0.is_some() && DEPTH.with(Cell::get) > 0
    }
}

/// Whether the code running now is a preview's proposed text: a region is open and the top artifact frame is a
/// screen's (not an entity find's or a service call's that the text called into).
pub(crate) fn restricted() -> bool {
    DEPTH.with(Cell::get) > 0
        && crate::context::ExecutionContext::with_current(crate::context::ExecutionContext::top_is_screen)
            .unwrap_or(false)
}

/// The error a member outside the allow-list answers in a region.
pub(crate) fn refusal(path: &str) -> String {
    format!("{path} is not available in a preview of proposed screen text")
}

/// The `ec.user` members a preview's text may call: reads of the user and the clock.
const USER_READS: &[&str] = &[
    "userId",
    "username",
    "hasPermission",
    "isInGroup",
    "getUserGroupIdSet",
    "getPreference",
    "getPreferences",
    "nowTimestamp",
    "timestampMillis",
    "getLocale",
    "getTimeZone",
    "locale",
    "timeZone",
    "userAccount",
];

/// The `sri` members a preview's text may call: reads of the render's state, the field values and the URLs.
const SRI_READS: &[&str] = &[
    "baseLinkUrl",
    "getBaseLinkUrl",
    "uniqueId",
    "encodeHtml",
    "getRenderMode",
    "isAgentRender",
    "isOverlaid",
    "getActiveScreenDef",
    "getScreenUrlInfo",
    "pushContext",
    "popContext",
    "setInContext",
    "pushSingleFormMapContext",
    "getSingleFormMap",
    "getFieldValueString",
    "getFieldValue",
    "getFieldValuePlainString",
    "getFieldValueClass",
    "getFieldValueTexts",
    "getNamedValuePlain",
    "getFieldEntityValue",
    "getFieldOptions",
    "getFormHiddenParameters",
    "getSavedFindFullLocation",
    "getUserFormListFinds",
    "getUserDefaultFormListFindId",
    "getThemeIconClass",
    "getCurrentThemeId",
    "getThemeValues",
    "getFieldValidation",
    "getFieldValidationJsRules",
    "getFieldValidationRules",
    "checkCondition",
    "doBoundaryComments",
    "formListColumnCharWidths",
    "formListSubRowCount",
    "nodeLit",
    "nodeLitParent",
    "nodeLitChild",
    // urls.rs
    "buildUrl",
    "buildUrlFromTarget",
    "getScreenUrlInstance",
    "getCurrentScreenUrl",
    "makeUrlByType",
    "isAnchorLink",
    "activeInCurrentMenu",
];

/// The `tools` members the allowed facades are built on: the context and its shared map, the lock's own question, and
/// the entity definitions a find reads.
const TOOLS: &[&str] = &[
    "authzLocked",
    "contextPush",
    "contextPop",
    "contextPushContext",
    "contextPopContext",
    "contextRootMap",
    "contextAddRootMap",
    "contextClear",
    "sharedGet",
    "sharedSet",
    "sharedDelete",
    "sharedHas",
    "sharedKeys",
    "isEntityDefined",
    "entityDefinitionInfo",
    "prettyName",
    "camelCaseToPretty",
    "prettyToCamelCase",
    "periodRange",
];

/// The `WebUtilities` members that are pure text functions.
const WEB_UTILITIES: &[&str] = &[
    "encodeHtml",
    "encodeHtmlJsSafe",
    "fieldValuesEncodeHtmlJsSafe",
    "isSafeSinglePathSegment",
    "lastPathSegment",
];

/// Whether a preview's proposed text may call the host member at census `path` (`ec.entity.find`,
/// `EntityFind.list`, `ec.service.sync().call`). `disableAuthz` of a find or a service call passes here and is refused
/// by its own check (`host::refuse_locked_authz`).
pub(crate) fn allowed(path: &str) -> bool {
    let Some((class, name)) = path.rsplit_once('.') else {
        return false;
    };
    match class {
        "EntityFind" | "EntityList" | "EntityValue" | "EntityListIterator" | "EntityCondition" => true,
        "ec.entity.conditionFactory" => true,
        "ec.entity" => matches!(name, "find" | "findOneXml" | "makeValue"),
        "ec.service" => matches!(name, "sync" | "callXml"),
        "ec.service.sync()" | "ec.service.sync().callFuture()" => true,
        "ec.message" | "ec.l10n" | "ec.logger" => true,
        "ec.user" => USER_READS.contains(&name),
        "ec.user.context" => matches!(name, "read" | "names"),
        "global" | "StringUtilities" | "CollectionUtilities" => true,
        "WebUtilities" => WEB_UTILITIES.contains(&name),
        "UrlInstance" => true,
        "sri" => SRI_READS.contains(&name),
        "tools" => TOOLS.contains(&name),
        _ => false,
    }
}
