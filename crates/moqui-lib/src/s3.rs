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

//! A small S3 client on blocking [`RestClient`], signed with AWS Signature Version 4: list, put (one request, or a
//! streamed multipart upload), server-side copy, and delete. It speaks to AWS and to the S3-compatible stores (MinIO,
//! Cloudflare R2, Backblaze B2, Wasabi, Garage) through `s3://bucket?region=...&endpoint=https://...`.

use std::io::{self, Read, Write};
use std::time::Duration;

use hmac::{KeyInit as _, Mac as _};
use sha2::{Digest as _, Sha256};

use crate::error::Error;
use crate::http::{Method, OpenRequest, RestClient};
use crate::node::MNode;

/// The first parts of a streamed upload; every 1000 parts the size doubles, so 10,000 parts hold over 16 TB.
const FIRST_PART: usize = 16 * 1024 * 1024;
/// The largest object one CopyObject copies; a larger one is copied in parts.
const COPY_LIMIT: u64 = 5 * 1024 * 1024 * 1024;
const COPY_PART: u64 = 1024 * 1024 * 1024;
const ATTEMPTS: u32 = 4;
/// The largest list or error response read.
const MAX_RESPONSE: u64 = 16 * 1024 * 1024;
const EMPTY_SHA256: &str = "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855";

/// Where the objects go and the keys that sign for them.
#[derive(Clone)]
pub struct S3Config {
    pub bucket: String,
    pub region: String,
    /// `https://host[:port]` of an S3-compatible store, addressed path-style; none is AWS, virtual-hosted.
    pub endpoint: Option<String>,
    pub access_key: String,
    pub secret_key: String,
}

impl std::fmt::Debug for S3Config {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("S3Config")
            .field("bucket", &self.bucket)
            .field("region", &self.region)
            .field("endpoint", &self.endpoint)
            .field("access_key", &self.access_key)
            .finish_non_exhaustive()
    }
}

impl S3Config {
    /// `s3://bucket`, with `region` (default `us-east-1`) and `endpoint` query parameters.
    ///
    /// # Errors
    ///
    /// When `url` is not an `s3://` URL with a bucket, or the endpoint is not http(s).
    pub fn from_url(url: &str, access_key: &str, secret_key: &str) -> Result<Self, Error> {
        let rest = url
            .trim()
            .strip_prefix("s3://")
            .ok_or_else(|| Error::new(format!("{url} is not an s3:// URL")))?;
        let (bucket, query) = rest.split_once('?').unwrap_or((rest, ""));
        let bucket = bucket.trim_end_matches('/');
        if bucket.is_empty() || bucket.contains('/') {
            return Err(Error::new(format!(
                "{url}: name the bucket alone (s3://bucket); the key prefix is the message type's sendPath"
            )));
        }
        let mut config = Self {
            bucket: bucket.to_string(),
            region: "us-east-1".to_string(),
            endpoint: None,
            access_key: access_key.to_string(),
            secret_key: secret_key.to_string(),
        };
        for pair in query.split('&').filter(|pair| !pair.is_empty()) {
            let (name, value) = pair.split_once('=').unwrap_or((pair, ""));
            let value = percent_decode(value);
            match name {
                "region" if !value.is_empty() => config.region = value,
                "endpoint" if !value.is_empty() => {
                    if !value.starts_with("https://") && !value.starts_with("http://") {
                        return Err(Error::new(format!("{url}: the endpoint must be an http(s) URL")));
                    }
                    config.endpoint = Some(value.trim_end_matches('/').to_string());
                }
                _ => {}
            }
        }
        Ok(config)
    }

    /// The scheme and host, and the path before the key.
    fn base(&self) -> (String, String, String) {
        match &self.endpoint {
            Some(endpoint) => {
                let (scheme, host) = endpoint.split_once("://").unwrap_or(("https", endpoint));
                (
                    scheme.to_string(),
                    host.to_string(),
                    format!("/{}", uri_encode(&self.bucket, true)),
                )
            }
            // a bucket name with a dot does not match the wildcard certificate as a host name
            None if self.bucket.contains('.') => (
                "https".to_string(),
                format!("s3.{}.amazonaws.com", self.region),
                format!("/{}", uri_encode(&self.bucket, true)),
            ),
            None => (
                "https".to_string(),
                format!("{}.s3.{}.amazonaws.com", self.bucket, self.region),
                String::new(),
            ),
        }
    }
}

