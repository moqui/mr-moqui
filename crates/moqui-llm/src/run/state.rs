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

//! The states of a run, which are A2A's task states, and the transitions between them (plan 4.4). The same table is
//! the `StatusFlowTransition` rows of `data/AgentTypeData.xml`, which the entity-auto updates check; a test compares
//! the two.

/// A run's state. Its `StatusItem` is [`RunState::status_id`], whose `statusCode` is the A2A wire value.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum RunState {
    Submitted,
    Working,
    InputRequired,
    AuthRequired,
    Completed,
    Failed,
    Canceled,
    Rejected,
}

impl RunState {
    pub const ALL: [RunState; 8] = [
        RunState::Submitted,
        RunState::Working,
        RunState::InputRequired,
        RunState::AuthRequired,
        RunState::Completed,
        RunState::Failed,
        RunState::Canceled,
        RunState::Rejected,
    ];

    pub fn status_id(self) -> &'static str {
        match self {
            RunState::Submitted => "AgrSubmitted",
            RunState::Working => "AgrWorking",
            RunState::InputRequired => "AgrInputRequired",
            RunState::AuthRequired => "AgrAuthRequired",
            RunState::Completed => "AgrCompleted",
            RunState::Failed => "AgrFailed",
            RunState::Canceled => "AgrCanceled",
            RunState::Rejected => "AgrRejected",
        }
    }

    /// The A2A 1.0 `TaskState`.
    pub fn wire(self) -> &'static str {
        match self {
            RunState::Submitted => "TASK_STATE_SUBMITTED",
            RunState::Working => "TASK_STATE_WORKING",
            RunState::InputRequired => "TASK_STATE_INPUT_REQUIRED",
            RunState::AuthRequired => "TASK_STATE_AUTH_REQUIRED",
            RunState::Completed => "TASK_STATE_COMPLETED",
            RunState::Failed => "TASK_STATE_FAILED",
            RunState::Canceled => "TASK_STATE_CANCELED",
            RunState::Rejected => "TASK_STATE_REJECTED",
        }
    }

    pub fn from_status_id(status_id: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|state| state.status_id() == status_id)
    }

    pub fn from_wire(wire: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|state| state.wire() == wire)
    }

    /// No transition leaves it.
    pub fn is_terminal(self) -> bool {
        matches!(
            self,
            RunState::Completed | RunState::Failed | RunState::Canceled | RunState::Rejected
        )
    }

    /// Paused for input or authorization: the run holds no worker and no lease.
    pub fn is_paused(self) -> bool {
        matches!(self, RunState::InputRequired | RunState::AuthRequired)
    }

    pub fn can_move_to(self, to: RunState) -> bool {
        TRANSITIONS.contains(&(self, to))
    }

    /// `to`, when the table allows it.
    ///
    /// # Errors
    ///
    /// Returns an error naming both states when it does not.
    pub fn move_to(self, to: RunState) -> Result<RunState, moqui_lib::error::Error> {
        if self.can_move_to(to) {
            Ok(to)
        } else {
            Err(moqui_lib::error::Error::new(format!(
                "a run cannot go from {self:?} to {to:?}"
            )))
        }
    }
}

/// Every allowed transition. Rejected is for quota, permission or invalid input; Failed from a pause is an expired pause.
pub const TRANSITIONS: &[(RunState, RunState)] = &[
    (RunState::Submitted, RunState::Working),
    (RunState::Submitted, RunState::Canceled),
    (RunState::Submitted, RunState::Rejected),
    (RunState::Submitted, RunState::Failed),
    (RunState::Working, RunState::Completed),
    (RunState::Working, RunState::Failed),
    (RunState::Working, RunState::Canceled),
    (RunState::Working, RunState::InputRequired),
    (RunState::Working, RunState::AuthRequired),
    (RunState::InputRequired, RunState::Working),
    (RunState::InputRequired, RunState::Canceled),
    (RunState::InputRequired, RunState::Failed),
    (RunState::AuthRequired, RunState::Working),
    (RunState::AuthRequired, RunState::Canceled),
    (RunState::AuthRequired, RunState::Failed),
];
