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

//! The client's settings from the Moqui XML conf, so each build embeds its own (plans/Mobile-Desktop.md §6, MD4): the
//! `<client-conf>` element, which the host reads from its merged conf and hands over (this crate has no framework).
//!
//! ```xml
//! <client-conf device-mode="choose" mirror-days="90" prefetch-rooms="20" prefetch-mail="100" store-max-mb="512"/>
//! ```

use moqui_lib::node::MNode;
use serde::{Deserialize, Serialize};

/// Whose device this is, which decides how much of an account's data is kept and for how long.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum DeviceMode {
    /// The sign-in screen asks, and the answer is kept per account.
    #[default]
    Choose,
    /// A person's own phone or desktop: a full mirror within the limits, kept on sign-out and revocation, deleted
    /// only with the account.
    Personal,
    /// A hot desk, a packing terminal, a point of sale: what was seen and a small prefetch, deleted on sign-out and
    /// revocation.
    Shared,
}

impl DeviceMode {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Choose => "choose",
            Self::Personal => "personal",
            Self::Shared => "shared",
        }
    }

    pub fn parse(text: &str) -> Option<Self> {
        Some(match text {
            "choose" => Self::Choose,
            "personal" => Self::Personal,
            "shared" => Self::Shared,
            _ => return None,
        })
    }
}

/// The `<client-conf>` settings.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ClientConf {
    /// `device-mode`: `choose` by default, so one build serves both kinds of device.
    pub device_mode: DeviceMode,
    /// `mirror-days`: how far back a personal device mirrors rooms and mail; 90 by default, a season of work. At
    /// least 1.
    pub mirror_days: u32,
    /// `prefetch-rooms`: how many of the latest rooms a shared device reads ahead; 20 by default, a screen's list.
    pub prefetch_rooms: u32,
    /// `prefetch-mail`: how many of the inbox's latest threads a shared device reads ahead; 100 by default, about two
    /// pages.
    pub prefetch_mail: u32,
    /// `store-max-mb`: the size cap of an account's store in MB; 512 by default, room for months of chat and mail
    /// text on a phone. At least 1.
    pub store_max_mb: u64,
}

impl Default for ClientConf {
    fn default() -> Self {
        Self {
            device_mode: DeviceMode::Choose,
            mirror_days: 90,
            prefetch_rooms: 20,
            prefetch_mail: 100,
            store_max_mb: 512,
        }
    }
}

impl ClientConf {
    /// The settings of the text of a `<client-conf .../>` element; the defaults for text that does not parse or is
    /// another element, and for each attribute that is missing; a bad value or an unknown attribute is logged and its
    /// default used.
    pub fn parse(text: &str) -> Self {
        match MNode::parse_text("client-conf", text) {
            Ok(node) => Self::from_node(&node),
            Err(err) => {
                tracing::warn!("client-conf does not parse, using the defaults: {err}");
                Self::default()
            }
        }
    }

    /// The settings of a `<client-conf>` element already parsed, as the host's merged conf holds it.
    pub fn from_node(node: &MNode) -> Self {
        let mut conf = Self::default();
        if !node.has_name("client-conf") {
            tracing::warn!("{} is not client-conf, using the defaults", node.name());
            return conf;
        }
        node.for_each_attribute(|name, value| match name {
            "device-mode" => match DeviceMode::parse(value.trim()) {
                Some(mode) => conf.device_mode = mode,
                None => bad(name, value, conf.device_mode.as_str()),
            },
            "mirror-days" => number(name, value, 1, &mut conf.mirror_days),
            "prefetch-rooms" => number(name, value, 0, &mut conf.prefetch_rooms),
            "prefetch-mail" => number(name, value, 0, &mut conf.prefetch_mail),
            "store-max-mb" => number(name, value, 1, &mut conf.store_max_mb),
            _ => tracing::warn!("client-conf attribute {name} is not known, ignored"),
        });
        conf
    }

    /// The store's size cap in bytes.
    pub fn store_max_bytes(&self) -> u64 {
        self.store_max_mb.saturating_mul(1024 * 1024)
    }
}

/// Set `field` to `value` when it is a number of at least `least`; else log and keep its default.
fn number<T>(name: &str, value: &str, least: T, field: &mut T)
where
    T: std::str::FromStr + PartialOrd + std::fmt::Display,
{
    let parsed: Result<T, _> = value.trim().parse();
    match parsed {
        Ok(number) if number >= least => *field = number,
        _ => bad(name, value, &field.to_string()),
    }
}

fn bad(name: &str, value: &str, default: &str) {
    tracing::warn!("client-conf {name}=\"{value}\" is not valid, using {default}");
}

#[cfg(test)]
#[path = "conf_tests.rs"]
mod tests;
