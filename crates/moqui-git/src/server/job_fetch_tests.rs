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

//! The lines a job token's fetch may send.

use gix::ObjectId;

use super::check_request;
use crate::server::pktline;

fn request(lines: &[String]) -> Vec<u8> {
    let mut body = Vec::new();
    for line in lines {
        pktline::text(&mut body, line).unwrap();
    }
    pktline::flush(&mut body).unwrap();
    body
}

/// SEC-GIT-82: `deepen-relative` counts the depth from the client's shallow commits, so a job token's fetch could go
/// deeper than `max-fetch-depth` a request at a time; it is refused as a v2 argument and as a v0 capability.
#[test]
fn a_job_tokens_fetch_is_refused_deepen_relative() {
    let commit = ObjectId::from_hex(b"1111111111111111111111111111111111111111").unwrap();
    let want = format!("want {commit}");
    assert!(check_request(&request(&[want.clone(), "deepen 1".into()]), &commit, 50).is_ok());
    let v2 = request(&[want.clone(), "deepen 1".into(), "deepen-relative".into()]);
    assert!(check_request(&v2, &commit, 50).is_err());
    let v0 = request(&[format!("{want} multi_ack deepen-relative"), "deepen 1".into()]);
    assert!(check_request(&v0, &commit, 50).is_err());
}
