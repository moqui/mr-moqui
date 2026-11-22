//! An admitted meeting leg's grant to a particular open board. For a guest the
//! credential remains in memory on the serving node, never in a document or event.
//! The grant is asked at the join and again by the tick (`recheck_access`); the frames between are answered from the
//! room's copy ([`super::meeting_allows`]).
use super::{call_internal, flag, text, Access, Refusal, Value};

/// What [`Meeting::grant`] answers in a test in place of the call's: `Some` answers that (view, edit), `None` asks the
/// service; and how many times it was asked.
#[cfg(test)]
pub(crate) static MEETING_GRANT_FOR_TEST: std::sync::Mutex<Option<Result<(bool, bool), u16>>> =
    std::sync::Mutex::new(None);
#[cfg(test)]
pub(crate) static MEETING_GRANTS_ASKED: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);

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
        #[cfg(test)]
        {
            MEETING_GRANTS_ASKED.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            if let Some(answer) = *moqui_lib::sync::lock(&MEETING_GRANT_FOR_TEST) {
                return match answer {
                    Ok((can_view, can_edit)) => Ok((
                        Access {
                            can_view,
                            can_edit,
                            on_database: true,
                        },
                        self.user_id.clone(),
                        format!("{} (meeting)", self.user_id),
                    )),
                    Err(code) => Err(Refusal::new(code, "the meeting grant check failed")),
                };
            }
        }
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
}
