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

//! Localized expansion: `ec.l10n.localize` before `${}` expansion, as Java does.

use super::*;
use crate::l10n;

const LOCALIZE_LIMIT: usize = 256;

pub(crate) fn localized(template: &str) -> Result<String, Error> {
    if template.chars().count() < LOCALIZE_LIMIT {
        l10n::localize(template).map_err(|err| Error::new(err.message()))
    } else {
        Ok(template.to_string())
    }
}

/// `expand` with the localization Java does first. Nothing to expand is the localized text.
pub fn expand_localized(template: &str) -> Result<String, Error> {
    if template.is_empty() {
        return Ok(String::new());
    }
    expand(&localized(template)?)
}
