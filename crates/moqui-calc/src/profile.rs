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

//! Access profiles (plans/IronCalc-DB.md, Access and row scope): what a person who is not an administrator may read
//! through CalcGrid, set in the authorization data and not in a document.
//!
//! A profile is an artifact group. Its members, all service-type artifacts, are the profile artifact
//! `org.moqui.calc.profile.<profileId>` (`inheritAuthz` Y) and the entities it offers, each as
//! `org.moqui.calc.entity.<entityName>` (a name, or a pattern of names). An offer is a name of this engine's, as a
//! report dataset's artifact is: it grants nothing on the entity itself, so listing a view's member entities opens
//! none of them anywhere else. The profile's grants are `ArtifactAuthz` records of that group, `AUTHZT_ALWAYS` for
//! view, and the row scope is the `ArtifactAuthzFilter` rows on those records. The engine enters the profile artifact before it reads
//! ([`Profile::enter`]), so every find below is authorized by the profile's record and gets that record's entity
//! filters, whatever other grants the user holds.
//!
//! Two rules are the engine's own, since a frame entered with an inheritable always-allow lets every find below pass:
//! an entity or view is read only when the profile offers it and every member entity of a view ([`Profile::offers`]),
//! and, under a record that has filter sets, only when a filter applies to it
//! ([`Profile::check_scoped`]), so an entity nobody wrote a filter for is refused, not read whole.
//!
//! The Tools app's own access (no profile) is the `admin` case: every source the data grid offers, no filter.

use std::collections::BTreeSet;

use moqui_core::context::{Authz, Error, ExecutionContext, Tarpit};
use moqui_core::entity::EntityFacade;
use moqui_lib::object::{to_plain_string, Value};

/// The artifact name of a profile is this and its id.
pub const PROFILE_PREFIX: &str = "org.moqui.calc.profile.";
/// The artifact name of an offered entity is this and the entity's full name.
pub const ENTITY_PREFIX: &str = "org.moqui.calc.entity.";
/// Who holds update on this artifact authors documents: makes one over a source and saves it. Without it a holder of a
/// profile opens the documents shared with them.
pub const DOCUMENT_ARTIFACT: &str = "org.moqui.calc.document";
const NOT_AVAILABLE: &str = "Not available";

/// One offered entity's artifact name, or a pattern of such names.
#[derive(Clone, Debug, PartialEq, Eq)]
struct Offer {
    name: String,
    pattern: bool,
}

/// A profile the caller holds.
#[derive(Clone, Debug)]
pub struct Profile {
    pub id: String,
    /// The artifact group's description.
    pub description: String,
    offers: Vec<Offer>,
}

fn field(row: &moqui_core::entity::EntityValue, name: &str) -> String {
    row.get(name).map(to_plain_string).unwrap_or_default()
}

/// Whether `name` is `pattern` as a whole (an artifact name pattern is a regular expression).
fn pattern_matches(pattern: &str, name: &str) -> bool {
    regex::Regex::new(&format!("^(?:{pattern})$")).is_ok_and(|compiled| compiled.is_match(name))
}

/// The artifact groups that have the profile artifact as a member, with each one's description.
fn profile_groups(facade: &EntityFacade, artifact: &str) -> Result<Vec<(String, String)>, Error> {
    let members = facade
        .find("moqui.security.ArtifactGroupMember")?
        .eq("artifactName", Value::string(artifact))
        .eq("artifactTypeEnumId", Value::string("AT_SERVICE"))
        .list()?;
    let mut groups = Vec::new();
    for member in members.values() {
        let id = field(member, "artifactGroupId");
        let description = facade
            .find("moqui.security.ArtifactGroup")?
            .eq("artifactGroupId", Value::string(&id))
            .one()?
            .map(|group| field(&group, "description"))
            .unwrap_or_default();
        groups.push((id, description));
    }
    Ok(groups)
}

