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

//! What the server knows of its protocols, for the delegated view of them (mr-boss plan/design-admin.md §9 ProtocolsHome
//! and ServerConfiguration; impl-admin P12): `org.moqui.admin.ConfigurationServices.get#ProtocolFacts` and
//! `probe#ProtocolTarget`. No Java counterpart.
//!
//! A script can read a conf property and ask whether a feature is built, but not what each listener did at startup
//! (`crate::startup`, which the host logs once), a literal attribute of the conf, or whether a port answers. These two
//! services answer exactly that, each from a fixed list: the startup entries of [`ENTRIES`], the nonsecret conf
//! attributes of [`ATTRIBUTES`], and the probe targets of [`TARGETS`], whose host and port are the deployment's own conf
//! properties. Nothing is taken from the request but a target's key, so neither is a port scanner, and no key, host key
//! or secret is among the answers. `org.moqui.admin.ConfigurationServices` (ConfigurationProtocols.js) reads them as
//! protocol readiness and runs the probes behind its own confirmation.

use std::collections::BTreeMap;
use std::net::{SocketAddr, TcpStream, ToSocketAddrs};
use std::time::{Duration, Instant};

use moqui_lib::error::Error as LibError;
use moqui_lib::object::{Value, ValueMap};

use crate::context::ExecutionContext;
use crate::entity::EntityFacade;
use crate::service::util::param_text;
use crate::startup::{Section, State};

/// A read needs one of these: the integration, configuration or operations administrator's.
const VIEW_PERMISSIONS: [&str; 6] = [
    "ADMIN_CONNECTION_VIEW",
    "ADMIN_CONNECTION_MANAGE",
    "ADMIN_SETTINGS_VIEW",
    "ADMIN_SETTINGS_MANAGE",
    "ADMIN_OPERATION_VIEW",
    "ADMIN_OPERATION_MANAGE",
];
/// A probe sends something, so it needs the integration administrator's change permission.
const PROBE_PERMISSION: &str = "ADMIN_CONNECTION_MANAGE";

/// The startup report's entries answered, by section and name (`moqui_host::startup_report`).
const ENTRIES: [(Section, &str); 17] = [
    (Section::Listener, "HTTP"),
    (Section::Listener, "HTTPS"),
    (Section::Listener, "SSH"),
    (Section::Listener, "SMTP"),
    (Section::Listener, "Call media"),
    (Section::Listener, "TURN"),
    (Section::Listener, "SIP"),
    (Section::Listener, "XMPP"),
    (Section::Listener, "Edge tunnels"),
    (Section::Integration, "Edge link"),
    (Section::Integration, "Call recording"),
    (Section::Integration, "Call captions"),
    (Section::Integration, "SSH client"),
    (Section::Integration, "SSO"),
    (Section::Integration, "IPP printing"),
    (Section::Integration, "Web push"),
    (Section::Integration, "Mobile push (FCM)"),
];

/// The conf attributes answered, element and attribute: what decides whether a protocol runs and whom it trusts. No
/// key, host key or key file is among them.
const ATTRIBUTES: [(&str, &str); 17] = [
    ("smtp-server", "listen-port"),
    ("smtp-server", "listen-host"),
    ("smtp-server", "allowed-addresses"),
    ("smtp-server", "authserv-id"),
    ("smtp-server", "internet"),
    ("smtp-server", "proxy-protocol"),
    ("smtp-server", "protocol"),
    ("smtp-server", "starttls"),
    ("smtp-server", "require-tls"),
    ("smtp-server", "hostname"),
    ("ssh-server", "enabled"),
    ("ssh-server", "port"),
    ("ssh-server", "host"),
    ("ssh-server", "public-host"),
    ("ssh-server", "public-port"),
    ("git-server", "public-url"),
    ("git-server", "allow-http"),
];

/// A probe target: its key, whether it leaves the server (a public name, looked up and connected to from here), the
/// conf properties its host and its port are read from (the first with a value), and the port when none has one.
struct Target {
    key: &'static str,
    external: bool,
    hosts: &'static [&'static str],
    ports: &'static [&'static str],
    default_port: &'static str,
}

