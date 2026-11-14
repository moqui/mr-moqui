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

//! What is read of a push before its pack: the commands, shallow lines and push options, within limits.

use super::*;

fn pkt(line: &str) -> Vec<u8> {
    format!("{:04x}{line}", line.len() + 4).into_bytes()
}

/// A request of `commands` commands (the first with the capabilities), `shallow` shallow lines and `options` push
/// options.
fn request(commands: usize, shallow: usize, options: usize) -> Vec<u8> {
    let zero = "0".repeat(40);
    let id = "1".repeat(40);
    let mut body = Vec::new();
    for _ in 0..shallow {
        body.extend(pkt(&format!("shallow {id}\n")));
    }
    for at in 0..commands {
        let caps = if at == 0 { "\0report-status push-options" } else { "" };
        body.extend(pkt(&format!("{zero} {id} refs/heads/b{at}{caps}\n")));
    }
    body.extend_from_slice(b"0000");
    for at in 0..options {
        body.extend(pkt(&format!("option-{at}\n")));
    }
    body.extend_from_slice(b"0000");
    body
}

fn read(body: &[u8], limits: RequestLimits) -> Result<Option<PushRequest>, Error> {
    read_request(&mut &body[..], &limits)
}

#[test]
fn a_request_is_read_within_its_limits() {
    let limits = RequestLimits { lines: 3, bytes: 4096 };
    let taken = read(&request(3, 3, 3), limits).unwrap().unwrap();
    assert_eq!(
        (taken.commands.len(), taken.shallow.len(), taken.options.len()),
        (3, 3, 3)
    );
    // one more of each kind is too many
    for (commands, shallow, options) in [(4, 0, 0), (1, 4, 0), (1, 0, 4)] {
        let refused = read(&request(commands, shallow, options), limits).unwrap_err();
        assert!(
            refused.message().starts_with("GitPushRequestTooLarge"),
            "{commands} {shallow} {options}: {refused}"
        );
    }
    // and so are more bytes than the limit, however few the lines
    let body = request(2, 0, 2);
    let small = RequestLimits {
        lines: 100,
        bytes: body.len() / 2,
    };
    assert!(read(&body, small).is_err());
    assert!(read(
        &body,
        RequestLimits {
            lines: 100,
            bytes: body.len()
        }
    )
    .is_ok());
}
