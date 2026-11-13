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

use moqui_tui::{CodeAsk, Sendable};

use super::*;

#[test]
fn plain_http_is_for_this_machine_only() {
    assert_eq!(base_url("http://localhost:8080/").unwrap(), "http://localhost:8080");
    assert_eq!(base_url("https://erp.example.com").unwrap(), "https://erp.example.com");
    assert_eq!(
        base_url("https://erp.example.com/moqui/").unwrap(),
        "https://erp.example.com/moqui"
    );
    assert!(base_url("http://127.0.0.1:8080").is_ok());
    assert!(base_url("http://erp.example.com").is_err());
    assert!(base_url("ftp://localhost").is_err());
}

#[test]
fn the_key_file_is_the_owners_alone() {
    let dir = std::env::temp_dir().join(format!("moqui-tui-key-{}", std::process::id()));
    let path = dir.join("moqui/tui/localhost_8080.toml");
    let stored = Stored {
        user: "john.doe".to_string(),
        key: "abc123".to_string(),
    };
    write_stored(&path, &stored);
    assert_eq!(read_stored(&path), Some(stored));
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = std::fs::metadata(&path).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o600);
    }
    let _ = std::fs::remove_dir_all(&dir);
    assert!(key_file("http://localhost:8080")
        .unwrap()
        .ends_with("moqui/tui/localhost_8080.toml"));
}

/// A request the fake server took.
#[derive(Debug, Clone)]
struct Taken {
    path: String,
    headers: BTreeMap<String, String>,
    form: BTreeMap<String, String>,
}

/// A server on this machine that answers each request with `answer` (status, extra headers, JSON body) and keeps what
/// it took. One request per connection.
fn fake_server(
    answer: impl Fn(&Taken) -> (u16, Vec<(&'static str, &'static str)>, String) + Send + 'static,
) -> (String, Arc<Mutex<Vec<Taken>>>) {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let base = format!("http://127.0.0.1:{}", listener.local_addr().unwrap().port());
    let taken = Arc::new(Mutex::new(Vec::new()));
    let kept = Arc::clone(&taken);
    std::thread::spawn(move || {
        for stream in listener.incoming() {
            let Ok(mut stream) = stream else { return };
            let mut reader = BufReader::new(stream.try_clone().unwrap());
            let mut line = String::new();
            reader.read_line(&mut line).unwrap();
            let path = line.split(' ').nth(1).unwrap_or("").to_string();
            let mut headers = BTreeMap::new();
            loop {
                let mut header = String::new();
                reader.read_line(&mut header).unwrap();
                let header = header.trim_end();
                if header.is_empty() {
                    break;
                }
                if let Some((name, value)) = header.split_once(':') {
                    headers.insert(name.trim().to_ascii_lowercase(), value.trim().to_string());
                }
            }
            let length = headers.get("content-length").and_then(|n| n.parse().ok()).unwrap_or(0);
            let mut body = vec![0; length];
            reader.read_exact(&mut body).unwrap();
            let form = reqwest::Url::parse(&format!("http://form/?{}", String::from_utf8_lossy(&body)))
                .unwrap()
                .query_pairs()
                .map(|(key, value)| (key.into_owned(), value.into_owned()))
                .collect();
            let request = Taken { path, headers, form };
            let (status, extra, json) = answer(&request);
            lock(&kept).push(request);
            let mut response = format!(
                "HTTP/1.1 {status} X\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n",
                json.len()
            );
            for (name, value) in extra {
                response.push_str(&format!("{name}: {value}\r\n"));
            }
            response.push_str("\r\n");
            response.push_str(&json);
            let _ = stream.write_all(response.as_bytes());
        }
    });
    (base, taken)
}