/// The targets a probe may reach: each listener's own bound address, from this server, and the public names the
/// deployment gives its clients. TCP only: the UDP of call media, TURN and SIP is not probed.
const TARGETS: [Target; 9] = [
    Target {
        key: "smtp.listener",
        external: false,
        hosts: &["smtp-server@listen-host"],
        ports: &["smtp-server@listen-port"],
        default_port: "",
    },
    Target {
        key: "ssh.listener",
        external: false,
        hosts: &["ssh-server@host"],
        ports: &["ssh-server@port"],
        default_port: "",
    },
    Target {
        key: "xmpp.listener",
        external: false,
        hosts: &["xmpp-server@host"],
        ports: &["xmpp-server@port"],
        default_port: "",
    },
    Target {
        key: "sip.listener",
        external: false,
        hosts: &["sip-server@host"],
        ports: &["sip-server@port"],
        default_port: "",
    },
    Target {
        key: "turn.listener",
        external: false,
        hosts: &["media-server/turn-server@host", "media-server@host"],
        ports: &["media-server/turn-server@port"],
        default_port: "",
    },
    Target {
        key: "web.public",
        external: true,
        hosts: &["webapp-list/webapp[webroot]@http-host"],
        ports: &[],
        default_port: "",
    },
    Target {
        key: "ssh.public",
        external: true,
        hosts: &["ssh-server@public-host", "webapp-list/webapp[webroot]@http-host"],
        ports: &["ssh-server@public-port", "ssh-server@port"],
        default_port: "",
    },
    Target {
        key: "turn.public",
        external: true,
        hosts: &["media-server/turn-server@public-host"],
        ports: &["media-server/turn-server@port"],
        default_port: "",
    },
    Target {
        key: "xmpp.public",
        external: true,
        hosts: &["xmpp-server@domain"],
        ports: &["xmpp-server@port"],
        default_port: "",
    },
];

/// How long one connection may take, and how many addresses of a name are tried.
const CONNECT_LIMIT: Duration = Duration::from_secs(5);
const ADDRESS_LIMIT: usize = 3;

pub(in crate::service) fn register(
    map: &mut BTreeMap<&'static str, crate::service::rust_services::RustServiceFn>,
) -> Result<(), String> {
    rust_service!(map, get_protocol_facts)?;
    rust_service!(map, probe_protocol_target)?;
    rust_service!(map, get_own_host_names)?;
    Ok(())
}

/// `get#OwnHostNames`: the names this instance answers by ([`own_host_names`]), for a link it sends in mail.
pub(in crate::service) fn get_own_host_names(
    _parameters: &BTreeMap<String, Value>,
    facade: &EntityFacade,
) -> Result<BTreeMap<String, Value>, LibError> {
    let Some(factory) = ExecutionContext::with_current(|ctx| ctx.factory().clone()) else {
        return Err(LibError::new("no execution context is entered"));
    };
    let names = own_host_names(&factory, facade, crate::web::is_production());
    Ok(BTreeMap::from([(
        "hostNameList".to_string(),
        Value::List(names.into_iter().map(Value::String).collect()),
    )]))
}

/// A host name as a URL's authority gives it, for comparing: lower case, no trailing dot, no port, an IPv6 address in
/// brackets.
pub(crate) fn normal_host(host: &str) -> String {
    let host = host.trim().to_ascii_lowercase();
    if let Some(rest) = host.strip_prefix('[') {
        return format!("[{}]", rest.split(']').next().unwrap_or(""));
    }
    if host.parse::<std::net::Ipv6Addr>().is_ok() {
        return format!("[{host}]");
    }
    let bare = match host.rsplit_once(':') {
        Some((name, port)) if !port.is_empty() && port.bytes().all(|byte| byte.is_ascii_digit()) => name,
        _ => host.as_str(),
    };
    bare.trim_end_matches('.').to_string()
}

