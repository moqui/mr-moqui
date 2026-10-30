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

//! The package and features this binary was built with (docs/Build-Test.md): the framework's own features, from this
//! crate's configuration, and the host's, which the host registers once at start. `moqui version` prints them, the
//! server logs them at boot, and `moqui test` skips a spec whose `requires` names one that is missing.

use std::sync::OnceLock;

/// What the host registered: its package name and its features.
static HOST: OnceLock<(&'static str, Vec<&'static str>)> = OnceLock::new();

/// Source revision embedded at build time, or `unknown` for a source archive without Git metadata.
pub const GIT_REVISION: &str = env!("MOQUI_GIT_REVISION");

/// The framework features in this build.
#[must_use]
pub fn core_features() -> Vec<&'static str> {
    [
        ("smtp", cfg!(feature = "smtp")),
        ("imap", cfg!(feature = "imap")),
        ("http", cfg!(feature = "http")),
        ("cluster-tls", cfg!(feature = "cluster-tls")),
        ("git", cfg!(feature = "git")),
        ("image", cfg!(feature = "image")),
        ("xlsx", cfg!(feature = "xlsx")),
        ("llm", crate::llm_hooks::linked()),
        ("telemetry", cfg!(feature = "telemetry")),
    ]
    .into_iter()
    .filter_map(|(name, built)| built.then_some(name))
    .collect()
}

/// Record the host's package and features. The first call wins; the host makes it before anything reads them.
pub fn set_host(package: &'static str, features: &[&'static str]) {
    let _ = HOST.set((package, features.to_vec()));
}

/// The package the host was built as (`dev`, `lite`, `debug`, `enterprise`, `maximal`), or `custom`; empty before the host registers.
#[must_use]
pub fn package() -> &'static str {
    HOST.get().map_or("", |(package, _)| package)
}

/// Every feature in this build, the framework's and the host's, sorted.
#[must_use]
pub fn features() -> Vec<&'static str> {
    let mut all = core_features();
    if let Some((_, host)) = HOST.get() {
        all.extend(host.iter().copied());
    }
    all.sort_unstable();
    all.dedup();
    all
}

/// True when `name` is a feature of this build. `mail` is `smtp` and `imap` together.
#[must_use]
pub fn has_feature(name: &str) -> bool {
    let all = features();
    match name {
        "mail" => all.contains(&"smtp") && all.contains(&"imap"),
        _ => all.contains(&name),
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn mail_is_both_halves() {
        assert_eq!(
            super::has_feature("mail"),
            super::has_feature("smtp") && super::has_feature("imap")
        );
        assert!(!super::has_feature("no-such-feature"));
    }
}
