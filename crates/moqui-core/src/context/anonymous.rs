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

//! The anonymous side of an artifact frame (`anonymous-all`, `anonymous-view`), shared by the screen, REST path,
//! service and empty-database load frames (plans/archive/High-Level-Review.md HL-004): the inheritable grant on the top frame
//! ([`super::ExecutionContext::grant_anonymous`]) and the anonymous login the frame starts when nobody is logged in
//! ([`AnonymousLogin`]), ended with the frame.
//!
//! What differs between the frames stays with each caller, in the open: the order of login and grant, whether a login
//! error fails the frame, when the login ends against the frame pops, and the identity the grant is for.

use super::Error;

/// The actions an anonymous frame grants to what runs under it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AnonymousAccess {
    /// `anonymous-all`: every action, as Java's `setAnonymousAuthorizedAll`.
    All,
    /// `anonymous-view`: view, unless the frame already has all actions, as Java's `setAnonymousAuthorizedView`.
    View,
}

impl AnonymousAccess {
    /// The access of an `authenticate` or `require-authentication` value: `anonymous-all` or `anonymous-view`, else
    /// none.
    #[must_use]
    pub fn from_mode(mode: &str) -> Option<Self> {
        match mode {
            "anonymous-all" => Some(Self::All),
            "anonymous-view" => Some(Self::View),
            _ => None,
        }
    }
}

/// The anonymous login started for one or more anonymous frames. [`AnonymousLogin::begin`] logs the session in as
/// anonymous only when nobody is logged in and it is not anonymous already, so a nested frame or a logged-in user
/// starts nothing; only a login this started is ended, by [`AnonymousLogin::end`] or when this drops.
#[derive(Debug, Default)]
#[must_use = "dropping it ends the anonymous login it started"]
pub struct AnonymousLogin {
    started: bool,
}

impl AnonymousLogin {
    /// Log in as anonymous if nobody is logged in, remembering whether this did.
    ///
    /// # Errors
    ///
    /// Returns an error when the session cannot be read. Nothing is started then.
    pub fn begin(&mut self) -> Result<(), Error> {
        if crate::user::login_anonymous()? {
            self.started = true;
        }
        Ok(())
    }

    /// End the anonymous login this started, leaving a real login alone. A caller that must order the end against its
    /// frame pops calls this; otherwise dropping does it.
    pub fn end(&mut self) {
        if std::mem::take(&mut self.started) {
            crate::user::logout_anonymous_only();
        }
    }
}

impl Drop for AnonymousLogin {
    fn drop(&mut self) {
        self.end();
    }
}

#[cfg(test)]
#[path = "anonymous_tests.rs"]
mod tests;