/// One object a list returned.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct S3Object {
    pub key: String,
    pub size: u64,
}

pub struct S3Client {
    config: S3Config,
    http: RestClient,
}

/// One request before it is signed: the key (empty for the bucket) and query pairs, not encoded.
struct Request<'a> {
    method: Method,
    key: &'a str,
    query: Vec<(&'a str, String)>,
    headers: Vec<(String, String)>,
    body: Vec<u8>,
    timeout: Duration,
}

impl<'a> Request<'a> {
    fn new(method: Method, key: &'a str) -> Self {
        Self {
            method,
            key,
            query: Vec::new(),
            headers: Vec::new(),
            body: Vec::new(),
            timeout: Duration::from_secs(60),
        }
    }
}

struct Response {
    status: u16,
    headers: Vec<(String, String)>,
    body: String,
}

impl Response {
    fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(found, _)| found.eq_ignore_ascii_case(name))
            .map(|(_, value)| value.as_str())
    }
}

impl S3Client {
    #[must_use]
    pub fn new(config: S3Config) -> Self {
        Self {
            config,
            // a part of 16 MB and more on a slow link takes minutes; each request sets its own limit
            http: RestClient::new().timeout(Duration::from_secs(600)),
        }
    }

    #[must_use]
    pub fn config(&self) -> &S3Config {
        &self.config
    }

    /// Every object whose key starts with `prefix`.
    ///
    /// # Errors
    ///
    /// When a request fails or the store refuses it.
    pub fn list(&self, prefix: &str) -> Result<Vec<S3Object>, Error> {
        let mut found = Vec::new();
        let mut token: Option<String> = None;
        loop {
            let mut request = Request::new(Method::Get, "");
            request.query.push(("list-type", "2".to_string()));
            request.query.push(("prefix", prefix.to_string()));
            if let Some(token) = &token {
                request.query.push(("continuation-token", token.clone()));
            }
            let response = self.send(request)?;
            let root = parse_xml(&response.body)?;
            for contents in root.children_named("Contents") {
                let key = child_text(&contents, "Key");
                let size = child_text(&contents, "Size").parse().unwrap_or(0);
                found.push(S3Object { key, size });
            }
            token = Some(child_text(&root, "NextContinuationToken")).filter(|token| !token.is_empty());
            if child_text(&root, "IsTruncated") != "true" || token.is_none() {
                return Ok(found);
            }
        }
    }

    /// Put `body` as `key` in one request.
    ///
    /// # Errors
    ///
    /// When the request fails or the store refuses it.
    pub fn put(&self, key: &str, body: Vec<u8>) -> Result<(), Error> {
        let mut request = Request::new(Method::Put, key);
        request.timeout = part_timeout(body.len());
        request.body = body;
        self.send(request).map(|_| ())
    }

    /// # Errors
    ///
    /// When the request fails or the store refuses it. A missing key is no error.
    pub fn delete(&self, key: &str) -> Result<(), Error> {
        self.send(Request::new(Method::Delete, key)).map(|_| ())
    }

    /// Copy `from` (of `size` bytes) to `to` on the store, in parts past 5 GB.
    ///
    /// # Errors
    ///
    /// When a request fails or the store refuses it.
    pub fn copy(&self, from: &str, to: &str, size: u64) -> Result<(), Error> {
        let source = format!("/{}/{}", self.config.bucket, uri_encode(from, false));
        if size <= COPY_LIMIT {
            let mut request = Request::new(Method::Put, to);
            request.headers.push(("x-amz-copy-source".to_string(), source));
            request.timeout = Duration::from_secs(3600);
            return self.send(request).map(|_| ());
        }
        let upload_id = self.create_multipart(to)?;
        let copied = (|| {
            let mut parts = Vec::new();
            let mut start = 0;
            while start < size {
                let end = (start + COPY_PART).min(size) - 1;
                let number = parts.len() + 1;
                let mut request = Request::new(Method::Put, to);
                request.query.push(("partNumber", number.to_string()));
                request.query.push(("uploadId", upload_id.clone()));
                request.headers.push(("x-amz-copy-source".to_string(), source.clone()));
                request
                    .headers
                    .push(("x-amz-copy-source-range".to_string(), format!("bytes={start}-{end}")));
                request.timeout = Duration::from_secs(3600);
                let response = self.send(request)?;
                parts.push(child_text(&parse_xml(&response.body)?, "ETag"));
                start = end + 1;
            }
            self.complete_multipart(to, &upload_id, &parts)
        })();
        if copied.is_err() {
            self.abort_multipart(to, &upload_id);
        }
        copied
    }