/// The names this instance answers by, for a link it sends (a relay's link base URL, plans/archive/Security-Review-Medium.md
/// SEC-ADM-83): the webroot webapp's `http-host`, `https-host` and `allowed-hosts`, the public sites' hosts and the names
/// set up with ACME (System › Certificates); and on an instance that is not production this machine's own names too:
/// `localhost`, the loopback addresses, its host name and the addresses that name has. Sorted, each once, as
/// [`normal_host`] gives it.
pub(crate) fn own_host_names(
    factory: &crate::context::ExecutionContextFactory,
    facade: &EntityFacade,
    production: bool,
) -> Vec<String> {
    let mut names = std::collections::BTreeSet::new();
    let setting = |name: &str| crate::context::conf_env::setting(&format!("webapp-list/webapp[webroot]@{name}"));
    let mut add = |host: &str| {
        let host = normal_host(host);
        if !host.is_empty() && host != "[]" {
            names.insert(host);
        }
    };
    add(&setting("http-host"));
    add(&setting("https-host"));
    for host in setting("allowed-hosts").split(',') {
        add(host);
    }
    for host in crate::screen::public_site_hosts(factory) {
        add(&host);
    }
    let acme = {
        let _authz = ExecutionContext::with_current(ExecutionContext::disable_authz);
        facade
            .find("moqui.server.AcmeName")
            .and_then(|find| find.select_field("hostName").list())
    };
    match acme {
        Ok(rows) => {
            for row in rows.iter() {
                add(&row
                    .get("hostName")
                    .map(moqui_lib::object::to_plain_string)
                    .unwrap_or_default());
            }
        }
        Err(err) => tracing::error!("The names set up with ACME could not be read: {err}"),
    }
    if !production {
        for host in ["localhost", "127.0.0.1", "::1"] {
            add(host);
        }
        let (address, name) = crate::context::identity::server_address();
        add(name);
        add(address);
        if let Ok(found) = (name.as_str(), 0).to_socket_addrs() {
            for address in found {
                add(&address.ip().to_string());
            }
        }
    }
    names.into_iter().collect()
}

fn require_any(permissions: &[&str]) -> Result<(), LibError> {
    if permissions
        .iter()
        .any(|permission| crate::user::has_permission(permission).unwrap_or(false))
    {
        Ok(())
    } else {
        Err(LibError::new(format!("{} is required", permissions[0])))
    }
}

/// The state of a startup entry as the services answer it, with its detail: where a listener is bound, why one is off,
/// why one did not start.
fn state_parts(state: &State) -> (&'static str, &str) {
    match state {
        State::On(detail) => ("on", detail),
        State::Off(reason) => ("off", reason),
        State::Failed(reason) => ("failed", reason),
        State::Skipped(reason) => ("skipped", reason),
        State::NotBuilt => ("notBuilt", ""),
    }
}

fn section_name(section: Section) -> &'static str {
    match section {
        Section::Listener => "listener",
        Section::Integration => "integration",
    }
}

/// A line of text for a person: one line, at most 255 characters.
fn safe_line(text: &str) -> String {
    text.chars()
        .map(|character| if character.is_control() { ' ' } else { character })
        .take(255)
        .collect()
}

/// `get#ProtocolFacts`: `packageName` and `featureList` (the build), `entryList` (each of [`ENTRIES`] the startup
/// report holds: section, name, state on, off, failed, skipped or notBuilt, detail), `serverStarted` (whether this
/// process opened its listeners: a command-line run opens none, and then no entry says a listener is off or failed)
/// and `confMap` (each of [`ATTRIBUTES`] by `element/@attribute`, as the conf has it with its properties expanded). An
/// error without one of [`VIEW_PERMISSIONS`].
fn get_protocol_facts(
    _parameters: &BTreeMap<String, Value>,
    _facade: &EntityFacade,
) -> Result<BTreeMap<String, Value>, LibError> {
    require_any(&VIEW_PERMISSIONS)?;
    let (entries, conf) = ExecutionContext::with_current(|ctx| {
        let factory = ctx.factory();
        let entries = crate::startup::report(factory).entries();
        let conf: ValueMap = ATTRIBUTES
            .iter()
            .map(|(element, name)| {
                (
                    format!("{element}/@{name}"),
                    Value::string(factory.conf_setting(element, name)),
                )
            })
            .collect();
        (entries, conf)
    })
    .unwrap_or_default();
    let started = entries
        .iter()
        .any(|entry| entry.section == Section::Listener && entry.name == "HTTP");
    let entry_list: Vec<Value> = entries
        .iter()
        .filter(|entry| ENTRIES.contains(&(entry.section, entry.name.as_str())))
        .map(|entry| {
            let (state, detail) = state_parts(&entry.state);
            let mut row = ValueMap::default();
            row.insert("section".to_string(), Value::string(section_name(entry.section)));
            row.insert("name".to_string(), Value::string(&entry.name));
            row.insert("state".to_string(), Value::string(state));
            row.insert("detail".to_string(), Value::string(safe_line(detail)));
            Value::Map(row)
        })
        .collect();
    let mut out = BTreeMap::new();
    out.insert("packageName".to_string(), Value::string(crate::build_info::package()));
    out.insert(
        "featureList".to_string(),
        Value::List(crate::build_info::features().into_iter().map(Value::string).collect()),
    );
    out.insert("entryList".to_string(), Value::List(entry_list));
    out.insert("serverStarted".to_string(), Value::Bool(started));
    out.insert("confMap".to_string(), Value::Map(conf));
    Ok(out)
}

