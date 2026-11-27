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

use super::{allowed_url, header_allowed, host_allowed, host_entry, host_list, method, request, HOSTS_MAX};

fn list(hosts: &[&str]) -> Vec<String> {
    host_list(&hosts.iter().map(|host| host.to_string()).collect::<Vec<_>>()).unwrap()
}

#[test]
fn a_host_entry_is_a_dns_name_or_a_wildcard_of_one() {
    assert_eq!(host_entry("API.Example.com").unwrap(), "api.example.com");
    assert_eq!(host_entry("*.example.com").unwrap(), "*.example.com");
    for bad in ["", "*", "*.", "com", "localhost", "app.localhost", "10.0.0.1", "127.0.0.1", "exa mple.com",
        "-a.example.com", "a..example.com", "*.*.example.com", "example.com/path", "user@example.com"]
    {
        assert!(host_entry(bad).is_err(), "{bad} was taken");
    }
}

#[test]
fn the_list_has_one_to_the_most_hosts() {
    assert!(host_list(&[]).is_err());
    let many: Vec<String> = (0..=HOSTS_MAX).map(|n| format!("h{n}.example.com")).collect();
    assert!(host_list(&many).is_err());
    assert!(host_list(&many[..HOSTS_MAX]).is_ok());
}

#[test]
fn a_wildcard_takes_the_subdomains_only() {
    let allowed = list(&["api.example.com", "*.cdn.example.org"]);
    assert!(host_allowed("api.example.com", &allowed));
    assert!(!host_allowed("www.example.com", &allowed));
    assert!(!host_allowed("evilapi.example.com", &allowed));
    assert!(host_allowed("a.cdn.example.org", &allowed));
    assert!(host_allowed("a.b.cdn.example.org", &allowed));
    assert!(!host_allowed("cdn.example.org", &allowed));
    assert!(!host_allowed("evilcdn.example.org", &allowed));
    assert!(!host_allowed("cdn.example.org.evil.com", &allowed));
}

#[test]
fn only_https_urls_on_the_list_go_out() {
    let allowed = list(&["api.example.com"]);
    assert!(allowed_url("https://api.example.com/v1?q=1", &allowed).is_ok());
    assert!(allowed_url("https://API.example.com/", &allowed).is_ok());
    assert!(allowed_url("https://api.example.com:443/", &allowed).is_ok());
    for bad in ["http://api.example.com/", "https://api.example.com:8443/", "https://user:pw@api.example.com/",
        "https://other.example.com/", "https://93.184.216.34/", "https://[::1]/", "file:///etc/passwd",
        "https://api.example.com.evil.com/", "not a url"]
    {
        assert!(allowed_url(bad, &allowed).is_err(), "{bad} was taken");
    }
}

#[test]
fn the_page_sets_no_cookie_or_framing_header() {
    for name in ["user-agent", "content-type", "x-api-client", "origin", "referer", "authorization"] {
        assert!(header_allowed(name), "{name}");
    }
    for name in ["cookie", "host", "content-length", "transfer-encoding", "connection", "proxy-authorization",
        "sec-fetch-site"]
    {
        assert!(!header_allowed(name), "{name}");
    }
}

#[test]
fn get_head_and_post_only() {
    assert!(method("get").is_ok());
    assert!(method("").is_ok());
    assert!(method("POST").is_ok());
    assert!(method("HEAD").is_ok());
    assert!(method("PUT").is_err());
    assert!(method("DELETE").is_err());
}

#[test]
fn a_request_off_the_list_is_refused_before_it_goes() {
    let hosts = vec!["api.example.com".to_string()];
    let err = request("GET", "https://other.example.com/", &[], None, &hosts).unwrap_err();
    assert!(err.contains("not on the list"), "{err}");
    let err = request("GET", "https://api.example.com/", &[], None, &[]).unwrap_err();
    assert!(err.contains("hosts"), "{err}");
}
