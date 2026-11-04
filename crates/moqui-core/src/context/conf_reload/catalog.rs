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

//! What each part of the merged conf takes to change on a running server (plans/archive/Component-Deploy.md, Phase 1): read
//! on each use (live), built from the conf by a hook (apply), or read once at boot (restart). The catalog is a tree of
//! units by element name, each with its key, its class, the attributes whose class differs, and its child units. What
//! no unit names is part of its parent unit; at the top, an element no unit names needs a restart, so an element added
//! to the conf without a catalog entry is never reported as live.

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
    /// The `default-property` entries, set as boot sets them unless the environment set them.
    Properties,
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
            Self::Properties => "properties",
            Self::WebProtection => "web-protection",
        }
    }
}

/// How the elements of one unit are told apart.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Key {
    /// The first element of the name is the unit.
    Single,
    /// Each element is a unit, by the values of these attributes, as the merge keys them.
    By(&'static [&'static str]),
    /// The elements of the name together, in order, are one unit: a list the merge appends to.
    Sequence,
}

/// A unit of the conf: an element name, how its elements are told apart, its class, the attributes whose class
/// differs from it, and its child units.
#[derive(Clone, Copy, Debug)]
pub struct Unit {
    pub name: &'static str,
    pub key: Key,
    pub class: Class,
    pub attrs: &'static [(&'static str, Class)],
    pub children: &'static [Unit],
}

impl Unit {
    const fn new(name: &'static str, key: Key, class: Class) -> Self {
        Self {
            name,
            key,
            class,
            attrs: &[],
            children: &[],
        }
    }

    const fn attrs(mut self, attrs: &'static [(&'static str, Class)]) -> Self {
        self.attrs = attrs;
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

/// The class of an element at the top of the conf that no unit names.
pub const UNKNOWN: Class = Class::Restart("not known to the conf reload");

/// The units of the conf, by the top-level element. The classes follow what reads each setting; a setting is live
/// only where its reader reads the conf on each use.
pub static CATALOG: &[Unit] = &[
    keyed("default-property", &["name"], Class::Apply(Group::Properties)),
    single(
        "tools",
        Class::Restart("the tool factories and the worker and request pools start at boot"),
    )
    .children(&[keyed(
        "tool-factory",
        &["class"],
        Class::Restart("tool factories start at boot"),
    )]),
    single("cache-list", BOOT).children(&[keyed("cache", &["name"], Class::Apply(Group::Caches))]),
    single(
        "telemetry",
        Class::Restart("read once at boot; Monitor › Settings changes its levels and rates for a while"),
    ),
    // only read at boot for deprecation warnings
    single("server-stats", Class::Live),
    single("webapp-list", BOOT).children(&[keyed("webapp", &["name"], Class::Live)
        .attrs(&[
            ("http-port", LISTENER),
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
            keyed("public-site", &["host"], Class::Live),
            single("first-hit-in-visit", DERIVED),
            single("after-request", DERIVED),
            single("before-request", DERIVED),
            single("after-login", DERIVED),
            single("before-logout", DERIVED),
            single("after-startup", NEXT_START),
            single("before-shutdown", DERIVED),
            keyed("filter", &["name"], DERIVED),
            keyed("listener", &["class"], DERIVED),
            keyed("servlet", &["name"], DERIVED),
            single("session-config", Class::Live).attrs(&[("flush", BOOT), ("max", BOOT)]),
            keyed(
                "endpoint",
                &["path"],
                Class::Restart("the HTTP routes are built when the server starts"),
            ),
            keyed("response-header", &["type", "name"], Class::Live),
            // the listener's limits are read when it starts
            single("web-protection", Class::Apply(Group::WebProtection))
                .children(&[single("connections", LISTENER), single("requests", LISTENER)]),
        ])]),
    single("artifact-execution-facade", Class::Live).children(&[keyed("artifact-execution", &["type"], Class::Live)]),
    single("user-facade", Class::Live),
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
        single("thread-pool", BOOT),
        sequence("jms-service", Class::Live),
    ]),
    single(
        "search-facade",
        Class::Restart("the search backend is attached at boot"),
    ),
    single("llm-facade", Class::Restart("the agent runtime is built at boot")),
    single("entity-facade", Class::Restart("the entity facade is opened at boot")).children(&[
        keyed(
            "datasource",
            &["group-name"],
            Class::Restart("datasources connect at boot"),
        ),
        single("server-jndi", BOOT),
        sequence(
            "load-entity",
            Class::Restart("the entity model reads them when it is opened"),
        ),
        sequence("load-data", Class::Live),
    ]),
    single(
        "database-list",
        Class::Restart("the database dialects are read when the entity facade is opened"),
    ),
    single("repository-list", Class::Live),
    single("edge", Class::Restart("the edge link starts at boot")),
    single("email-client", Class::Live).attrs(&[("idle-enabled", LISTENER)]),
    single("ssh-server", LISTENER),
    single("terminal", Class::Live),
    single("git-server", Class::Live).attrs(&[("max-concurrent", LISTENER), ("max-queued", LISTENER)]),
    single("dav-server", Class::Live).attrs(&[("sftp-drives", LISTENER)]),
    single("smtp-server", LISTENER),
    single(
        "logging",
        Class::Restart("the log writer starts at boot; Monitor › Settings changes its levels for a while"),
    ),
    single("report", Class::Live),
    single("dev-reload", Class::Restart("the file watcher starts at boot")),
    single("dev-aide", Class::Live).attrs(&[("shared", BOOT)]),
    // read by the desktop and mobile apps, not the server
    single("client-conf", Class::Live),
    sequence(
        "component-list",
        Class::Restart("components are found at boot; System › Components loads one on a running server"),
    ),
];

/// Properties a conf reload never changes: what the process, its listeners, its databases or its keys were set up
/// with at boot.
const FIXED_PROPERTY_PREFIXES: &[&str] = &[
    "entity_ds_",
    "content_ds_",
    "cluster_",
    "edge_",
    "elasticsearch_",
    "entity_add_missing_",
    "entity_on_start_",
    "moqui_log_",
];
const FIXED_PROPERTY_SUFFIXES: &[&str] = &["_port", "_host", "_bind", "_key", "_secret", "_pass", "_password"];
const FIXED_PROPERTIES: &[&str] = &[
    "instance_purpose",
    "instance_id",
    "node_role",
    "MOQUI_CONF",
    "default_time_zone",
    "database_time_zone",
    "default_locale",
    "search_backend",
    "webapp_trusted_proxies",
    "session_cookie_key",
];

/// Why the property `name` is not changed by a conf reload, or `None` when it may be.
#[must_use]
pub fn fixed_property(name: &str) -> Option<&'static str> {
    let fixed = FIXED_PROPERTIES.contains(&name)
        || FIXED_PROPERTY_PREFIXES.iter().any(|prefix| name.starts_with(prefix))
        || FIXED_PROPERTY_SUFFIXES.iter().any(|suffix| name.ends_with(suffix));
    fixed.then_some("the process, a listener, a database or a key was set up with it at boot")
}
