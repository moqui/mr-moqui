//! Shared host identity and credential-name classification.
use std::sync::OnceLock;
/// Whole names of secrets: `returnkey` reads a PASS payment attempt's outcome (`getPaymentResult`, SEC-PSA-09); `pin`
/// is a voicemail box's (`setPin`, SEC-COM-95), `ha1` a SIP digest's verifier; `ticket` a terminal's recent sign-in in
/// the `/termws` query (SEC-FIL-64).
const SECRET_PARAMETER_NAMES: &[&str] = &[
    "code",
    "moquisessiontoken",
    "sessiontoken",
    "sql",
    "sharedsecret",
    "returnkey",
    "pin",
    "ha1",
    "ticket",
];

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
pub fn secret_parameter(key: &str) -> bool {
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

/// Whether `System.getProperty` refuses the property `name` to a script (plans/archive/Security-Review-Medium.md SEC-SCR-03): a
/// conf attribute's environment name (`moqui_conf__...`, in any case), a name [`secret_parameter`] calls a secret's
/// (password, secret, token, credential, a `_key` ending), and a name that ends in `_pass` or `_pw` or holds `crypt`;
/// each in lower case with `.` read as `_`, as the lookup reads a name both ways. A script reads the conf by selector
/// (`ec.factory.getConfValue`), which never gives it a secret; one that needs a secret uses a named host function
/// (`ec.factory.isConfValueSet`, `System.propertyMatches`, `System.secretCodeHash`), and one that sends a credential at
/// another server (mr-pass's `pass@client-secret`) names it to `RestClient.addPropertyHeader`, which never gives it to
/// the script.
pub(crate) fn script_refused_property(name: &str) -> bool {
    let lower = name.trim().to_ascii_lowercase();
    let under = lower.replace('.', "_");
    // a conf attribute's environment name (plans/archive/Conf-Paths.md §4)
    moqui_lib::node::path::env_segments(name.trim()).is_some()
        || secret_parameter(&lower)
        || secret_parameter(&under)
        || under.ends_with("_pass")
        || under.ends_with("_pw")
        || under.contains("crypt")
        // a secret the seal's store keeps, by its own name (plans/Sealed-Secrets.md S4)
        || crate::seal::secrets::is_secret_name(name)
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

    /// SEC-SCR-03: the names `System.getProperty` refuses to a script, and the setting names scripts read.
    #[test]
    fn scripts_read_no_secret_property() {
        for name in [
            // a conf attribute's environment name, whatever it holds: scripts read the conf by selector
            "moqui_conf__entity_facade__crypt_pass",
            "MOQUI_CONF__WEBAPP_LIST__WEBAPP__WEBROOT__HTTP_PORT",
            "entity_ds_crypt_pass",
            "entity_ds_crypt_pass_old",
            "entity_ds_password",
            "session_cookie_key",
            "ENTITY_DS_PASSWORD",
            "entity.ds.crypt.pass",
            "xmpp_upload_secret",
            "git_lfs_token_key_previous",
            "AWS_SECRET_ACCESS_KEY",
            "GITHUB_TOKEN",
            "PGPASSWORD",
            "smtp_pw",
            // a credential at another server, by its name: sent by RestClient.addPropertyHeader, never read
            "remote_client_secret",
            "REMOTE_CLIENT_SECRET",
        ] {
            assert!(super::script_refused_property(name), "{name}");
        }
        for name in [
            "remote_backend_url",
            "remote_client_id",
            "remote_client_web_sites",
            "instance_purpose",
            "webapp_http_host",
            "mail_change_keep_days",
            "client_request_keep_days",
            "xmpp_turn_per_hour",
            "embedded_db_backup_dir",
            "default_time_zone",
        ] {
            assert!(!super::script_refused_property(name), "{name}");
        }
    }
}
