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

//! The tiers of access (plans/design-dev-aide.md 3.4): `DEV_AIDE_VIEW` reads, `DEV_AIDE_EXTEND` also writes inside site
//! components, `DEV_AIDE` does everything. Every dev-aide service asks [`require`] first, which also refuses on any
//! instance purpose but dev: the tools give code execution as the server's OS user (D15), and refuses a write to a
//! request logged in by a header.

use moqui_lib::error::Error;

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Tier {
    View,
    Extend,
    Full,
}

impl Tier {
    #[must_use]
    pub fn name(self) -> &'static str {
        match self {
            Self::View => "view",
            Self::Extend => "extend",
            Self::Full => "full",
        }
    }
}

/// The current user's tier, `None` with no dev-aide permission. `ADMIN` is the full tier, as its seed grants.
#[must_use]
pub fn current() -> Option<Tier> {
    let has = |permission: &str| moqui_core::user::has_permission(permission).unwrap_or(false);
    if has("DEV_AIDE") || moqui_core::user::is_in_group("ADMIN").unwrap_or(false) {
        Some(Tier::Full)
    } else if has("DEV_AIDE_EXTEND") {
        Some(Tier::Extend)
    } else if has("DEV_AIDE_VIEW") {
        Some(Tier::View)
    } else {
        None
    }
}

/// The current user's tier when it is `least` or above, on a dev instance.
///
/// # Errors
///
/// Returns an error saying why otherwise.
pub fn require(least: Tier) -> Result<Tier, Error> {
    if !moqui_core::devtools::is_dev_instance() {
        return Err(Error::new("Dev AIDE is only available when instance_purpose is dev"));
    }
    // a program that logs in by a header (an API key) reads, but never writes files or runs git (15)
    if least >= Tier::Extend && moqui_core::screen::gate::header_authenticated_request() {
        return Err(Error::new(
            "Dev AIDE takes no write from a request logged in by a header (an API key); use a session",
        ));
    }
    match current() {
        Some(tier) if tier >= least => Ok(tier),
        Some(tier) => Err(Error::new(format!(
            "This needs the {} tier of Dev AIDE; you have {}",
            least.name(),
            tier.name()
        ))),
        None => Err(Error::new(
            "Dev AIDE needs the DEV_AIDE_VIEW, DEV_AIDE_EXTEND or DEV_AIDE permission",
        )),
    }
}