    /// A streamed upload to `key`: what is written goes up in parts, and [`Upload::finish`] makes the object. Dropped
    /// before that, nothing is left on the store.
    #[must_use]
    pub fn upload<'a>(&'a self, key: &'a str) -> Upload<'a> {
        Upload {
            client: self,
            key,
            upload_id: None,
            parts: Vec::new(),
            buffer: Vec::new(),
            bytes: 0,
        }
    }

    fn create_multipart(&self, key: &str) -> Result<String, Error> {
        let mut request = Request::new(Method::Post, key);
        request.query.push(("uploads", String::new()));
        let response = self.send(request)?;
        let id = child_text(&parse_xml(&response.body)?, "UploadId");
        if id.is_empty() {
            return Err(Error::new(format!("S3 gave no UploadId for {key}")));
        }
        Ok(id)
    }

    fn upload_part(&self, key: &str, upload_id: &str, number: usize, body: Vec<u8>) -> Result<String, Error> {
        let mut request = Request::new(Method::Put, key);
        request.query.push(("partNumber", number.to_string()));
        request.query.push(("uploadId", upload_id.to_string()));
        request.timeout = part_timeout(body.len());
        request.body = body;
        let response = self.send(request)?;
        response
            .header("etag")
            .map(str::to_string)
            .ok_or_else(|| Error::new(format!("S3 gave no ETag for part {number} of {key}")))
    }

    fn complete_multipart(&self, key: &str, upload_id: &str, etags: &[String]) -> Result<(), Error> {
        let mut xml = String::from("<CompleteMultipartUpload>");
        for (index, etag) in etags.iter().enumerate() {
            let etag = etag.replace('&', "&amp;").replace('"', "&quot;");
            xml.push_str(&format!(
                "<Part><PartNumber>{}</PartNumber><ETag>{etag}</ETag></Part>",
                index + 1
            ));
        }
        xml.push_str("</CompleteMultipartUpload>");
        let mut request = Request::new(Method::Post, key);
        request.query.push(("uploadId", upload_id.to_string()));
        request.body = xml.into_bytes();
        request.timeout = Duration::from_secs(3600);
        self.send(request).map(|_| ())
    }

    fn abort_multipart(&self, key: &str, upload_id: &str) {
        let mut request = Request::new(Method::Delete, key);
        request.query.push(("uploadId", upload_id.to_string()));
        if let Err(err) = self.send(request) {
            tracing::error!("could not abort the S3 upload of {key}: {err}");
        }
    }

    /// Sign and send, again after a connection failure or a 5xx, up to [`ATTEMPTS`] times.
    fn send(&self, request: Request<'_>) -> Result<Response, Error> {
        let mut last = None;
        for attempt in 0..ATTEMPTS {
            if attempt > 0 {
                std::thread::sleep(Duration::from_secs(1 << (2 * attempt)));
            }
            match self.send_once(&request) {
                Ok(response) => return Ok(response),
                Err((err, retry)) => {
                    if !retry {
                        return Err(err);
                    }
                    tracing::warn!("S3 {} {}: {err}; trying again", request.method, request.key);
                    last = Some(err);
                }
            }
        }
        Err(last.unwrap_or_else(|| Error::new("S3 request failed")))
    }

