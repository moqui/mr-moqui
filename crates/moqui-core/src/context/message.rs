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

//! Messages and errors for one execution context. Not process-global.

#[derive(Clone, Debug, Default)]
pub struct MessageFacade {
    messages: Vec<String>,
    /// The type of each message: `info`, `success`, `warning` or `danger`, as Java's `MessageInfo`.
    message_types: Vec<String>,
    errors: Vec<String>,
    validation_errors: Vec<ValidationError>,
    error_stack: Vec<(Vec<String>, Vec<ValidationError>)>,
}

/// How many errors a [`MessageFacade`] had, so the ones added after can be taken back out.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ErrorMark {
    errors: usize,
    validation_errors: usize,
}

/// `ValidationError`: an error for one field of a form or service. Its text is also in the error list, so whatever
/// asks whether there are errors sees it, as Java's `hasError` counts validation errors.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ValidationError {
    pub form: String,
    pub field: String,
    pub service: String,
    pub message: String,
}

impl ValidationError {
    /// Java's `ValidationError.toStringPretty`: the message, then `(`, then `for field`, ` on form` and ` of service`
    /// with the pretty names that are not empty. As in Java there is no space before the `(` and no closing `)`.
    pub fn to_string_pretty(&self) -> String {
        let pretty = moqui_lib::string::camel_case_to_pretty;
        let mut text = self.message.clone();
        text.push('(');
        let field = pretty(&self.field);
        if !field.is_empty() {
            text.push_str("for field ");
            text.push_str(&field);
        }
        let form = pretty(&self.form);
        if !form.is_empty() {
            text.push_str(" on form ");
            text.push_str(&form);
        }
        let service = pretty(&self.service);
        if !service.is_empty() {
            text.push_str(" of service ");
            text.push_str(&service);
        }
        text
    }
}

impl MessageFacade {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn add_message(&mut self, message: impl Into<String>) {
        self.add_message_typed(message, "info");
    }

    /// A message with its type, `info` when blank.
    pub fn add_message_typed(&mut self, message: impl Into<String>, kind: &str) {
        let message = message.into();
        if message.is_empty() {
            return;
        }
        tracing::info!("{message}");
        self.messages.push(message);
        self.message_types
            .push(if kind.trim().is_empty() { "info" } else { kind.trim() }.to_string());
    }

    /// The type of the message at `index`.
    pub fn message_type(&self, index: usize) -> &str {
        self.message_types.get(index).map_or("info", String::as_str)
    }

    pub fn add_error(&mut self, error: impl Into<String>) {
        let error = error.into();
        if error.is_empty() {
            return;
        }
        tracing::error!("{error}");
        self.errors.push(error);
    }

    pub fn add_validation_error(&mut self, form: &str, field: &str, service: &str, message: &str) {
        if message.is_empty() {
            return;
        }
        let text = message.to_string();
        tracing::error!("{text} (field {field})");
        self.errors.push(text);
        self.validation_errors.push(ValidationError {
            form: form.to_string(),
            field: field.to_string(),
            service: service.to_string(),
            message: message.to_string(),
        });
    }

    pub fn validation_errors(&self) -> &[ValidationError] {
        &self.validation_errors
    }

    pub fn messages(&self) -> &[String] {
        &self.messages
    }

    pub fn errors(&self) -> &[String] {
        &self.errors
    }

    /// The lines of Java's `getErrorsString`: the errors, then each validation error as `toStringPretty` writes it.
    /// [`Self::errors`] holds a validation error's message too, so that one is not written twice.
    pub fn errors_text(&self) -> Vec<String> {
        let mut owned: Vec<&str> = self
            .validation_errors
            .iter()
            .map(|error| error.message.as_str())
            .collect();
        let mut lines = Vec::with_capacity(self.errors.len());
        for error in &self.errors {
            match owned.iter().position(|message| *message == error) {
                Some(index) => {
                    owned.swap_remove(index);
                }
                None => lines.push(error.clone()),
            }
        }
        lines.extend(self.validation_errors.iter().map(ValidationError::to_string_pretty));
        lines
    }

    /// [`Self::errors_text`] joined by newlines, as Java's `getErrorsString` (without its newline after the last).
    pub fn errors_string(&self) -> String {
        self.errors_text().join("\n")
    }

