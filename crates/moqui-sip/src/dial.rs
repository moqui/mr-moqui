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

//! What a dialed number is (plans/Comms-Framework.md §5.2 "Fraud", "Emergency calls"): its E.164 form and its class.
//! The voice services decide a call with the same rules (`Voice.js`); the engine checks them again before it sends an
//! INVITE, so a call the deployment does not allow never reaches a trunk whatever asked for it. Emergency numbers come
//! first: they are dialed as they are, before any other rule.

/// The deployment's dial rules (`VoicePolicy`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DialPolicy {
    /// The country code of national numbers, `1` for the North American plan.
    pub home_country: String,
    /// The emergency numbers, as dialed (`911`, `112`, `999`).
    pub emergency_numbers: Vec<String>,
    /// E.164 prefixes of premium-rate numbers (`+1900`).
    pub premium_prefixes: Vec<String>,
    pub international_enabled: bool,
    pub premium_enabled: bool,
}

impl Default for DialPolicy {
    fn default() -> Self {
        DialPolicy {
            home_country: "1".to_string(),
            emergency_numbers: vec!["911".to_string(), "112".to_string(), "999".to_string()],
            premium_prefixes: vec!["+1900".to_string(), "+1976".to_string()],
            international_enabled: false,
            premium_enabled: false,
        }
    }
}

/// A number's class.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DestinationClass {
    Emergency,
    Domestic,
    International,
    Premium,
}

impl DestinationClass {
    pub fn code(self) -> &'static str {
        match self {
            DestinationClass::Emergency => "emergency",
            DestinationClass::Domestic => "domestic",
            DestinationClass::International => "international",
            DestinationClass::Premium => "premium",
        }
    }

    pub fn parse(code: &str) -> Option<DestinationClass> {
        match code {
            "emergency" => Some(DestinationClass::Emergency),
            "domestic" => Some(DestinationClass::Domestic),
            "international" => Some(DestinationClass::International),
            "premium" => Some(DestinationClass::Premium),
            _ => None,
        }
    }
}

/// The digits of a dialed string (and a leading `+`), without spaces, dashes, dots or parentheses; `None` when it has
/// anything else.
fn digits_of(dialed: &str) -> Option<String> {
    let mut out = String::new();
    for (i, c) in dialed.trim().chars().enumerate() {
        match c {
            '0'..='9' => out.push(c),
            '+' if i == 0 => out.push(c),
            ' ' | '-' | '.' | '(' | ')' => {}
            _ => return None,
        }
    }
    (!out.is_empty() && out != "+").then_some(out)
}

/// The E.164 form of a number (`+` and up to 15 digits), with the home country for a national number. An emergency
/// number stays as dialed.
pub fn normalize(dialed: &str, policy: &DialPolicy) -> Option<String> {
    let digits = digits_of(dialed)?;
    if policy.emergency_numbers.contains(&digits) {
        return Some(digits);
    }
    let e164 = if let Some(rest) = digits.strip_prefix('+') {
        format!("+{rest}")
    } else if let Some(rest) = digits.strip_prefix("00") {
        format!("+{rest}")
    } else if policy.home_country == "1" {
        if let Some(rest) = digits.strip_prefix("011") {
            format!("+{rest}")
        } else if digits.len() == 10 {
            format!("+1{digits}")
        } else if digits.len() == 11 && digits.starts_with('1') {
            format!("+{digits}")
        } else {
            return None;
        }
    } else {
        let national = digits.strip_prefix('0').unwrap_or(&digits);
        format!("+{}{national}", policy.home_country)
    };
    let len = e164.len() - 1;
    ((8..=15).contains(&len) && !e164[1..].starts_with('0')).then_some(e164)
}

/// The class of a dialed number with its E.164 form (an emergency number as dialed). `None` when it is not a number.
pub fn classify(dialed: &str, policy: &DialPolicy) -> Option<(String, DestinationClass)> {
    let number = normalize(dialed, policy)?;
    if policy.emergency_numbers.contains(&number) {
        return Some((number, DestinationClass::Emergency));
    }
    let class = if policy
        .premium_prefixes
        .iter()
        .any(|p| !p.is_empty() && number.starts_with(p.as_str()))
    {
        DestinationClass::Premium
    } else if number.starts_with(&format!("+{}", policy.home_country)) {
        DestinationClass::Domestic
    } else {
        DestinationClass::International
    };
    Some((number, class))
}

/// Whether the deployment allows a call to this class at all; the user's own grants are the services'. An emergency
/// call is always allowed here (it still needs an emergency-capable trunk).
pub fn deployment_allows(class: DestinationClass, policy: &DialPolicy) -> Result<(), &'static str> {
    match class {
        DestinationClass::International if !policy.international_enabled => Err("internationalNotEnabled"),
        DestinationClass::Premium if !policy.premium_enabled => Err("premiumNotEnabled"),
        _ => Ok(()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn numbers_and_their_classes() {
        let policy = DialPolicy::default();
        assert_eq!(normalize("(555) 123-4567", &policy).as_deref(), Some("+15551234567"));
        assert_eq!(normalize("1 555 123 4567", &policy).as_deref(), Some("+15551234567"));
        assert_eq!(normalize("+44 20 7946 0958", &policy).as_deref(), Some("+442079460958"));
        assert_eq!(
            normalize("011 44 20 7946 0958", &policy).as_deref(),
            Some("+442079460958")
        );
        assert_eq!(normalize("911", &policy).as_deref(), Some("911"));
        assert_eq!(normalize("12", &policy), None);
        assert_eq!(normalize("555-CALL", &policy), None);
        assert_eq!(normalize("+", &policy), None);
        let uk = DialPolicy {
            home_country: "44".to_string(),
            ..DialPolicy::default()
        };
        assert_eq!(normalize("020 7946 0958", &uk).as_deref(), Some("+442079460958"));

        assert_eq!(classify("911", &policy).unwrap().1, DestinationClass::Emergency);
        assert_eq!(classify("555 123 4567", &policy).unwrap().1, DestinationClass::Domestic);
        assert_eq!(
            classify("+442079460958", &policy).unwrap().1,
            DestinationClass::International
        );
        assert_eq!(
            classify("1-900-555-0100", &policy).unwrap().1,
            DestinationClass::Premium
        );
        assert_eq!(
            deployment_allows(DestinationClass::International, &policy),
            Err("internationalNotEnabled")
        );
        assert_eq!(
            deployment_allows(DestinationClass::Premium, &policy),
            Err("premiumNotEnabled")
        );
        assert_eq!(deployment_allows(DestinationClass::Emergency, &policy), Ok(()));
        let open = DialPolicy {
            international_enabled: true,
            ..DialPolicy::default()
        };
        assert_eq!(deployment_allows(DestinationClass::International, &open), Ok(()));
    }
}