    /// The error, and whether another attempt may succeed.
    fn send_once(&self, request: &Request<'_>) -> Result<Response, (Error, bool)> {
        let (scheme, host, base) = self.config.base();
        let path = format!("{base}/{}", uri_encode(request.key, false));
        let query = canonical_query(&request.query);
        let payload_hash = if request.body.is_empty() {
            EMPTY_SHA256.to_string()
        } else {
            hex(&Sha256::digest(&request.body))
        };
        let amz_date = jiff::Timestamp::now().strftime("%Y%m%dT%H%M%SZ").to_string();
        let mut headers = request.headers.clone();
        headers.push(("host".to_string(), host.clone()));
        headers.push(("x-amz-content-sha256".to_string(), payload_hash.clone()));
        headers.push(("x-amz-date".to_string(), amz_date.clone()));
        let authorization = authorization(
            &request.method.to_string(),
            &path,
            &query,
            &headers,
            &payload_hash,
            &self.config,
            &amz_date,
        );
        headers.retain(|(name, _)| name != "host");
        headers.push(("authorization".to_string(), authorization));
        let url = if query.is_empty() {
            format!("{scheme}://{host}{path}")
        } else {
            format!("{scheme}://{host}{path}?{query}")
        };
        let opened = self
            .http
            .open(OpenRequest {
                method: request.method,
                url,
                headers,
                body: Some(bytes::Bytes::from(request.body.clone())),
                timeout: Some(request.timeout),
            })
            .map_err(|err| (err, true))?;
        let status = opened.status;
        let mut body = String::new();
        opened
            .reader
            .take(MAX_RESPONSE)
            .read_to_string(&mut body)
            .map_err(|err| (Error::new("could not read the S3 response").with_source(err), true))?;
        let response = Response {
            status,
            headers: opened.headers,
            body,
        };
        // CompleteMultipartUpload and CopyObject can fail after a 200, with an <Error> body
        let error_body = response.body.trim_start().starts_with("<?xml") && response.body.contains("<Error>");
        if (200..300).contains(&status) && !error_body {
            return Ok(response);
        }
        if request.method == Method::Delete && status == 404 {
            return Ok(response);
        }
        let (code, message) = parse_xml(&response.body)
            .map(|root| (child_text(&root, "Code"), child_text(&root, "Message")))
            .unwrap_or_default();
        let retry = status >= 500 || status == 429 || matches!(code.as_str(), "SlowDown" | "InternalError");
        let target = if request.key.is_empty() {
            &self.config.bucket
        } else {
            request.key
        };
        Err((
            Error::new(format!(
                "S3 {} {target} returned {} {code}: {message}",
                request.method, response.status
            )),
            retry,
        ))
    }
}

/// A streamed upload: [`Write`] it, then [`finish`](Self::finish).
pub struct Upload<'a> {
    client: &'a S3Client,
    key: &'a str,
    upload_id: Option<String>,
    parts: Vec<String>,
    buffer: Vec<u8>,
    bytes: u64,
}

impl Upload<'_> {
    fn part_size(&self) -> usize {
        let doublings = (self.parts.len() / 1000).min(8);
        FIRST_PART << doublings
    }

    fn send_part(&mut self) -> Result<(), Error> {
        let upload_id = match &self.upload_id {
            Some(id) => id.clone(),
            None => {
                let id = self.client.create_multipart(self.key)?;
                self.upload_id = Some(id.clone());
                id
            }
        };
        let body = std::mem::take(&mut self.buffer);
        let etag = self
            .client
            .upload_part(self.key, &upload_id, self.parts.len() + 1, body)?;
        self.parts.push(etag);
        Ok(())
    }

    /// Send what is left and make the object. Returns the bytes uploaded.
    ///
    /// # Errors
    ///
    /// When a request fails; the parts sent are then removed.
    pub fn finish(mut self) -> Result<u64, Error> {
        let finished = if self.upload_id.is_none() {
            let body = std::mem::take(&mut self.buffer);
            self.client.put(self.key, body)
        } else {
            self.send_part().and_then(|()| {
                let id = self.upload_id.clone().unwrap_or_default();
                self.client.complete_multipart(self.key, &id, &self.parts)
            })
        };
        if finished.is_ok() {
            self.upload_id = None;
        }
        finished.map(|()| self.bytes)
    }
}

