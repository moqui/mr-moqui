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

//! Plain error for `moqui-lib`. Artifact exceptions stay in `moqui-core`.

use std::fmt;

/// Where a parsed node or a parse failure sits in a file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SourceLocation {
    pub file: Option<String>,
    pub line: u32,
    pub column: u32,
}

/// Failure from a lib utility. Library paths return this instead of panicking.
#[derive(Debug)]
pub struct Error {
    message: String,
    origin: Option<&'static std::panic::Location<'static>>,
    location: Option<Box<SourceLocation>>,
    source: Option<Box<dyn std::error::Error + Send + Sync>>,
    /// The source is an error of a higher layer carried as it is: Display shows it in place of the message.
    carried: bool,
}

impl Error {
    #[track_caller]
    pub fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
            origin: Some(std::panic::Location::caller()),
            location: None,
            source: None,
            carried: false,
        }
    }

    #[track_caller]
    pub fn at(message: impl Into<String>, file: Option<String>, line: u32, column: u32) -> Self {
        Self {
            message: message.into(),
            origin: Some(std::panic::Location::caller()),
            location: Some(Box::new(SourceLocation { file, line, column })),
            source: None,
            carried: false,
        }
    }

    /// An error of a higher layer carried through lib-typed code, such as a closure run by a transaction. Display is
    /// the carried error's, `source()` is the carried error, and [`Self::into_carried`] gives it back whole.
    pub fn carry(message: impl Into<String>, error: impl std::error::Error + Send + Sync + 'static) -> Self {
        Self {
            message: message.into(),
            origin: None,
            location: None,
            source: Some(Box::new(error)),
            carried: true,
        }
    }

    /// The error made by [`Self::carry`], or this error when it carries none.
    ///
    /// # Errors
    ///
    /// Returns this error unchanged when it was not made by [`Self::carry`].
    pub fn into_carried(self) -> Result<Box<dyn std::error::Error + Send + Sync>, Self> {
        match self.source {
            Some(source) if self.carried => Ok(source),
            source => Err(Self { source, ..self }),
        }
    }

    pub fn with_source(mut self, source: impl std::error::Error + Send + Sync + 'static) -> Self {
        self.source = Some(Box::new(source));
        self.carried = false;
        self
    }

    pub fn message(&self) -> &str {
        &self.message
    }

    /// The Rust call site that created this error, separate from a parsed document's location.
    pub fn origin(&self) -> Option<&'static std::panic::Location<'static>> {
        self.origin
    }

    /// Preserve an origin when a higher-layer error is carried through this library.
    pub fn with_origin(mut self, origin: &'static std::panic::Location<'static>) -> Self {
        self.origin = Some(origin);
        self
    }

    /// Detailed diagnostics without changing the message used for validation and error classification.
    pub fn diagnostic(&self) -> String {
        let mut text = self.to_string();
        let mut origin = self.origin;
        let mut source = std::error::Error::source(self);
        while let Some(error) = source {
            if let Some(error) = error.downcast_ref::<Self>() {
                origin = error.origin.or(origin);
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

    pub fn location(&self) -> Option<&SourceLocation> {
        self.location.as_deref()
    }
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if let (true, Some(source)) = (self.carried, &self.source) {
            return fmt::Display::fmt(source, f);
        }
        f.write_str(&self.message)?;
        if let Some(loc) = &self.location {
            write!(f, " (")?;
            if let Some(file) = &loc.file {
                write!(f, "{file}:")?;
            }
            write!(f, "{}:{})", loc.line, loc.column)?;
        }
        if let Some(source) = &self.source {
            write!(f, ": {source}")?;
        }
        Ok(())
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
    use super::Error;

    #[test]
    fn diagnostics_locate_the_constructor_not_the_wrapper() {
        let line = line!() + 1;
        let original = Error::new("original");
        assert_eq!(original.origin().unwrap().line(), line);
        assert_eq!(original.message(), "original");
        let wrapped = Error::new("wrapper").with_source(original);
        let diagnostic = wrapped.diagnostic();
        assert!(
            diagnostic.contains(&format!("Rust: {}:{line}:", file!())),
            "{diagnostic}"
        );
        assert_eq!(wrapped.message(), "wrapper");
    }

    #[test]
    fn parsed_document_locations_are_distinct_from_rust_origins() {
        let error = Error::at("bad XML", Some("Screen.xml".into()), 12, 3);
        assert_eq!(error.location().unwrap().line, 12);
        let diagnostic = error.diagnostic();
        assert!(diagnostic.contains("Screen.xml:12:3"), "{diagnostic}");
        assert!(diagnostic.contains(&format!("Rust: {}:", file!())), "{diagnostic}");
    }
}
