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

//! The user appearance policy of a Quasar theme: what the user may change in Appearance, and the
//! mode the theme forces (plans/Themes.md, Appearance policy). The policy is a
//! `ScreenThemeResource` value of type `STRT_APPEARANCE_POLICY`; absent allows every appearance
//! preference except the accent, which the theme sets unless the policy allows the user's. The
//! server checks it before writing a preference, and the shell sends it to the page so the controls
//! it locks are not shown.

use super::theme::{theme_resource_values, user_theme_id, DEFAULT_QUASAR_THEME, QUASAR_THEME_TYPE};

/// The theme resource type of the appearance policy.
pub(crate) const APPEARANCE_POLICY: &str = "STRT_APPEARANCE_POLICY";

/// What a user may change in Appearance. The accent is the theme's unless the policy allows it; a
/// theme with a malformed policy is locked rather than trusted.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AppearancePolicy {
    /// False hides the appearance controls and refuses every appearance preference.
    pub appearance: bool,
    /// The mode the theme forces: `light`, `dark`, `auto`, or empty for the user's choice.
    pub mode: String,
    pub scheme: bool,
    pub accent: bool,
    pub fonts: bool,
    pub sizes: bool,
    pub motion: bool,
    pub density: bool,
    pub colors: bool,
}

impl Default for AppearancePolicy {
    fn default() -> Self {
        Self {
            appearance: true,
            mode: String::new(),
            scheme: true,
            accent: false,
            fonts: true,
            sizes: true,
            motion: true,
            density: true,
            colors: true,
        }
    }
}

/// True for a preference key the appearance policy governs.
pub(crate) fn is_appearance_key(key: &str) -> bool {
    matches!(key, "QUASAR_DARK" | "SHELL_DENSITY") || key.starts_with("THEME_")
}

impl AppearancePolicy {
    /// Every appearance preference is refused.
    pub fn locked() -> Self {
        Self {
            appearance: false,
            ..Self::default()
        }
    }

    /// The policy of `text`, or the permissive default when there is none. A policy that is not JSON
    /// or not an object is logged and locked.
    pub fn parse(text: Option<&str>) -> Self {
        let Some(text) = text.map(str::trim).filter(|text| !text.is_empty()) else {
            return Self::default();
        };
        let Ok(value) = serde_json::from_str::<serde_json::Value>(text) else {
            tracing::error!("Theme {} is not JSON, user appearance is locked", APPEARANCE_POLICY);
            return Self::locked();
        };
        let Some(object) = value.as_object() else {
            tracing::error!(
                "Theme {} is not a JSON object, user appearance is locked",
                APPEARANCE_POLICY
            );
            return Self::locked();
        };
        let flag = |name: &str| object.get(name).and_then(serde_json::Value::as_bool).unwrap_or(true);
        // The accent is the theme's own primary unless the theme allows the user's: an absent field locks it.
        let accent = object
            .get("accent")
            .and_then(serde_json::Value::as_bool)
            .unwrap_or(false);
        let mode = object
            .get("mode")
            .and_then(serde_json::Value::as_str)
            .map(str::trim)
            .filter(|mode| matches!(*mode, "light" | "dark" | "auto"))
            .unwrap_or_default()
            .to_string();
        Self {
            appearance: flag("appearance"),
            mode,
            scheme: flag("scheme"),
            accent,
            fonts: flag("fonts"),
            sizes: flag("sizes"),
            motion: flag("motion"),
            density: flag("density"),
            colors: flag("colors"),
        }
    }

    /// The policy as the page and the preferences service read it.
    pub fn as_json(&self) -> String {
        serde_json::json!({
            "appearance": self.appearance,
            "mode": self.mode,
            "scheme": self.scheme,
            "accent": self.accent,
            "fonts": self.fonts,
            "sizes": self.sizes,
            "motion": self.motion,
            "density": self.density,
            "colors": self.colors,
        })
        .to_string()
    }

    /// Why `key` may not be written, or `None` when the policy allows it. Keys that are not an
    /// appearance preference are always allowed.
    pub fn denial(&self, key: &str) -> Option<String> {
        if !self.appearance {
            return match key {
                "QUASAR_DARK" | "SHELL_DENSITY" => Some(format!("The theme does not allow the {key} preference")),
                other if other.starts_with("THEME_") => Some(format!("The theme does not allow the {key} preference")),
                _ => None,
            };
        }
        match key {
            "QUASAR_DARK" => (!self.mode.is_empty()).then(|| format!("The theme forces {} mode", self.mode)),
            "SHELL_DENSITY" => self.denied(key, self.density),
            "THEME_COLOR_SCHEME" => self.denied(key, self.scheme),
            "THEME_PRIMARY_COLOR" | "THEME_PRIMARY_COLOR_DARK" => self.denied(key, self.accent),
            "THEME_FONT_BODY" | "THEME_FONT_HEADING" | "THEME_FONT_MONO" => self.denied(key, self.fonts),
            "THEME_SIZE_SMALL" | "THEME_SIZE_LARGE" => self.denied(key, self.sizes),
            "THEME_MOTION" | "THEME_LINE_HEIGHT" | "THEME_LINKS_UNDERLINE" | "THEME_RADIUS" => {
                self.denied(key, self.motion)
            }
            "THEME_COLOR_OVERRIDES" => self.denied(key, self.colors),
            other if other.starts_with("THEME_") => None,
            _ => None,
        }
    }

    fn denied(&self, key: &str, allowed: bool) -> Option<String> {
        (!allowed).then(|| format!("The theme does not allow the {key} preference"))
    }
}

/// The policy of the Quasar theme of `user_id`, resolved as the shell resolves it. A missing
/// `STRT_APPEARANCE_POLICY` is permissive; one that cannot be read is logged and permissive.
pub(crate) fn appearance_policy_for_user(user_id: Option<&str>) -> AppearancePolicy {
    let theme = user_theme_id(user_id, QUASAR_THEME_TYPE, DEFAULT_QUASAR_THEME);
    match theme_resource_values(&theme, APPEARANCE_POLICY) {
        Ok(values) => AppearancePolicy::parse(values.first().map(String::as_str)),
        Err(err) => {
            tracing::error!("Could not read {APPEARANCE_POLICY} of theme {theme}: {err}");
            AppearancePolicy::default()
        }
    }
}

#[cfg(test)]
#[path = "appearance_tests.rs"]
mod tests;
