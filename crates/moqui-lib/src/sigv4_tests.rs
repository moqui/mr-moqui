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

use super::*;

/// The example of AWS's SigV4 documentation ("Signature Version 4 signing process", the IAM `ListUsers` request of
/// 2015-08-30), whose canonical request, string to sign and signature it publishes.
#[test]
fn signs_the_published_aws_example() {
    let headers = vec![
        (
            "Content-Type".to_string(),
            "application/x-www-form-urlencoded; charset=utf-8".to_string(),
        ),
        ("Host".to_string(), "iam.amazonaws.com".to_string()),
        ("X-Amz-Date".to_string(), "20150830T123600Z".to_string()),
    ];
    let query = canonical_query(&[("Action", "ListUsers".into()), ("Version", "2010-05-08".into())]);
    assert_eq!(query, "Action=ListUsers&Version=2010-05-08");
    let header = authorization(
        &Request {
            method: "GET",
            path: "/",
            query: &query,
            headers: &headers,
            payload_hash: EMPTY_SHA256,
        },
        &Credentials {
            access_key: "AKIDEXAMPLE",
            secret_key: "wJalrXUtnFEMI/K7MDENG+bPxRfiCYEXAMPLEKEY",
        },
        Scope {
            region: "us-east-1",
            service: "iam",
        },
        "20150830T123600Z",
    );
    assert_eq!(
        header,
        "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20150830/us-east-1/iam/aws4_request, \
         SignedHeaders=content-type;host;x-amz-date, \
         Signature=5d672d79c15b13162d9279b0855cfba6789a8edb4c82c400e06b5924a6f2b5d7"
    );
    assert_eq!(sha256_hex(b""), EMPTY_SHA256);
    assert_eq!(uri_encode("a b/c+d~", false), "a%20b/c%2Bd~");
    assert_eq!(uri_encode("a/b", true), "a%2Fb");
    let debug = format!(
        "{:?}",
        Credentials {
            access_key: "AKIDEXAMPLE",
            secret_key: "the-secret"
        }
    );
    assert!(debug.contains("AKIDEXAMPLE") && !debug.contains("the-secret"));
}