impl Write for Upload<'_> {
    fn write(&mut self, data: &[u8]) -> io::Result<usize> {
        if data.is_empty() {
            return Ok(0);
        }
        if self.buffer.len() >= self.part_size() {
            self.send_part().map_err(io::Error::other)?;
        }
        let size = self.part_size();
        if self.buffer.capacity() < size {
            self.buffer.reserve_exact(size - self.buffer.len());
        }
        let take = data.len().min(size - self.buffer.len());
        self.buffer.extend_from_slice(&data[..take]);
        self.bytes += take as u64;
        Ok(take)
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

impl Drop for Upload<'_> {
    fn drop(&mut self) {
        if let Some(id) = self.upload_id.take() {
            self.client.abort_multipart(self.key, &id);
        }
    }
}

fn part_timeout(len: usize) -> Duration {
    // at least 100 KB/s, and a minute for the round trips
    Duration::from_secs(60 + (len / 100_000) as u64)
}

fn parse_xml(text: &str) -> Result<MNode, Error> {
    MNode::parse_text("s3-response", text)
}

fn child_text(node: &MNode, name: &str) -> String {
    node.first_named(name)
        .and_then(|child| child.text())
        .unwrap_or_default()
}

/// SigV4's URI encoding: unreserved characters as they are, every other byte `%XX`; `/` too when `slash`.
fn uri_encode(text: &str, slash: bool) -> String {
    let mut out = String::with_capacity(text.len());
    for byte in text.bytes() {
        if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.' | b'~') || (byte == b'/' && !slash) {
            out.push(char::from(byte));
        } else {
            out.push_str(&format!("%{byte:02X}"));
        }
    }
    out
}

fn percent_decode(text: &str) -> String {
    let bytes = text.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut at = 0;
    while at < bytes.len() {
        if bytes[at] == b'%' {
            if let Some(byte) = text
                .get(at + 1..at + 3)
                .and_then(|hex| u8::from_str_radix(hex, 16).ok())
            {
                out.push(byte);
                at += 3;
                continue;
            }
        }
        out.push(bytes[at]);
        at += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

fn canonical_query(query: &[(&str, String)]) -> String {
    let mut pairs: Vec<(String, String)> = query
        .iter()
        .map(|(name, value)| (uri_encode(name, true), uri_encode(value, true)))
        .collect();
    pairs.sort();
    pairs
        .iter()
        .map(|(name, value)| format!("{name}={value}"))
        .collect::<Vec<_>>()
        .join("&")
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

fn hmac(key: &[u8], data: &str) -> Vec<u8> {
    // HMAC takes a key of any length
    let mut mac = hmac::Hmac::<Sha256>::new_from_slice(key).unwrap_or_else(|_| unreachable!());
    mac.update(data.as_bytes());
    mac.finalize().into_bytes().to_vec()
}

/// The `Authorization` header of a request: `path` and `query` already encoded, `headers` with `host` and every
/// `x-amz-` header, all of them signed.
fn authorization(
    method: &str,
    path: &str,
    query: &str,
    headers: &[(String, String)],
    payload_hash: &str,
    config: &S3Config,
    amz_date: &str,
) -> String {
    let mut signed: Vec<(String, String)> = headers
        .iter()
        .map(|(name, value)| (name.to_ascii_lowercase(), value.trim().to_string()))
        .collect();
    signed.sort();
    let canonical_headers: String = signed.iter().map(|(name, value)| format!("{name}:{value}\n")).collect();
    let signed_names = signed
        .iter()
        .map(|(name, _)| name.as_str())
        .collect::<Vec<_>>()
        .join(";");
    let canonical = format!("{method}\n{path}\n{query}\n{canonical_headers}\n{signed_names}\n{payload_hash}");
    let date = &amz_date[..8];
    let scope = format!("{date}/{}/s3/aws4_request", config.region);
    let to_sign = format!(
        "AWS4-HMAC-SHA256\n{amz_date}\n{scope}\n{}",
        hex(&Sha256::digest(canonical.as_bytes()))
    );
    let mut key = hmac(format!("AWS4{}", config.secret_key).as_bytes(), date);
    for part in [config.region.as_str(), "s3", "aws4_request"] {
        key = hmac(&key, part);
    }
    let signature = hex(&hmac(&key, &to_sign));
    format!(
        "AWS4-HMAC-SHA256 Credential={}/{scope}, SignedHeaders={signed_names}, Signature={signature}",
        config.access_key
    )
}

#[cfg(test)]
#[path = "s3_tests.rs"]
mod tests;
