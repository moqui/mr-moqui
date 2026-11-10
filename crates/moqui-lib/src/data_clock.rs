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

//! Supplied data timestamps for isolated acceptance runs; elapsed execution deadlines stay real.
use std::ffi::OsStr;
use std::fs::File;
use std::io::{self, Read};
use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

pub const ENV: &str = "MOQUI_TEST_DATA_CLOCK_FILE";
const MAX_MILLIS: i64 = 253_402_300_799_999;

/// Change 10: each read validates the supplied assignment; no stale value or wall-clock fallback on error.
pub fn try_now_millis() -> io::Result<i64> {
    let setting = std::env::var_os(ENV);
    let millis = from_setting(setting.as_deref())?;
    if setting.is_some() {
        static ACK: std::sync::Once = std::sync::Once::new();
        ACK.call_once(|| eprintln!("MOQUI_TEST_DATA_CLOCK_FILE active epoch_ms={millis}"));
    }
    Ok(millis)
}

fn from_setting(setting: Option<&OsStr>) -> io::Result<i64> {
    let Some(setting) = setting else {
        return SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_err(io::Error::other)
            .and_then(|elapsed| i64::try_from(elapsed.as_millis()).map_err(io::Error::other));
    };
    read_assignment(Path::new(setting))
}

/// Read one atomically published ASCII epoch-millisecond assignment from an absolute regular file.
pub fn read_assignment(path: &Path) -> io::Result<i64> {
    if !path.is_absolute() {
        return Err(io::Error::other("test data clock requires an absolute file path"));
    }
    // Reject named pipes/devices before opening; the harness owns this private file.
    if !std::fs::metadata(path)?.is_file() {
        return Err(io::Error::other("test data clock requires a regular file"));
    }
    let file = File::open(path)?;
    if !file.metadata()?.is_file() {
        return Err(io::Error::other("test data clock requires a regular file"));
    }
    let mut bytes = Vec::new();
    file.take(33).read_to_end(&mut bytes)?;
    if bytes.last() == Some(&b'\n') {
        bytes.pop();
    }
    if bytes.is_empty() || bytes.len() > 15 || !bytes.iter().all(u8::is_ascii_digit) {
        return Err(io::Error::other(
            "test data clock requires positive ASCII epoch milliseconds",
        ));
    }
    let millis = std::str::from_utf8(&bytes)
        .map_err(io::Error::other)?
        .parse::<i64>()
        .map_err(io::Error::other)?;
    if !(1..=MAX_MILLIS).contains(&millis) {
        return Err(io::Error::other(
            "test data clock epoch is outside years 1970 through 9999",
        ));
    }
    Ok(millis)
}

/// Infallible legacy consumers must stop an invalid enabled run, never silently write wall-clock values.
pub fn now_millis() -> i64 {
    match try_now_millis() {
        Ok(millis) => millis,
        Err(_) => {
            eprintln!("invalid MOQUI_TEST_DATA_CLOCK_FILE assignment; stopping before data-clock fallback");
            std::process::exit(78);
        }
    }
}

#[cfg(test)]
#[path = "data_clock_tests.rs"]
mod tests;