/// The sign-in the shared remote_login makes, through the TUI's link, against a fake server: the second factor asked
/// for, a code sent in the pre-authenticated session, the code taken, the key stored and named as this device's.
#[test]
fn the_tui_signs_in_with_a_second_factor_and_keeps_the_key() {
    let (base, taken) = fake_server(|request| match (request.path.as_str(), request.form.get("code")) {
        ("/rest/login", None) => (
            200,
            vec![
                ("Set-Cookie", "JSESSIONID=s1; Path=/; HttpOnly"),
                ("X-CSRF-Token", "t1"),
            ],
            r#"{"secondFactorRequired": true, "factorTypeDescriptions": ["Email"],
                "sendableFactors": [{"factorId": "F1", "factorOption": "a***@example.com"}]}"#
                .to_string(),
        ),
        ("/rest/sendOtp", _) => {
            let session = request.headers.get("cookie").map(String::as_str) == Some("JSESSIONID=s1")
                && request.headers.get("x-csrf-token").map(String::as_str) == Some("t1");
            if session {
                (200, vec![], "{}".to_string())
            } else {
                (403, vec![], r#"{"errors": "no session"}"#.to_string())
            }
        }
        ("/rest/login", Some(code)) if code == "123456" => {
            (200, vec![], r#"{"loggedIn": true, "loginKey": "K1"}"#.to_string())
        }
        _ => (
            401,
            vec![],
            r#"{"errors": "Authentication code is not valid"}"#.to_string(),
        ),
    });
    let dir = std::env::temp_dir().join(format!("moqui-tui-signin-{}", std::process::id()));
    let path = dir.join("key.toml");
    let http = Http::new(base, Some(path.clone())).unwrap();

    let Err(LinkError::Code(ask)) = http.login("ada", "Secret1!", "") else {
        panic!("no code asked");
    };
    assert_eq!(
        ask,
        CodeAsk {
            text: "Enter the authentication code (Email)".to_string(),
            sendable: vec![Sendable {
                id: "F1".to_string(),
                label: "a***@example.com".to_string()
            }],
        }
    );
    assert_eq!(http.send_code("ada", "Secret1!", "F1").unwrap(), "F1");
    assert_eq!(http.user(), None);
    http.login("ada", "Secret1!", "123456").unwrap();
    assert_eq!(http.user().as_deref(), Some("ada"));
    assert_eq!(stored_key(&path).as_deref(), Some("K1"));

    let taken = lock(&taken).clone();
    let paths: Vec<&str> = taken.iter().map(|request| request.path.as_str()).collect();
    assert_eq!(paths, ["/rest/login", "/rest/login", "/rest/sendOtp", "/rest/login"]);
    // the pre-authenticating login asks for no key
    assert_eq!(taken[1].form.get("createLoginKey"), None);
    assert_eq!(taken[2].form.get("factorId").map(String::as_str), Some("F1"));
    let last = &taken[3];
    assert_eq!(last.form.get("createLoginKey").map(String::as_str), Some("true"));
    assert_eq!(last.form.get("deviceName"), Some(&device_name()));
    assert!(device_name().starts_with("moqui tui on "));
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn screen_redirects_keep_session_cookies_in_memory_with_their_scope() {
    let (base, taken) = fake_server(|request| match request.path.as_str() {
        "/apps/start" => (
            200,
            vec![
                ("Set-Cookie", "moqui.session=first; Path=/apps; HttpOnly; SameSite=Lax"),
                ("Set-Cookie", "foreign=no; Domain=example.invalid; Path=/"),
            ],
            "{}".to_string(),
        ),
        "/apps/issue" => (
            302,
            vec![
                ("Set-Cookie", "moqui.session=second; Path=/apps; HttpOnly; SameSite=Lax"),
                ("Location", "/apps/show"),
            ],
            "{}".to_string(),
        ),
        "/apps/show" => (
            200,
            vec![],
            if request.headers.get("cookie").map(String::as_str) == Some("moqui.session=second") {
                r#"{"oneTimeValue":"shown"}"#.to_string()
            } else {
                r#"{"oneTimeValue":null}"#.to_string()
            },
        ),
        _ => (200, vec![], "{}".to_string()),
    });
    let http = Http::new(base, None).unwrap();
    *lock(&http.login) = Some(Stored {
        user: "ada".to_string(),
        key: "K1".to_string(),
    });
    let send = |path: &str, method: &str| {
        http.send(Ask {
            path: path.to_string(),
            method: method.to_string(),
            parameters: BTreeMap::new(),
            json: true,
            tui: false,
            remember: false,
            page: String::new(),
        })
        .unwrap()
    };
    send("/apps/start", "GET");
    assert_eq!(send("/apps/issue", "POST").status, 302);
    assert!(send("/apps/show", "GET")
        .body_text()
        .contains(r#""oneTimeValue":"shown""#));
    send("/outside", "GET");
    let taken = lock(&taken);
    assert_eq!(
        taken[1].headers.get("cookie").map(String::as_str),
        Some("moqui.session=first")
    );
    assert_eq!(
        taken[3].headers.get("cookie"),
        None,
        "path and domain cookies remain scoped"
    );
    assert!(taken
        .iter()
        .all(|request| request.headers.get("login_key").map(String::as_str) == Some("K1")));
}

#[test]
fn a_tui_sign_in_with_no_key_is_refused_and_nothing_is_kept() {
    let (base, _) = fake_server(|_| (200, vec![], r#"{"loggedIn": true}"#.to_string()));
    let http = Http::new(base, None).unwrap();
    let Err(LinkError::Remote(reason)) = http.login("ada", "Secret1!", "") else {
        panic!("not refused");
    };
    assert_eq!(reason, moqui_lib::remote_login::NO_KEY);
    assert_eq!(http.user(), None);
}
