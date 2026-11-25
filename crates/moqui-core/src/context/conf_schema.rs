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

//! The schema of the merged conf (plans/archive/Conf-Paths.md §2): a tree of units by element name, each with how its
//! elements are told apart (its key), its class, the attributes whose class differs, and its child units. The merge
//! keys repeated elements by it, an environment name or selector finds an element by it ([`keying`]), and the conf
//! reload (plans/archive/Component-Deploy.md, Phase 1) reads what each part takes to change on a running server: read
//! on each use (live), built from the conf by a hook (apply), or read once at boot (restart). What no unit names is
//! part of its parent unit; at the top, an element no unit names needs a restart, so an element added to the conf
//! without a catalog entry is never reported as live.

pub use moqui_lib::node::path::Keying as Key;

/// How a change to a unit of the conf takes effect.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Class {
    /// Read on each use: the change is in effect once the conf is replaced.
    Live,
    /// Built from the conf: the group's hook builds it again after the conf is replaced.
    Apply(Group),
    /// Read once at boot: the change waits for a restart. The reason says what reads it.
    Restart(&'static str),
}

impl Class {
    /// The class as a word: `live`, `apply` or `restart`.
    #[must_use]
    pub fn name(self) -> &'static str {
        match self {
            Self::Live => "live",
            Self::Apply(_) => "apply",
            Self::Restart(_) => "restart",
        }
    }
}

/// The state a hook builds again after a conf reload.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Group {
    /// The screen facade's render modes, mounts and settings, and the definitions built with them.
    Screens,
    /// State the factory builds from the conf on first use ([`crate::context::ExecutionContextFactory::conf_derived`]):
    /// the root screens, the token routes, the template renderers, and the webapp's servlets, filters, listeners and
    /// actions. Built again on first use after the reload.
    Derived,
    /// The `cache` entries: a new one is made; a changed one waits for a restart.
    Caches,
    /// The webapp's `web-protection` rules, which the host's guard takes in place, keeping its counters.
    WebProtection,
}

impl Group {
    /// The group as a name, for the report.
    #[must_use]
    pub fn name(self) -> &'static str {
        match self {
            Self::Screens => "screens",
            Self::Derived => "derived",
            Self::Caches => "caches",
            Self::WebProtection => "web-protection",
        }
    }
}

/// A secret attribute: kept as given, or generated when the conf leaves it at `CHANGEME` (plans/archive/Conf-Paths.md §2).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Secret {
    /// Never logged or shown, and null to scripts.
    Kept,
    /// As [`Secret::Kept`], and a value ending in `CHANGEME` at boot is replaced by 32 random bytes kept in
    /// `runtime/.env` under the attribute's environment name.
    Generated,
}

/// A unit of the conf: an element name, how its elements are told apart, its class, the attributes whose class
/// differs from it, and its child units.
#[derive(Clone, Copy, Debug)]
pub struct Unit {
    pub name: &'static str,
    pub key: Key,
    pub class: Class,
    pub attrs: &'static [(&'static str, Class)],
    /// The attributes that hold a secret.
    pub secrets: &'static [(&'static str, Secret)],
    pub children: &'static [Unit],
}

impl Unit {
    const fn new(name: &'static str, key: Key, class: Class) -> Self {
        Self {
            name,
            key,
            class,
            attrs: &[],
            secrets: &[],
            children: &[],
        }
    }

    const fn attrs(mut self, attrs: &'static [(&'static str, Class)]) -> Self {
        self.attrs = attrs;
        self
    }

    const fn secrets(mut self, secrets: &'static [(&'static str, Secret)]) -> Self {
        self.secrets = secrets;
        self
    }

    const fn children(mut self, children: &'static [Unit]) -> Self {
        self.children = children;
        self
    }

    /// The class of a change to `attr` of this unit's element.
    #[must_use]
    pub fn attr_class(&self, attr: &str) -> Class {
        self.attrs
            .iter()
            .find(|(name, _)| *name == attr)
            .map_or(self.class, |(_, class)| *class)
    }
}

const fn single(name: &'static str, class: Class) -> Unit {
    Unit::new(name, Key::Single, class)
}

