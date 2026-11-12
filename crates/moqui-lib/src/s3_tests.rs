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

use std::collections::BTreeMap;
use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::{Arc, Mutex};

use super::*;

fn aws_example() -> S3Config {
    S3Config {
        bucket: "examplebucket".into(),
        region: "us-east-1".into(),
        endpoint: None,
        access_key: "AKIAIOSFODNN7EXAMPLE".into(),
        secret_key: "wJalrXUtnFEMI/K7MDENG/bPxRfiCYEXAMPLEKEY".into(),
    }
}

/// The GET Object example of the AWS SigV4 documentation for S3 ("Example: GET Object").
#[test]
fn signs_the_aws_get_object_example() {
    let headers = vec![
        ("host".to_string(), "examplebucket.s3.amazonaws.com".to_string()),
        ("range".to_string(), "bytes=0-9".to_string()),
        ("x-amz-content-sha256".to_string(), EMPTY_SHA256.to_string()),
        ("x-amz-date".to_string(), "20130524T000000Z".to_string()),
    ];
    let header = authorization(
        "GET",
        "/test.txt",
        "",
        &headers,
        EMPTY_SHA256,
        &aws_example(),
        "20130524T000000Z",
    );
    assert_eq!(
        header,
        "AWS4-HMAC-SHA256 Credential=AKIAIOSFODNN7EXAMPLE/20130524/us-east-1/s3/aws4_request, \
         SignedHeaders=host;range;x-amz-content-sha256;x-amz-date, \
         Signature=f0e8bdb87c964420e857bd35b5d6ed310bd44f0170aba48dd91039c6036bdb41"
    );
}

#[test]
fn urls_name_a_bucket_region_and_endpoint() {
    let aws = S3Config::from_url("s3://backups?region=eu-west-1", "a", "s").unwrap();
    assert_eq!(
        (aws.bucket.as_str(), aws.region.as_str(), aws.endpoint.as_deref()),
        ("backups", "eu-west-1", None)
    );
    assert_eq!(aws.base().1, "backups.s3.eu-west-1.amazonaws.com");
    let other = S3Config::from_url("s3://b?endpoint=https%3A%2F%2Fminio.example%3A9000%2F", "a", "s").unwrap();
    assert_eq!(other.endpoint.as_deref(), Some("https://minio.example:9000"));
    assert_eq!(other.base(), ("https".into(), "minio.example:9000".into(), "/b".into()));
    assert!(S3Config::from_url("s3://b/prefix", "a", "s").is_err());
    assert!(S3Config::from_url("sftp://b", "a", "s").is_err());
    assert!(S3Config::from_url("s3://b?endpoint=ftp://x", "a", "s").is_err());
    assert_eq!(uri_encode("a b/c+d~", false), "a%20b/c%2Bd~");
    assert_eq!(
        canonical_query(&[("uploadId", "x/y".into()), ("partNumber", "2".into())]),
        "partNumber=2&uploadId=x%2Fy"
    );
}

/// An in-memory S3 of one bucket: list, put, copy, delete, multipart.
#[derive(Default)]
struct Store {
    objects: BTreeMap<String, Vec<u8>>,
    uploads: BTreeMap<String, BTreeMap<usize, Vec<u8>>>,
    requests: Vec<String>,
}

fn fake_s3() -> (S3Client, Arc<Mutex<Store>>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    let store = Arc::new(Mutex::new(Store::default()));
    let shared = store.clone();
    std::thread::spawn(move || {
        for stream in listener.incoming().flatten() {
            let store = shared.clone();
            std::thread::spawn(move || serve(stream, &store));
        }
    });
    let config = S3Config {
        bucket: "bk".into(),
        region: "us-east-1".into(),
        endpoint: Some(format!("http://127.0.0.1:{port}")),
        access_key: "a".into(),
        secret_key: "s".into(),
    };
    (S3Client::new(config), store)
}

fn serve(stream: TcpStream, store: &Mutex<Store>) {
    let mut reader = BufReader::new(stream.try_clone().unwrap());
    let mut out = stream;
    loop {
        let mut line = String::new();
        if reader.read_line(&mut line).unwrap_or(0) == 0 {
            return;
        }
        let mut parts = line.split_whitespace();
        let method = parts.next().unwrap_or("").to_string();
        let target = parts.next().unwrap_or("").to_string();
        let mut headers = BTreeMap::new();
        loop {
            let mut header = String::new();
            reader.read_line(&mut header).unwrap();
            let header = header.trim_end();
            if header.is_empty() {
                break;
            }
            let (name, value) = header.split_once(':').unwrap();
            headers.insert(name.to_ascii_lowercase(), value.trim().to_string());
        }
        let len: usize = headers.get("content-length").and_then(|v| v.parse().ok()).unwrap_or(0);
        let mut body = vec![0; len];
        reader.read_exact(&mut body).unwrap();
        assert!(headers
            .get("authorization")
            .is_some_and(|v| v.starts_with("AWS4-HMAC-SHA256 ")));
        let (status, extra, reply) = answer(&method, &target, &headers, body, store);
        let response = format!(
            "HTTP/1.1 {status} X\r\ncontent-length: {}\r\n{extra}\r\n{reply}",
            reply.len()
        );
        out.write_all(response.as_bytes()).unwrap();
    }
}