fn property(name: &str) -> String {
    crate::context::conf_env::setting_or_property(name).trim().to_string()
}

fn first_property(names: &[&str]) -> String {
    names
        .iter()
        .map(|name| property(name))
        .find(|value| !value.is_empty())
        .unwrap_or_default()
}

/// The host and port of `target` as the conf has them, or what is missing. A listener bound to every address is
/// reached on this machine; the web address's port is the one links are made with.
fn target_address(target: &Target) -> Result<(String, u16), &'static str> {
    let mut host = first_property(target.hosts);
    let port_text = if target.key == "web.public" {
        let https = property("webapp-list/webapp[webroot]@https-enabled") == "true";
        match first_property(if https {
            &["webapp-list/webapp[webroot]@https-port"]
        } else {
            &["webapp-list/webapp[webroot]@http-port"]
        }) {
            port if port.is_empty() => if https { "443" } else { "80" }.to_string(),
            port => port,
        }
    } else {
        match first_property(target.ports) {
            port if port.is_empty() => target.default_port.to_string(),
            port => port,
        }
    };
    if !target.external && matches!(host.as_str(), "" | "0.0.0.0" | "::" | "[::]") {
        host = "127.0.0.1".to_string();
    }
    // a name or an address alone: a value that carries a port or a path of its own is not probed
    let plain = !host.is_empty()
        && host
            .chars()
            .all(|character| character.is_ascii_alphanumeric() || ".-:[]".contains(character))
        && (host.matches(':').count() != 1);
    if !plain {
        return Err("notConfigured");
    }
    match port_text.parse::<u16>() {
        Ok(port) if port > 0 => Ok((host.trim_matches(['[', ']']).to_string(), port)),
        _ => Err("notConfigured"),
    }
}

/// Why a connection was not made, as a kind: never the system's own text.
fn failure_kind(error: &std::io::Error) -> &'static str {
    match error.kind() {
        std::io::ErrorKind::ConnectionRefused => "refused",
        std::io::ErrorKind::TimedOut | std::io::ErrorKind::WouldBlock => "timedOut",
        _ => "unreachable",
    }
}

/// Connect to the first of `addresses` that answers within [`CONNECT_LIMIT`]; the connection is closed at once and
/// nothing is written to it.
fn connect(addresses: &[SocketAddr]) -> Result<(), &'static str> {
    let mut failure = "unreachable";
    for address in addresses.iter().take(ADDRESS_LIMIT) {
        match TcpStream::connect_timeout(address, CONNECT_LIMIT) {
            Ok(_) => return Ok(()),
            Err(error) => failure = failure_kind(&error),
        }
    }
    Err(failure)
}

