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

//! Boot and facade errors. Display is the message only, never a secret value.

use std::fmt;

/// What a caller can act on. The web layer maps these to HTTP statuses; see `web::status_for`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum ErrorKind {
    #[default]
    General,
    /// The call needs a logged-in user.
    AuthenticationRequired,
    /// The user is logged in and not allowed.
    Unauthorized,
    /// Too many hits on an artifact.
    Tarpit,
    /// There is no such resource.
    NotFound,
    /// The resource exists but not for this method.
    MethodNotAllowed,
    /// The service cannot take the work now, such as a full request queue.
    Unavailable,
    /// Input that does not pass a service's parameter checks. The checks that failed are validation errors in the
    /// message facade.
    Validation,
    /// A request the client got wrong, such as a missing or ambiguous name in a REST path.
    BadRequest,
    /// The transaction was marked rollback-only, so it rolled back or takes no more statements.
    RollbackOnly,
}

impl ErrorKind {
    /// The kind of the first context [`Error`] in an error's source chain, or `General`.
    pub fn of(err: &(dyn std::error::Error + 'static)) -> Self {
        let mut current: Option<&(dyn std::error::Error + 'static)> = Some(err);
        while let Some(found) = current {
            if let Some(context) = found.downcast_ref::<Error>() {
                return context.kind;
            }
            current = found.source();
        }
        Self::General
    }
}

/// The artifact an `Unauthorized` error refused: what the `error/Forbidden` page names.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DeniedArtifact {
    pub artifact_type: String,
    pub name: String,
    pub action: String,
}

pub struct Error {
    message: String,
    origin: Option<&'static std::panic::Location<'static>>,
    kind: ErrorKind,
    denied: Option<Box<DeniedArtifact>>,
    missing_path: Option<Box<str>>,
    source: Option<Box<dyn std::error::Error + Send + Sync>>,
}

impl Error {
    #[track_caller]
    pub fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
            origin: Some(std::panic::Location::caller()),
            kind: ErrorKind::General,
            denied: None,
            missing_path: None,
            source: None,
        }
    }

    #[track_caller]
    pub fn of_kind(kind: ErrorKind, message: impl Into<String>) -> Self {
        Self {
            kind,
            ..Self::new(message)
        }
    }

    pub fn kind(&self) -> ErrorKind {
        self.kind
    }

    #[must_use]
    pub fn with_denied(mut self, denied: DeniedArtifact) -> Self {
        self.denied = Some(Box::new(denied));
        self
    }

    /// The refused artifact of the first context error in the source chain that names one.
    pub fn denied(&self) -> Option<&DeniedArtifact> {
        self.first_in_chain(|err| err.denied.as_deref())
    }

    /// A `NotFound` error for a screen path: `name` is the path segment no screen or transition matched.
    #[must_use]
    pub fn with_missing_path(mut self, name: impl Into<String>) -> Self {
        self.missing_path = Some(name.into().into_boxed_str());
        self
    }

    /// The screen path segment that was not found, of the first context error in the source chain that names one.
    pub fn missing_path(&self) -> Option<&str> {
        self.first_in_chain(|err| err.missing_path.as_deref())
    }

    fn first_in_chain<'a, T: ?Sized>(&'a self, pick: impl Fn(&'a Error) -> Option<&'a T>) -> Option<&'a T> {
        let mut current: Option<&(dyn std::error::Error + 'static)> = Some(self);
        while let Some(found) = current {
            if let Some(picked) = found.downcast_ref::<Error>().and_then(&pick) {
                return Some(picked);
            }
            current = found.source();
        }
        None
    }

    pub fn with_source(mut self, source: impl std::error::Error + Send + Sync + 'static) -> Self {
        self.source = Some(Box::new(source));
        self
    }

    pub fn message(&self) -> &str {
        &self.message
    }

    /// Attach execution context without losing the original kind, source, or source location.
    pub(crate) fn with_message(mut self, message: impl Into<String>) -> Self {
        self.message = message.into();
        self
    }

    /// The same error as another kind, such as the security kind a script error carried.
    #[must_use]
    pub(crate) fn with_kind(mut self, kind: ErrorKind) -> Self {
        self.kind = kind;
        self
    }

    /// A JavaScript exception already has its own source location in its stack.
    pub(crate) fn script(message: impl Into<String>) -> Self {
        Self {
            origin: None,
            ..Self::new(message)
        }
    }

    /// The complete diagnostic, including the deepest known Rust origin, not wrapper call sites.
    pub fn diagnostic(&self) -> String {
        let mut text = self.to_string();
        let mut origin = self.origin;
        let mut source = std::error::Error::source(self);
        while let Some(error) = source {
            if let Some(error) = error.downcast_ref::<Self>() {
                origin = error.origin.or(origin);
            } else if let Some(error) = error.downcast_ref::<moqui_lib::error::Error>() {
                origin = error.origin().or(origin);
            }
            source = error.source();
        }
        if !text.contains("\n    Rust: ") {
            if let Some(origin) = origin {
                text.push_str(&format!("\n    Rust: {origin}"));
            }
        }
        text
    }
}