const fn keyed(name: &'static str, key: &'static [&'static str], class: Class) -> Unit {
    Unit::new(name, Key::By(key), class)
}

const fn sequence(name: &'static str, class: Class) -> Unit {
    Unit::new(name, Key::Sequence, class)
}

const BOOT: Class = Class::Restart("read once at boot");
const LISTENER: Class = Class::Restart("a listener, worker pool or background loop reads it when it starts");
const NEXT_START: Class = Class::Restart("runs at the next start");
const SCREENS: Class = Class::Apply(Group::Screens);
const DERIVED: Class = Class::Apply(Group::Derived);
const TELEMETRY: Class =
    Class::Restart("read once at boot; Monitor › Settings changes its levels and rates for a while");
const WEB_PROTECTION: Class = Class::Apply(Group::WebProtection);
const LLM: Class = Class::Restart("the agent runtime is built at boot");
const DATABASES: Class = Class::Restart("the database dialects are read when the entity facade is opened");
const EDGE: Class = Class::Restart("the edge link starts at boot");

/// The class of an element at the top of the conf that no unit names.
pub const UNKNOWN: Class = Class::Restart("not known to the conf reload");

/// A top-level element no unit names, a component's own settings (plans/archive/Conf-Paths.md §6.27): live, as its readers
/// read the conf on each use (`ec.factory.getConfValue`).
pub const COMPONENT: Class = Class::Live;

/// The units of the conf, by the top-level element. The classes follow what reads each setting; a setting is live
/// only where its reader reads the conf on each use.
pub static CATALOG: &[Unit] = &[
    single(
        "tools",
        Class::Restart("the tool factories and the worker and request pools start at boot"),
    )
    // read on each use: the audit's days at its purge, the external databases' address rule at each pool
    .attrs(&[
        ("database-audit-days", Class::Live),
        ("external-database-private-network", Class::Live),
    ])
    .children(&[keyed(
        "tool-factory",
        &["class"],
        Class::Restart("tool factories start at boot"),
    )]),
    single("cache-list", BOOT).children(&[keyed("cache", &["name"], Class::Apply(Group::Caches))]),
    single(
        "telemetry",
        Class::Restart("read once at boot; Monitor › Settings changes its levels and rates for a while"),
    )
    .children(&[
        keyed("artifact", &["type"], TELEMETRY),
        keyed("metric", &["name"], TELEMETRY),
        single("operations", TELEMETRY).children(&[keyed("operation", &["service"], TELEMETRY)]),
        // read on each post (moqui-report browser.rs)
        single("browser", Class::Live),
    ]),
    // only read at boot for deprecation warnings
    single("server-stats", Class::Live).children(&[keyed("artifact-stats", &["type", "sub-type"], Class::Live)]),
    single("webapp-list", BOOT).children(&[keyed("webapp", &["name"], Class::Live)
        .attrs(&[
            ("http-port", LISTENER),
            ("http-listen-port", LISTENER),
            ("grpc-listen-port", LISTENER),
            ("grpc-listen-bind", LISTENER),
            ("https-port", LISTENER),
            ("https-enabled", LISTENER),
            ("client-ip-header", BOOT),
            ("trusted-proxies", BOOT),
            ("transfer-workers", LISTENER),
        ])
        .children(&[
            keyed("root-screen", &["host"], DERIVED),
            keyed("error-screen", &["error"], Class::Live),
            keyed("token-route", &["name"], DERIVED),
            keyed("token-path", &["pattern"], DERIVED),
            keyed("public-site", &["host"], Class::Live),
            single("first-hit-in-visit", DERIVED),
            single("after-request", DERIVED),
            single("before-request", DERIVED),
            single("after-login", DERIVED),
            single("before-logout", DERIVED),
            single("after-startup", NEXT_START),
            single("before-shutdown", DERIVED),
            keyed("filter", &["name"], DERIVED).children(&[
                keyed("init-param", &["name"], DERIVED),
                sequence("url-pattern", DERIVED),
                sequence("dispatcher", DERIVED),
            ]),
            keyed("listener", &["class"], DERIVED),
            keyed("servlet", &["name"], DERIVED).children(&[
                keyed("init-param", &["name"], DERIVED),
                sequence("url-pattern", DERIVED),
            ]),
            single("session-config", Class::Live)
                .attrs(&[("flush", BOOT), ("max", BOOT), ("cookie-key", BOOT)])
                .secrets(&[("cookie-key", Secret::Generated)]),
            keyed(
                "endpoint",
                &["path"],
                Class::Restart("the HTTP routes are built when the server starts"),
            ),
            keyed("response-header", &["type", "name"], Class::Live),
            // the listener's limits are read when it starts
            single("web-protection", WEB_PROTECTION).children(&[
                single("connections", LISTENER),
                single("requests", LISTENER),
                single("overload", WEB_PROTECTION),
                keyed("rate-limit", &["name"], WEB_PROTECTION),
                keyed("ban", &["name"], WEB_PROTECTION),
                keyed("client-rule", &["name"], WEB_PROTECTION),
                keyed("path-rule", &["name"], WEB_PROTECTION),
                keyed("user-agent-rule", &["name"], WEB_PROTECTION),
            ]),
        ])]),
    single("artifact-execution-facade", Class::Live).children(&[keyed("artifact-execution", &["type"], Class::Live)]),
    single("user-facade", Class::Live).children(&[
        single("password", Class::Live),
        single("login-key", Class::Live),
        single("login", Class::Live).secrets(&[("initial-admin-secret", Secret::Kept)]),
        single("authz", Class::Live).children(&[
            keyed("sealed-permission", &["name"], Class::Live),
            keyed("privileged-group", &["name"], Class::Live),
        ]),
    ]),
    single(
        "transaction-facade",
        Class::Restart("the transaction managers are set up at boot"),
    ),
    single("resource-facade", Class::Live).children(&[
        keyed(
            "resource-reference",
            &["scheme"],
            Class::Restart("resource providers are installed at boot"),
        ),
        keyed("template-renderer", &["extension"], DERIVED),
        keyed("script-runner", &["extension"], Class::Live),
        // a drive's share link limits (FileServices download#PublicShare, upload#PublicShare; SEC-BOS-83)
        single("drive-share", Class::Live),
        // each user's rich-text image limits (ResourceServices upload#RichTextImage; SEC-CLI-92)
        single("rich-text-images", Class::Live),
    ]),
    single("screen-facade", SCREENS).children(&[
        keyed("screen-text-output", &["type"], SCREENS),
        keyed("screen-output", &["type"], SCREENS),
        keyed("screen", &["location"], SCREENS).children(&[keyed("subscreens-item", &["name"], SCREENS)]),
        single("pdf", Class::Live),
    ]),
    single(
        "service-facade",
        Class::Restart("the job pools and the scheduler start at boot"),
    )
    .children(&[
        keyed("service-location", &["name"], Class::Live),
        keyed(
            "service-type",
            &["name"],
            Class::Restart("service runners are bound at boot"),
        ),
        keyed("service-file", &["location"], Class::Live),
        keyed("startup-service", &["name"], NEXT_START),
        single("thread-pool", BOOT).children(&[keyed("run-from-pool", &["name"], BOOT)]),
        sequence("jms-service", Class::Live),
    ]),
    single(
        "search-facade",
        Class::Restart("the search backend is attached at boot"),
    )
    // read at each search
    .attrs(&[
        ("query-timeout-seconds", Class::Live),
        ("query-max-clauses", Class::Live),
    ])
    .children(&[
        single("cluster", Class::Restart("the search backend is attached at boot"))
            .secrets(&[("password", Secret::Kept)]),
    ]),
    single("llm-facade", LLM).children(&[
        keyed("endpoint", &["id"], LLM)
            .secrets(&[("api-key", Secret::Kept)])
            .children(&[keyed("header", &["name"], LLM)]),
        keyed("model", &["id"], LLM).children(&[keyed("reasoning", &["level"], LLM)]),
        keyed("bind", &["feature"], LLM),
        keyed("data-entity", &["package", "entity"], LLM),
        keyed("user-facts", &["service"], LLM),
        keyed("constitution-provider", &["service"], LLM),
        keyed("ai-task", &["name"], LLM),
        single("mcp", LLM).secrets(&[("request-state-key", Secret::Kept)]),
    ]),
    single("entity-facade", Class::Restart("the entity facade is opened at boot"))
        .secrets(&[("crypt-pass", Secret::Generated), ("crypt-pass-old", Secret::Kept)])
        .children(&[
            keyed(
                "datasource",
                &["group-name"],
                Class::Restart("datasources connect at boot"),
            )
            .secrets(&[("password", Secret::Kept)]),
            single("server-jndi", BOOT),
            sequence("decrypt-alt", Class::Restart("the entity facade is opened at boot")),
            sequence(
                "load-entity",
                Class::Restart("the entity model reads them when it is opened"),
            ),
            sequence("load-data", Class::Live),
        ]),
    single(
        "database-list",
        Class::Restart("the database dialects are read when the entity facade is opened"),
    )
    .children(&[
        keyed("dictionary-type", &["type"], DATABASES),
        keyed("database", &["name"], DATABASES).children(&[keyed("database-type", &["type"], DATABASES)]),
    ]),
    single("edge", EDGE)
        .secrets(&[("link-key", Secret::Generated)])
        .children(&[
            // the pairings, written by pairing and the wizard and read at once (conf_env::keep_in_env)
            single("node", Class::Live).secrets(&[("key", Secret::Kept)]),
            single("front-node", Class::Live).secrets(&[("key", Secret::Kept)]),
            keyed("delegate", &["service"], EDGE),
            keyed("queue-kind", &["name"], EDGE),
            keyed("replicate", &["set"], EDGE).children(&[keyed("entity", &["name"], EDGE)]),
        ]),
    // read at boot: which components load, the node's role, the server's time zone
    single(
        "instance",
        Class::Restart("the purpose, role and time zone are read at boot"),
    ),
    // the seal's gate runs before the factory is built (plans/Sealed-Secrets.md §3.3); the method seal.json records wins
    single(
        "seal",
        Class::Restart("the seal is opened at start, and its method is fixed at init"),
    ),
    single("acme", Class::Live).secrets(&[("account-key", Secret::Generated)]),
    single("client-sync", Class::Live),
    single("board", Class::Live),
    single("chat", Class::Live)
        .secrets(&[("store-key-secret", Secret::Generated)])
        .children(&[single("stickers", Class::Live)]),
    single("load-runner", Class::Live),
    // Maps (plans/Maps.md §4.2): read on each request, import and viewport query
    single("maps", Class::Live),
    single("media-server", LISTENER)
        .children(&[single("turn-server", LISTENER).secrets(&[("auth-secret", Secret::Generated)])]),
    single("calls", Class::Live).children(&[
        single("queue", Class::Live),
        single("recording", Class::Live),
        single("captions", Class::Live),
    ]),
    single("voicemail", Class::Live),
    // the settings of basic-comms and demo, set in their MoquiConf.xml (plans/archive/Conf-Paths.md §6.27)
    single("comms", Class::Live),
    single("demo", Class::Live),
    single("pass-authority", Class::Live)
        .attrs(&[("grpc-enabled", LISTENER), ("grpc-insecure", LISTENER)])
        .children(&[keyed("client", &["id"], Class::Live).secrets(&[("secret", Secret::Kept)])]),
    single("database-backup", Class::Live),
    single("sip-server", LISTENER).children(&[single("limits", LISTENER), single("registrar", Class::Live)]),
    single("xmpp-server", LISTENER)
        .secrets(&[("scram-decoy-key", Secret::Generated)])
        .children(&[single("upload", Class::Live).secrets(&[("secret", Secret::Generated)])]),
    // the TLS pair is made together at boot (conf_env::generate_secrets), not as two random values
    single("cluster-member", BOOT).secrets(&[("tls-cert", Secret::Kept), ("tls-key", Secret::Kept)]),
    single("push", Class::Live).children(&[
        single("web", Class::Live).secrets(&[("vapid-key", Secret::Generated)]),
        single("fcm", Class::Live).secrets(&[("service-account", Secret::Kept)]),
    ]),
    single("channels", Class::Live)
        .attrs(&[("webhooks-enabled", LISTENER)])
        .secrets(&[("media-key", Secret::Generated)])
        .children(&[single("edge-limits", Class::Live)]),
    single("email-client", Class::Live)
        .attrs(&[("idle-enabled", LISTENER)])
        .children(&[keyed("oauth-provider", &["id"], Class::Live).secrets(&[("client-secret", Secret::Kept)])]),
    single("ssh-server", LISTENER).secrets(&[("host-key", Secret::Generated)]),
    single("terminal", Class::Live).children(&[keyed("host-profile", &["name"], Class::Live)]),
    single("git-server", Class::Live)
        .attrs(&[("max-concurrent", LISTENER), ("max-queued", LISTENER)])
        .secrets(&[
            ("lfs-token-key", Secret::Generated),
            ("lfs-token-key-previous", Secret::Kept),
        ]),
    single("git-ci", Class::Live),
    single("dav-server", Class::Live).attrs(&[("sftp-drives", LISTENER)]),
    single("smtp-server", LISTENER),
    // the MTA's loop starts at boot; the rest is read at each delivery, the DKIM key at each signature
    single("mail-delivery", Class::Live)
        .attrs(&[("mta", LISTENER)])
        .children(&[single("dkim", Class::Live).secrets(&[("key", Secret::Kept)])]),
    single(
        "logging",
        Class::Restart("the log writer starts at boot; Monitor › Settings changes its levels for a while"),
    ),
    single("report", Class::Live),
    single("resource-watch", Class::Live),
    single("dev-reload", Class::Restart("the file watcher starts at boot")),
    single("dev-aide", Class::Live).attrs(&[("shared", BOOT)]),
    // the gate is read once at start, so a conf editor of a running server does not turn it off or point it at another
    // secret or repository (docs/tools/Component-Deploy.md "The gate")
    single(
        "component-deploy",
        Class::Restart("the gate, its secret and its allow entries are read once at start"),
    )
    .secrets(&[("attestation-secret", Secret::Kept)]),
    // vaults (plans/archive/Vault-Instances.md §3): read at each call of the vault services
    single("vault", Class::Live).secrets(&[("lease-key", Secret::Generated)]),
    // the manager link (plans/archive/Vault-Instances.md §4.4): read at start, by the sealed gate's dialer and the listener
    single(
        "manage",
        Class::Restart("the manager link is dialed and taken from the start"),
    ),
    // read by the desktop and mobile apps, not the server
    single("client-conf", Class::Live),
    sequence(
        "component-list",
        Class::Restart("components are found at boot; System › Components loads one on a running server"),
    ),
];

/// Whether `name` is a top-level element of the conf schema, which the merge has a rule of its own for; any other is
/// a component's settings element (`conf::merge_component_elements`).
#[must_use]
pub fn is_known_element(name: &str) -> bool {
    CATALOG.iter().any(|unit| unit.name == name)
}

/// The unit of the elements at `path`, the element names from below the root (`["webapp-list", "webapp"]`), or `None`
/// when no unit names them.
#[must_use]
pub fn unit(path: &[&str]) -> Option<&'static Unit> {
    let mut units: &'static [Unit] = CATALOG;
    let mut found = None;
    for name in path {
        let unit = units.iter().find(|unit| unit.name == *name)?;
        found = Some(unit);
        units = unit.children;
    }
    found
}