/// `probe#ProtocolTarget`: one TCP connection to a target of [`TARGETS`] by its `targetKey`, closed as soon as it is
/// made: `target` (the host and port tried), `external`, `reached`, `failureKind` (notConfigured: the conf names no
/// host or port; notFound: the name has no address; refused, timedOut or unreachable), `internalAddress` (a public
/// name that answers with an address inside the network) and `millis`. A target that leaves the server is tried only
/// with `confirmed` true. An error without `ADMIN_CONNECTION_MANAGE`, for an unknown key and for an external target
/// that is not confirmed.
fn probe_protocol_target(
    parameters: &BTreeMap<String, Value>,
    _facade: &EntityFacade,
) -> Result<BTreeMap<String, Value>, LibError> {
    require_any(&[PROBE_PERMISSION])?;
    let key = param_text(parameters, "targetKey");
    let Some(target) = TARGETS.iter().find(|target| target.key == key) else {
        return Err(LibError::new(format!("{key} is not a protocol probe target")));
    };
    if target.external && param_text(parameters, "confirmed") != "true" {
        return Err(LibError::new(
            "A probe of a public address is sent only when it is confirmed",
        ));
    }
    let mut out = BTreeMap::new();
    out.insert("targetKey".to_string(), Value::string(target.key));
    out.insert("external".to_string(), Value::Bool(target.external));
    let started = Instant::now();
    let mut internal = false;
    let result = match target_address(target) {
        Err(kind) => Err(kind),
        Ok((host, port)) => {
            out.insert("target".to_string(), Value::string(format!("{host}:{port}")));
            match (host.as_str(), port).to_socket_addrs() {
                Err(_) => Err("notFound"),
                Ok(found) => {
                    let addresses: Vec<SocketAddr> = found.collect();
                    internal = target.external
                        && addresses
                            .iter()
                            .any(|address| moqui_lib::net::internal_address(address.ip()));
                    if addresses.is_empty() {
                        Err("notFound")
                    } else {
                        connect(&addresses)
                    }
                }
            }
        }
    };
    out.insert("reached".to_string(), Value::Bool(result.is_ok()));
    out.insert(
        "failureKind".to_string(),
        result.err().map_or(Value::Null, Value::string),
    );
    out.insert("internalAddress".to_string(), Value::Bool(internal));
    out.insert(
        "millis".to_string(),
        Value::I64(i64::try_from(started.elapsed().as_millis()).unwrap_or(i64::MAX)),
    );
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::{failure_kind, normal_host, own_host_names, state_parts, ATTRIBUTES, ENTRIES, TARGETS};
    use crate::startup::State;

    /// The names a relay's link base URL may name (plans/archive/Security-Review-Medium.md SEC-ADM-83): the names set up with ACME
    /// on every instance, and on one that is not production this machine's own (localhost, its host name and addresses);
    /// a name compares in any case, with no port or trailing dot.
    #[test]
    fn the_own_names_are_the_instances_and_on_dev_the_machines() {
        assert_eq!(normal_host(" Shop.Example.COM.:8443 "), "shop.example.com");
        assert_eq!(normal_host("[::1]:8080"), "[::1]");
        assert_eq!(normal_host("::1"), "[::1]");
        let facade = crate::testing::open();
        facade.set_disable_fk_create(true);
        let ctx = crate::context::ExecutionContextFactory::test_context();
        ctx.factory().set_entity(facade.clone());
        let _entered = ctx.enter();
        {
            let _authz = ctx.disable_authz();
            let mut name = facade.make_value("moqui.server.AcmeName").unwrap();
            name.set("hostName", moqui_lib::object::Value::string("shop.example.com"));
            name.create(&facade).unwrap();
        }
        let has = |names: &[String], name: &str| names.iter().any(|found| found == name);
        let dev = own_host_names(ctx.factory(), &facade, false);
        let (address, machine) = crate::context::identity::server_address();
        assert!(
            has(&dev, "shop.example.com") && has(&dev, "localhost") && has(&dev, "[::1]"),
            "{dev:?}"
        );
        assert!(
            has(&dev, &normal_host(machine)) && has(&dev, &normal_host(address)),
            "{dev:?}"
        );
        let production = own_host_names(ctx.factory(), &facade, true);
        assert!(has(&production, "shop.example.com"), "{production:?}");
        assert!(
            !has(&production, "localhost") && !has(&production, &normal_host(machine)),
            "{production:?}"
        );
    }

    #[test]
    fn the_lists_name_no_secret_and_each_key_once() {
        let targets: std::collections::BTreeSet<&str> = TARGETS.iter().map(|target| target.key).collect();
        assert_eq!(targets.len(), TARGETS.len());
        let attributes: std::collections::BTreeSet<_> = ATTRIBUTES.iter().collect();
        assert_eq!(attributes.len(), ATTRIBUTES.len());
        let entries: std::collections::BTreeSet<_> = ENTRIES.iter().map(|(_, name)| name).collect();
        assert_eq!(entries.len(), ENTRIES.len());
        for (element, name) in ATTRIBUTES {
            assert!(
                !["key", "secret", "pass", "token"]
                    .iter()
                    .any(|word| name.contains(word)),
                "{element} {name}"
            );
        }
        for target in &TARGETS {
            for name in target.hosts.iter().chain(target.ports) {
                assert!(
                    !["key", "secret", "pass", "token"]
                        .iter()
                        .any(|word| name.contains(word)),
                    "{} {name}",
                    target.key
                );
            }
            assert_eq!(target.external, target.key.ends_with(".public"), "{}", target.key);
        }
    }

    #[test]
    fn a_listener_that_is_off_failed_or_not_built_reads_apart() {
        assert_eq!(state_parts(&State::On("tcp 127.0.0.1:2525".into())).0, "on");
        assert_eq!(
            state_parts(&State::off("smtp-server@listen-port is not set")),
            ("off", "smtp-server@listen-port is not set")
        );
        assert_eq!(state_parts(&State::failed("bind")).0, "failed");
        assert_eq!(state_parts(&State::Skipped("busy".into())).0, "skipped");
        assert_eq!(state_parts(&State::NotBuilt), ("notBuilt", ""));
        assert_eq!(
            failure_kind(&std::io::Error::from(std::io::ErrorKind::ConnectionRefused)),
            "refused"
        );
    }
}