impl Profile {
    /// The profile `id` when the caller holds it for view, checked whatever the caller disabled.
    ///
    /// # Errors
    ///
    /// `Not available` for a profile that does not exist or the caller does not hold: one answer for both.
    pub fn held(facade: &EntityFacade, id: &str) -> Result<Self, Error> {
        let plain = !id.is_empty()
            && id
                .chars()
                .all(|ch| ch.is_ascii_alphanumeric() || ch == '_' || ch == '-');
        if !plain {
            return Err(Error::new(NOT_AVAILABLE));
        }
        let artifact = format!("{PROFILE_PREFIX}{id}");
        let _required = ExecutionContext::with_current(ExecutionContext::require_authz);
        if !moqui_core::user::permitted_own("AT_SERVICE", &artifact, "view").unwrap_or(false) {
            tracing::info!(profile = id, "Refused a CalcGrid profile");
            return Err(Error::new(NOT_AVAILABLE));
        }
        drop(_required);
        let _authz = ExecutionContext::with_current(ExecutionContext::disable_authz);
        let groups = profile_groups(facade, &artifact)?;
        let mut offers = Vec::new();
        for (group, _) in &groups {
            let members = facade
                .find("moqui.security.ArtifactGroupMember")?
                .eq("artifactGroupId", Value::string(group))
                .eq("artifactTypeEnumId", Value::string("AT_SERVICE"))
                .list()?;
            for member in members
                .values()
                .iter()
                .filter(|member| field(member, "artifactName") != artifact)
            {
                offers.push(Offer {
                    name: field(member, "artifactName"),
                    pattern: field(member, "nameIsPattern") == "Y",
                });
            }
        }
        Ok(Self {
            id: id.to_string(),
            description: groups
                .into_iter()
                .map(|(_, description)| description)
                .next()
                .unwrap_or_default(),
            offers,
        })
    }

    /// The ids of the profiles the caller holds, in order.
    ///
    /// # Errors
    ///
    /// Returns the facade's error.
    pub fn held_ids(facade: &EntityFacade) -> Result<Vec<String>, Error> {
        let names: BTreeSet<String> = {
            let _authz = ExecutionContext::with_current(ExecutionContext::disable_authz);
            facade
                .find("moqui.security.ArtifactGroupMember")?
                .condition(moqui_core::entity::Condition::like(
                    "artifactName",
                    Value::string(format!("{PROFILE_PREFIX}%")),
                ))
                .eq("artifactTypeEnumId", Value::string("AT_SERVICE"))
                .list()?
                .values()
                .iter()
                .map(|member| field(member, "artifactName"))
                .collect()
        };
        let _required = ExecutionContext::with_current(ExecutionContext::require_authz);
        Ok(names
            .into_iter()
            .filter(|artifact| moqui_core::user::permitted_own("AT_SERVICE", artifact, "view").unwrap_or(false))
            .filter_map(|artifact| artifact.strip_prefix(PROFILE_PREFIX).map(str::to_string))
            .collect())
    }

    fn offers_name(&self, entity_name: &str) -> bool {
        let artifact = format!("{ENTITY_PREFIX}{entity_name}");
        self.offers.iter().any(|offer| {
            if offer.pattern {
                pattern_matches(&offer.name, &artifact)
            } else {
                offer.name == artifact
            }
        })
    }

    /// Whether the profile offers `entity_name`: the entity is one of its members, and for a view every member entity
    /// of the view is too, at any depth.
    #[must_use]
    pub fn offers(&self, facade: &EntityFacade, entity_name: &str) -> bool {
        self.offers_at(facade, entity_name, 0)
    }

    fn offers_at(&self, facade: &EntityFacade, entity_name: &str, depth: usize) -> bool {
        if depth > 8 || !self.offers_name(entity_name) {
            return false;
        }
        let members: Vec<String> = facade
            .with_entity(entity_name, |entity, _, _| {
                Ok(entity
                    .member_entities
                    .iter()
                    .map(|member| member.entity_name.clone())
                    .collect())
            })
            .unwrap_or_default();
        members.iter().all(|member| {
            // a member is named in full or by its short name; the model knows which entity it is
            let full = facade
                .with_entity(member, |entity, _, _| Ok(entity.full_name.clone()))
                .unwrap_or_else(|_| member.clone());
            self.offers_at(facade, &full, depth + 1)
        })
    }

    /// The entities and views of the model the profile offers whose name holds `term` (any, when empty), at most
    /// `limit`.
    #[must_use]
    pub fn offered_entities(&self, facade: &EntityFacade, term: &str, limit: usize) -> Vec<String> {
        let term = term.to_lowercase();
        let mut names: Vec<String> = moqui_core::entity::info::all_entity_names(facade, "")
            .into_iter()
            .filter(|name| term.is_empty() || name.to_lowercase().contains(&term))
            .filter(|name| self.offers(facade, name))
            .collect();
        names.sort_unstable();
        names.truncate(limit);
        names
    }