fn answer(
    method: &str,
    target: &str,
    headers: &BTreeMap<String, String>,
    body: Vec<u8>,
    store: &Mutex<Store>,
) -> (u16, String, String) {
    let mut store = store.lock().unwrap();
    let (path, query) = target.split_once('?').unwrap_or((target, ""));
    let query: BTreeMap<String, String> = query
        .split('&')
        .filter(|pair| !pair.is_empty())
        .map(|pair| {
            let (name, value) = pair.split_once('=').unwrap_or((pair, ""));
            (name.to_string(), percent_decode(value))
        })
        .collect();
    let key = percent_decode(path.strip_prefix("/bk").unwrap().trim_start_matches('/'));
    store.requests.push(format!("{method} {key}"));
    match method {
        "GET" => {
            let prefix = query.get("prefix").cloned().unwrap_or_default();
            let mut xml = String::from(
                "<?xml version=\"1.0\"?><ListBucketResult xmlns=\"http://s3.amazonaws.com/doc/2006-03-01/\"><IsTruncated>false</IsTruncated>",
            );
            for (name, data) in store.objects.iter().filter(|(name, _)| name.starts_with(&prefix)) {
                xml.push_str(&format!(
                    "<Contents><Key>{name}</Key><Size>{}</Size></Contents>",
                    data.len()
                ));
            }
            xml.push_str("</ListBucketResult>");
            (200, String::new(), xml)
        }
        "POST" if query.contains_key("uploads") => {
            let id = format!("up{}", store.uploads.len());
            store.uploads.insert(id.clone(), BTreeMap::new());
            (
                200,
                String::new(),
                format!("<InitiateMultipartUploadResult><UploadId>{id}</UploadId></InitiateMultipartUploadResult>"),
            )
        }
        "POST" => {
            let parts = store.uploads.remove(&query["uploadId"]).unwrap();
            let named = String::from_utf8(body).unwrap().matches("<Part>").count();
            assert_eq!(named, parts.len());
            let data: Vec<u8> = parts.into_values().flatten().collect();
            store.objects.insert(key, data);
            (200, String::new(), "<CompleteMultipartUploadResult/>".into())
        }
        "PUT" if query.contains_key("partNumber") => {
            let number: usize = query["partNumber"].parse().unwrap();
            store.uploads.get_mut(&query["uploadId"]).unwrap().insert(number, body);
            (200, format!("etag: \"e{number}\"\r\n"), String::new())
        }
        "PUT" => {
            let data = match headers.get("x-amz-copy-source") {
                Some(source) => {
                    let from = percent_decode(source.strip_prefix("/bk/").unwrap());
                    store.objects[&from].clone()
                }
                None => body,
            };
            store.objects.insert(key, data);
            (200, String::new(), String::new())
        }
        "DELETE" if query.contains_key("uploadId") => {
            store.uploads.remove(&query["uploadId"]);
            (204, String::new(), String::new())
        }
        "DELETE" => {
            store.objects.remove(&key);
            (204, String::new(), String::new())
        }
        _ => (400, String::new(), "<Error><Code>Bad</Code></Error>".into()),
    }
}

#[test]
fn objects_go_up_in_one_put_or_in_parts_and_are_copied_and_deleted() {
    let (client, store) = fake_s3();
    let mut small = client.upload("p/small one");
    small.write_all(b"tiny").unwrap();
    assert_eq!(small.finish().unwrap(), 4);

    let big: Vec<u8> = (0..FIRST_PART * 2 + 5).map(|i| (i % 253) as u8).collect();
    let mut upload = client.upload("p/big");
    for piece in big.chunks(1 << 20) {
        upload.write_all(piece).unwrap();
    }
    assert_eq!(upload.finish().unwrap(), big.len() as u64);
    assert_eq!(store.lock().unwrap().objects["p/big"], big);
    assert!(store.lock().unwrap().requests.contains(&"POST p/big".to_string()));

    let mut listed = client.list("p/").unwrap();
    listed.sort_by(|a, b| a.key.cmp(&b.key));
    assert_eq!(
        listed,
        [
            S3Object {
                key: "p/big".into(),
                size: big.len() as u64
            },
            S3Object {
                key: "p/small one".into(),
                size: 4
            },
        ]
    );
    client.copy("p/small one", "p/renamed", 4).unwrap();
    client.delete("p/small one").unwrap();
    let keys: Vec<String> = client.list("p/").unwrap().into_iter().map(|o| o.key).collect();
    assert_eq!(keys, ["p/big", "p/renamed"]);
    assert_eq!(store.lock().unwrap().objects["p/renamed"], b"tiny");
}

#[test]
fn an_upload_dropped_unfinished_is_aborted() {
    let (client, store) = fake_s3();
    let mut upload = client.upload("p/half");
    upload.write_all(&vec![1; FIRST_PART + 1]).unwrap();
    drop(upload);
    let store = store.lock().unwrap();
    assert!(store.uploads.is_empty());
    assert!(!store.objects.contains_key("p/half"));
}
