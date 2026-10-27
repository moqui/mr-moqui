//! Shared host identity and credential-name classification.
use std::sync::OnceLock;
const SECRET_PARAMETER_NAMES: &[&str] = &["code", "moquisessiontoken", "sessiontoken", "sql", "sharedsecret"];

/// Parts that make any name a secret's, wherever they are in it.
const SECRET_NAME_PARTS: &[&str] = &[
    "password",
    "secret",
    "token",
    "authorization",
    "cookie",
    "credential",
    "arguments",
    "locals",
    "api.key",
    "api-key",
    "apikey",
    "private_key",
];

/// Whether a parameter, URL query, header or telemetry attribute name names a credential or other secret, without
/// case: the one rule for what request details, the HTTP log's query and telemetry attributes leave out. A count of
/// tokens (`gen_ai.usage.input_tokens`) is not a secret, nor is a name that only holds "key" (`primaryKeyId`).
pub(crate) fn secret_parameter(key: &str) -> bool {
    let lowered;
    let lower = if key.bytes().any(|byte| byte.is_ascii_uppercase()) {
        lowered = key.to_ascii_lowercase();
        lowered.as_str()
    } else {
        key
    };
    if lower.ends_with(".tokens") || lower.ends_with("_tokens") || lower.ends_with("token_count") {
        return false;
    }
    crate::web::CREDENTIAL_PARAMETER_NAMES.contains(&lower)
        || SECRET_PARAMETER_NAMES.contains(&lower)
        || SECRET_NAME_PARTS.iter().any(|part| lower.contains(part))
        || ["_key", "-key", ".key", "loginkey"]
            .iter()
            .any(|end| lower.ends_with(end))
}

pub(crate) fn server_address() -> &'static (String, String) {
    static ADDRESS: OnceLock<(String, String)> = OnceLock::new();
    ADDRESS.get_or_init(|| {
        let name = std::fs::read_to_string("/proc/sys/kernel/hostname")
            .ok()
            .or_else(|| std::env::var("HOSTNAME").ok())
            .map(|text| text.trim().to_string())
            .filter(|text| !text.is_empty());
        let Some(name) = name else {
            return ("127.0.0.1".to_string(), "localhost".to_string());
        };
        // Java orders the resolved addresses IPv4 first (java.net.preferIPv6Addresses defaults to false), keeping
        // the resolver's order within each family, and takes the first.
        let addresses: Vec<std::net::IpAddr> = std::net::ToSocketAddrs::to_socket_addrs(&(name.as_str(), 0))
            .map(|found| found.map(|address| address.ip()).collect())
            .unwrap_or_default();
        let ip = addresses
            .iter()
            .find(|address| address.is_ipv4())
            .or_else(|| addresses.first())
            .map_or_else(|| "127.0.0.1".to_string(), ToString::to_string);
        (ip, name)
    })
}

#[cfg(test)]
mod tests {
    #[test]
    fn credential_names_remain_redacted_after_hit_storage_retirement() {
        for name in [
            "PASSWORD",
            "authToken",
            "sharedSecret",
            "sql",
            "code",
            "apiKey",
            "private_key",
            "x-api-key",
            "access_token",
            "http.request.header.authorization",
            "http.request.header.cookie",
            "gen_ai.tool.call.arguments",
        ] {
            assert!(super::secret_parameter(name), "{name}");
        }
        for name in [
            "orderId",
            "primaryKeyId",
            "monkeyId",
            "gen_ai.usage.input_tokens",
            "moqui.artifact.name",
        ] {
            assert!(!super::secret_parameter(name), "{name}");
        }
    }
}