    /// Enter the profile artifact: until the guard drops, finds are authorized by the profile's record and get its
    /// entity filters. Authorization is required while it is entered, whatever the caller disabled.
    ///
    /// # Errors
    ///
    /// `Not available` when the caller does not hold the profile.
    pub fn enter(&self) -> Result<Entered, Error> {
        user_organizations();
        let artifact = format!("{PROFILE_PREFIX}{}", self.id);
        let required = ExecutionContext::with_current(ExecutionContext::require_authz);
        let pushed = ExecutionContext::with_current(|ec| {
            ec.push_checked("AT_SERVICE", &artifact, "view", Authz::Required, Tarpit::Skip, None)
        });
        match pushed {
            Some(Ok(())) => Ok(Entered { _required: required }),
            _ => Err(Error::new(NOT_AVAILABLE)),
        }
    }

    /// With the profile entered: refuse `entity_name` when the profile's record has filter sets and none of their
    /// filters applies to the entity, so an entity nobody scoped is not read whole. A record with no filter set (an
    /// unscoped profile) reads what it offers.
    ///
    /// # Errors
    ///
    /// `Not available` for such an entity.
    pub fn check_scoped(&self, facade: &EntityFacade, entity_name: &str) -> Result<(), Error> {
        if !self.scoped(facade)? {
            return Ok(());
        }
        let filters = moqui_core::user::find_filters_for_user(entity_name)?;
        if filters.is_empty() {
            tracing::warn!(profile = %self.id, entity = entity_name, "A scoped CalcGrid profile offers an entity no filter applies to; refused");
            return Err(Error::new(NOT_AVAILABLE));
        }
        Ok(())
    }

    /// Whether any grant of the profile's groups has a filter set.
    fn scoped(&self, facade: &EntityFacade) -> Result<bool, Error> {
        let _authz = ExecutionContext::with_current(ExecutionContext::disable_authz);
        for (group, _) in profile_groups(facade, &format!("{PROFILE_PREFIX}{}", self.id))? {
            let grants = facade
                .find("moqui.security.ArtifactAuthz")?
                .eq("artifactGroupId", Value::string(&group))
                .list()?;
            for grant in grants.values() {
                let filtered = facade
                    .find("moqui.security.ArtifactAuthzFilter")?
                    .eq("artifactAuthzId", Value::string(field(grant, "artifactAuthzId")))
                    .count()?;
                if filtered > 0 {
                    return Ok(true);
                }
            }
        }
        Ok(false)
    }
}

/// The service of an application that puts the user's organizations in the user context (`filterOrgIds`), which the
/// entity filters of a scoped profile read: Mantle's, as an app's root screen and CalcGrid's call it.
const USER_ORGANIZATIONS_SERVICE: &str = "mantle.party.PartyServices.setup#UserOrganizationInfo";

/// Put the user's organizations in the user context when nothing has yet and the application has the service: a call
/// that reaches a profile with no screen before it (a scheduled export, a report's scope check, a dashboard panel)
/// reads the same rows as the CalcGrid screen. A failure is logged and the filters then match nothing.
fn user_organizations() {
    let missing = ExecutionContext::with_current(|ec| !ec.user_context().contains_key("filterOrgIds")).unwrap_or(false);
    if !missing
        || moqui_core::user::current_user_id().ok().flatten().is_none()
        || !moqui_core::service::call::is_service_defined(USER_ORGANIZATIONS_SERVICE, None)
    {
        return;
    }
    let options = moqui_core::service::call::CallOptions {
        // the user's own organizations, whatever grants they hold on the service
        disable_authz: true,
        ..Default::default()
    };
    if let Err(err) = moqui_core::service::call::sync_checked(
        USER_ORGANIZATIONS_SERVICE,
        &std::collections::BTreeMap::new(),
        &options,
    ) {
        tracing::warn!(error = %err, "Could not set the user's organizations for a CalcGrid profile");
        ExecutionContext::with_current(|ec| ec.message().clear_errors());
    }
}

/// A profile entered ([`Profile::enter`]); dropping it leaves the profile.
pub struct Entered {
    _required: Option<moqui_core::context::AuthzRequired>,
}

impl Drop for Entered {
    fn drop(&mut self) {
        if let Some(Err(err)) = ExecutionContext::with_current(ExecutionContext::pop) {
            tracing::warn!(error = %err, "Leaving a CalcGrid profile");
        }
    }
}

/// Whether the caller authors documents (update on [`DOCUMENT_ARTIFACT`]), checked whatever the caller disabled.
#[must_use]
pub fn may_author() -> bool {
    let _required = ExecutionContext::with_current(ExecutionContext::require_authz);
    moqui_core::user::permitted_own("AT_SERVICE", DOCUMENT_ARTIFACT, "update").unwrap_or(false)
}
