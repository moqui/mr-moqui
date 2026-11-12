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

//! When an X.509 certificate expires, read from its DER bytes: the `notAfter` of its validity, for the resource
//! readings (plans/archive/Tools-Fancy.md §11, certificate expiry). It reads the few fields on the way to the validity
//! and checks nothing else: a certificate is verified by the TLS stack, not here.

/// One DER element: its tag, its content, and what follows it.
struct Element<'a> {
    tag: u8,
    content: &'a [u8],
    rest: &'a [u8],
}

/// The element at the start of `bytes`: a one-byte tag and a definite length in the short or the long form.
fn element(bytes: &[u8]) -> Option<Element<'_>> {
    let (&tag, after_tag) = bytes.split_first()?;
    let (&first, after_first) = after_tag.split_first()?;
    let (length, body) = if first < 0x80 {
        (usize::from(first), after_first)
    } else {
        let count = usize::from(first & 0x7f);
        // the indefinite form (0) is not DER, and a length past four bytes is not a certificate's
        if count == 0 || count > 4 {
            return None;
        }
        let (digits, body) = after_first.split_at_checked(count)?;
        let length = digits
            .iter()
            .fold(0usize, |total, digit| (total << 8) | usize::from(*digit));
        (length, body)
    };
    let (content, rest) = body.split_at_checked(length)?;
    Some(Element { tag, content, rest })
}

const SEQUENCE: u8 = 0x30;
const UTC_TIME: u8 = 0x17;
const GENERALIZED_TIME: u8 = 0x18;
/// `[0]`, the explicit tag of a certificate's version.
const VERSION: u8 = 0xa0;

/// When the certificate in `der` stops being valid (its `notAfter`), in seconds since 1970-01-01 UTC; `None` when
/// the bytes are not a certificate this reads.
#[must_use]
pub fn not_after(der: &[u8]) -> Option<i64> {
    let certificate = element(der).filter(|found| found.tag == SEQUENCE)?;
    let tbs = element(certificate.content).filter(|found| found.tag == SEQUENCE)?;
    // version (optional), serialNumber, signature, issuer, then validity
    let mut field = element(tbs.content)?;
    if field.tag == VERSION {
        field = element(field.rest)?;
    }
    for _ in 0..3 {
        field = element(field.rest)?;
    }
    let validity = Some(field).filter(|found| found.tag == SEQUENCE)?;
    let not_before = element(validity.content)?;
    let not_after = element(not_before.rest)?;
    time(not_after.tag, not_after.content)
}

/// A `UTCTime` (`YYMMDDHHMMSSZ`, the year 1950 to 2049) or a `GeneralizedTime` (`YYYYMMDDHHMMSSZ`) as seconds since
/// 1970.
fn time(tag: u8, content: &[u8]) -> Option<i64> {
    let text = std::str::from_utf8(content).ok()?.strip_suffix('Z')?;
    if !text.bytes().all(|byte| byte.is_ascii_digit()) {
        return None;
    }
    let (year, rest) = match tag {
        UTC_TIME => {
            let (year, rest) = text.split_at_checked(2)?;
            let year: i64 = year.parse().ok()?;
            (if year >= 50 { 1900 + year } else { 2000 + year }, rest)
        }
        GENERALIZED_TIME => {
            let (year, rest) = text.split_at_checked(4)?;
            (year.parse().ok()?, rest)
        }
        _ => return None,
    };
    if rest.len() != 10 {
        return None;
    }
    let part = |at: usize| -> Option<i64> { rest.get(at..at + 2)?.parse().ok() };
    let (month, day, hour, minute, second) = (part(0)?, part(2)?, part(4)?, part(6)?, part(8)?);
    if !(1..=12).contains(&month) || !(1..=31).contains(&day) || hour > 23 || minute > 59 || second > 60 {
        return None;
    }
    Some(days_from_civil(year, month, day) * 86_400 + hour * 3_600 + minute * 60 + second)
}

/// The days from 1970-01-01 to a date of the proleptic Gregorian calendar.
fn days_from_civil(year: i64, month: i64, day: i64) -> i64 {
    let year = if month <= 2 { year - 1 } else { year };
    let era = year.div_euclid(400);
    let year_of_era = year.rem_euclid(400);
    let day_of_year = (153 * (if month > 2 { month - 3 } else { month + 9 }) + 2) / 5 + day - 1;
    let day_of_era = year_of_era * 365 + year_of_era / 4 - year_of_era / 100 + day_of_year;
    era * 146_097 + day_of_era - 719_468
}

#[cfg(test)]
mod tests {
    use super::{days_from_civil, element, not_after, time, GENERALIZED_TIME, UTC_TIME};

    #[test]
    fn a_time_is_read_in_both_forms_and_refused_when_it_is_not_one() {
        assert_eq!(days_from_civil(1970, 1, 1), 0);
        assert_eq!(days_from_civil(2000, 3, 1), 11_017);
        assert_eq!(time(UTC_TIME, b"700101000000Z"), Some(0));
        assert_eq!(time(UTC_TIME, b"261110120000Z"), Some(1_794_312_000));
        assert_eq!(time(GENERALIZED_TIME, b"20261110120000Z"), Some(1_794_312_000));
        assert_eq!(
            time(UTC_TIME, b"491231235959Z"),
            time(GENERALIZED_TIME, b"20491231235959Z")
        );
        assert_eq!(
            time(UTC_TIME, b"500101000000Z"),
            time(GENERALIZED_TIME, b"19500101000000Z")
        );
        for bad in [
            &b"2611101200Z"[..],
            b"261310120000Z",
            b"26111012000aZ",
            b"261110120000",
            b"",
        ] {
            assert_eq!(time(UTC_TIME, bad), None, "{bad:?}");
        }
        assert_eq!(time(0x04, b"261110120000Z"), None, "not a time's tag");
    }

    #[test]
    fn bytes_that_are_not_a_certificate_are_none_and_never_a_panic() {
        assert_eq!(not_after(&[]), None);
        assert_eq!(not_after(&[0x30]), None);
        assert_eq!(
            not_after(&[0x30, 0x82, 0xff, 0xff, 0x00]),
            None,
            "a length past the bytes"
        );
        assert_eq!(not_after(&[0x30, 0x80, 0x00, 0x00]), None, "the indefinite form");
        assert_eq!(not_after(&[0x04, 0x02, 0x30, 0x00]), None, "not a sequence");
        assert_eq!(not_after(&[0x30, 0x02, 0x30, 0x00]), None, "an empty certificate body");
        assert!(element(&[0x30, 0x81, 0x01, 0x05]).is_some_and(|found| found.content == [0x05]));
    }
}