/// How the elements at `path` are told apart: their unit's key, or the first of the name ([`Key::Single`]) for an
/// element no unit names. What an environment name or a selector walks by (plans/archive/Conf-Paths.md §1).
#[must_use]
pub fn keying(path: &[&str]) -> Key {
    unit(path).map_or(Key::Single, |unit| unit.key)
}

/// Whether the attribute `attr` of the elements at `path` holds a secret: the schema marks it ([`Unit::secrets`]), or
/// its name says so ([`secret_name`]). Such a value is never logged, is masked wherever the conf is shown, and is null
/// to `ec.factory.getConfValue`.
#[must_use]
pub fn attribute_secret(path: &[&str], attr: &str) -> bool {
    unit(path).is_some_and(|unit| unit.secrets.iter().any(|(name, _)| *name == attr)) || secret_name(attr)
}

/// An attribute that holds a secret by its name: `password`, `jdbc-password`, `crypt-pass`, `api-key`,
/// `lfs-token-key`, `client-secret`.
#[must_use]
pub fn secret_name(name: &str) -> bool {
    let name = name.to_ascii_lowercase();
    // the flag that says whether a value is a secret
    if name == "is-secret" {
        return false;
    }
    let secret_words = [
        "password",
        "pass",
        "secret",
        "api-key",
        "token-key",
        "state-key",
        "private-key",
    ];
    secret_words
        .iter()
        .any(|word| name == *word || name.ends_with(&format!("-{word}")))
        || name.contains("token-key")
}

/// The key attributes the merge matches the elements at `path` by: their unit's, so the merge, the conf reload and an
/// environment name pick the same element. `path` names a keyed unit; a test merges every one.
pub(crate) fn merge_keys(path: &[&str]) -> &'static [&'static str] {
    match keying(path) {
        Key::By(attrs) => attrs,
        Key::Single | Key::Sequence => {
            debug_assert!(false, "{} is not a keyed unit of the conf schema", path.join("/"));
            &[]
        }
    }
}

#[cfg(test)]
#[path = "conf_schema_tests.rs"]
mod tests;