    /// Java `clearErrors`: the errors and validation errors, not the messages.
    pub fn clear_errors(&mut self) {
        self.errors.clear();
        self.validation_errors.clear();
    }

    /// Isolate validation errors without discarding the caller's errors or informational messages.
    pub fn push_errors(&mut self) {
        self.error_stack.push((
            std::mem::take(&mut self.errors),
            std::mem::take(&mut self.validation_errors),
        ));
    }

    pub fn pop_errors(&mut self) {
        if let Some((errors, validation_errors)) = self.error_stack.pop() {
            self.errors.extend(errors);
            self.validation_errors.extend(validation_errors);
        }
    }

    /// A mark for [`Self::take_errors_since`].
    pub fn error_mark(&self) -> ErrorMark {
        ErrorMark {
            errors: self.errors.len(),
            validation_errors: self.validation_errors.len(),
        }
    }

    /// The error texts added since `mark`, left in place.
    pub fn errors_since(&self, mark: ErrorMark) -> Vec<String> {
        self.errors
            .get(mark.errors..)
            .map(<[String]>::to_vec)
            .unwrap_or_default()
    }

    /// Removes the errors and validation errors added since `mark` and returns the error texts. The earlier ones stay.
    pub fn take_errors_since(&mut self, mark: ErrorMark) -> Vec<String> {
        self.validation_errors.truncate(mark.validation_errors);
        self.errors.split_off(mark.errors.min(self.errors.len()))
    }

    pub fn clear(&mut self) {
        self.messages.clear();
        self.message_types.clear();
        self.errors.clear();
        self.validation_errors.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::MessageFacade;

    #[test]
    fn nested_error_scopes_restore_validation_errors_without_losing_messages() {
        let mut facade = MessageFacade::new();
        facade.add_validation_error("form", "field", "svc", "caller");
        facade.add_message("before");
        facade.push_errors();
        assert!(facade.errors().is_empty());
        facade.add_error("outer");
        facade.push_errors();
        facade.add_validation_error("", "tool", "", "inner");
        facade.clear_errors();
        facade.add_message("during");
        facade.pop_errors();
        assert_eq!(facade.errors(), ["outer"]);
        assert!(facade.validation_errors().is_empty());
        facade.pop_errors();
        assert_eq!(facade.errors(), ["outer", "caller"]);
        assert_eq!(facade.validation_errors()[0].field, "field");
        assert_eq!(facade.messages(), ["before", "during"]);
        facade.pop_errors();
        assert_eq!(facade.errors(), ["outer", "caller"]);
    }

    /// Java's `getErrorsString`: the errors, then each validation error by `toStringPretty`, once.
    #[test]
    fn the_errors_string_writes_validation_errors_as_java_does() {
        let mut facade = MessageFacade::new();
        facade.add_error("plain");
        facade.add_validation_error(
            "",
            "emailAddress",
            "org.moqui.impl.UserServices.update#UserAccount",
            "Bad",
        );
        facade.add_validation_error("EditThing", "title", "", "Needs a letter");
        facade.add_validation_error("", "", "", "No field");
        assert_eq!(
            facade.errors_text(),
            [
                "plain",
                &format!(
                    "Bad(for field Email Address of service {}",
                    moqui_lib::string::camel_case_to_pretty("org.moqui.impl.UserServices.update#UserAccount")
                ),
                "Needs a letter(for field Title on form Edit Thing",
                "No field(",
            ]
        );
        assert_eq!(facade.errors(), ["plain", "Bad", "Needs a letter", "No field"]);
        assert_eq!(facade.errors_string(), facade.errors_text().join("\n"));
    }

    #[test]
    fn take_errors_since_leaves_the_earlier_errors_and_the_messages() {
        let mut facade = MessageFacade::new();
        facade.add_error("before");
        let mark = facade.error_mark();
        facade.add_message("info");
        facade.add_validation_error("", "field", "", "after");
        assert_eq!(facade.take_errors_since(mark), ["after"]);
        assert_eq!(facade.errors(), ["before"]);
        assert!(facade.validation_errors().is_empty());
        assert_eq!(facade.messages(), ["info"]);
        facade.clear_errors();
        assert!(facade.take_errors_since(mark).is_empty());
    }
}
