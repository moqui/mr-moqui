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

//! The device mode of an account (plans/Mobile-Desktop.md §6, MD4): what a sign-in asks for, what the account keeps,
//! and what sign-out and a revocation do with its data.
//!
//! - The build's conf decides (`<client-conf device-mode>`): `personal` or `shared` for every account, or `choose`, where
//!   the sign-in asks.
//! - The mode is kept with the account when it is created here. An account already `personal` stays personal when a
//!   sign-in asks for `shared` (its mirrored data is not trimmed), and the user is told.
//! - Sign-out and a revocation delete a `shared` account's data and the account itself; a `personal` one keeps all.
//!   An account with no mode (from before MD4) follows the build's mode when it names one, else keeps its data.

use crate::account::kept_mode;
use crate::conf::DeviceMode;

/// What the shell tells the user when a sign-in asked for shared and the account stays personal.
pub const KEPT_PERSONAL: &str = "This account already has data kept for a personal device on this computer, so \
    it runs as a personal device: its data stays here after you sign out.";

/// The mode a sign-in asks for: the conf's when it is not `choose`, else what the user chose (`personal` or `shared`);
/// none when nothing was chosen.
///
/// # Errors
///
/// Returns an error for a choice that is not `personal` or `shared`.
pub fn wanted(conf: DeviceMode, chosen: Option<&str>) -> Result<Option<DeviceMode>, String> {
    if conf != DeviceMode::Choose {
        return Ok(Some(conf));
    }
    match chosen.map(str::trim).filter(|text| !text.is_empty()) {
        None => Ok(None),
        Some(text) => match DeviceMode::parse(text) {
            Some(DeviceMode::Choose) | None => Err(format!("{text} is not personal or shared")),
            Some(mode) => Ok(Some(mode)),
        },
    }
}

/// The mode an account runs as after a sign-in that asked for `wanted`, given the mode it is kept with (`kept`, none
/// for an account that is new or has none), and whether the user is told that it stays personal.
pub fn resolved(kept: Option<DeviceMode>, wanted: Option<DeviceMode>) -> (Option<DeviceMode>, bool) {
    let mode = kept_mode(kept, wanted);
    let told = kept == Some(DeviceMode::Personal) && wanted == Some(DeviceMode::Shared);
    (mode, told)
}

/// The mode an account's data is kept by: its own; for an account with none (from before MD4, or signed in with
/// nothing chosen), the build's when the conf names one (`conf`), else personal, as such an account kept its answers on
/// sign-out before.
pub(crate) fn effective(mode: Option<DeviceMode>, conf: DeviceMode) -> DeviceMode {
    match (mode, conf) {
        (Some(DeviceMode::Shared), _) | (None, DeviceMode::Shared) => DeviceMode::Shared,
        _ => DeviceMode::Personal,
    }
}

/// Whether sign-out, or a revocation of the key, deletes the account's data and the account.
pub fn forgets_on_sign_out(mode: Option<DeviceMode>, conf: DeviceMode) -> bool {
    effective(mode, conf) == DeviceMode::Shared
}

#[cfg(test)]
#[path = "modes_tests.rs"]
mod tests;