/// A context error that crossed a lib-typed closure comes back as it went in. Any other lib error keeps the kind of a
/// context error in its source chain.
impl From<moqui_lib::error::Error> for Error {
    fn from(err: moqui_lib::error::Error) -> Self {
        let carried = match err.into_carried() {
            Ok(carried) => carried,
            Err(err) => {
                return Self {
                    message: err.message().to_string(),
                    origin: None,
                    kind: ErrorKind::of(&err),
                    denied: None,
                    missing_path: None,
                    source: Some(Box::new(err)),
                }
            }
        };
        match carried.downcast::<Error>() {
            Ok(err) => *err,
            Err(other) => Self {
                message: other.to_string(),
                origin: None,
                kind: ErrorKind::of(other.as_ref()),
                denied: None,
                missing_path: None,
                source: Some(other),
            },
        }
    }
}

/// Carried whole, so its kind, source, and refused artifact come back on the way out of lib-typed code.
impl From<Error> for moqui_lib::error::Error {
    fn from(err: Error) -> Self {
        let origin = err.origin;
        let carried = moqui_lib::error::Error::carry(err.message.clone(), err);
        match origin {
            Some(origin) => carried.with_origin(origin),
            None => carried,
        }
    }
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.message)?;
        if let Some(source) = &self.source {
            write!(f, ": {source}")?;
        }
        Ok(())
    }
}

impl fmt::Debug for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Display::fmt(self, f)
    }
}

impl std::error::Error for Error {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        self.source
            .as_ref()
            .map(|err| err.as_ref() as &(dyn std::error::Error + 'static))
    }
}

#[cfg(test)]
mod tests {
    use super::{DeniedArtifact, Error, ErrorKind};

    #[test]
    fn rust_origin_and_artifact_survive_context_and_library_wrappers() {
        let line = line!() + 1;
        let error = Error::of_kind(ErrorKind::Unauthorized, "denied").with_denied(DeniedArtifact {
            name: "Screen.xml".into(),
            action: "view".into(),
            artifact_type: "AT_XML_SCREEN".into(),
        });
        let error: moqui_lib::error::Error = error.with_message("denied in service A").into();
        let diagnostic = error.diagnostic();
        assert!(
            diagnostic.contains(&format!("Rust: {}:{line}:", file!())),
            "{diagnostic}"
        );
        let error = Error::from(error);
        assert_eq!(error.kind(), ErrorKind::Unauthorized);
        assert_eq!(error.denied().unwrap().name, "Screen.xml");
        assert_eq!(error.message(), "denied in service A");
        assert_eq!(error.diagnostic(), diagnostic);
    }

    #[test]
    fn a_script_error_does_not_claim_the_rust_exception_handler_as_its_origin() {
        let error: moqui_lib::error::Error = Error::script("Error: failed\n    at A.js:4:2").into();
        assert!(!error.diagnostic().contains("Rust:"));
        assert!(!Error::from(error).diagnostic().contains("Rust:"));
    }

    #[test]
    fn deepest_library_origin_is_used() {
        let line = line!() + 1;
        let error = moqui_lib::error::Error::new("disk failed");
        let error = Error::new("reading screen").with_source(error);
        let diagnostic = error.diagnostic();
        assert!(
            diagnostic.contains(&format!("Rust: {}:{line}:", file!())),
            "{diagnostic}"
        );
    }
}
