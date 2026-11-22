//! An admitted meeting leg's grant to a particular open board. For a guest the
//! credential remains in memory on the serving node, never in a document or event.
use super::{call_internal, flag, text, Access, Refusal, Value};

#[derive(Clone)]
pub struct Meeting {
    pub call_id: String,
    pub leg_id: String,
    pub user_id: String,
    pub credential: Option<String>,
}

impl Meeting {
    /// No database user id contains NUL. A meeting grant cannot replace a wiki grant.
    pub fn key(&self) -> String {
        format!("\0meeting/{}/{}", self.call_id, self.leg_id)
    }

    pub(crate) fn grant(&self, page_id: &str) -> Result<(Access, String, String), Refusal> {
        let out = call_internal(
            "org.moqui.call.MeetingBoardServices.get#MeetingBoardGrant",
            &[
                ("wikiPageId", Value::string(page_id)),
                ("callSessionId", Value::string(&self.call_id)),
                ("callLegId", Value::string(&self.leg_id)),
                ("userId", Value::string(&self.user_id)),
                (
                    "credential",
                    self.credential.as_deref().map_or(Value::Null, Value::string),
                ),
            ],
        )?;
        Ok((
            Access {
                can_view: flag(&out, "canView"),
                can_edit: flag(&out, "canEdit"),
                on_database: text(&out, "storage") == "db",
            },
            text(&out, "writerUserId"),
            text(&out, "displayName"),
        ))
    }

    pub fn check(&self, key: &str, editing: bool) -> Result<bool, Refusal> {
        let (access, _, _) = self.grant(super::wiki_page_id(key)?)?;
        if !access.can_view || (editing && !access.can_edit) {
            return Err(Refusal::new(403, "this meeting does not grant access to that board"));
        }
        Ok(access.can_edit)
    }
}
