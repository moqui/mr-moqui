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

//! The host's web protection asked again for the screen path a request reached (SEC-WEB-81). The host checks its path
//! rules and rate limits against the path as sent before a worker takes the request, but the screens reach a
//! transition or a screen by paths that form does not show: the FOP servlet's prefix (`/fop/Login/login`), a `//name`
//! search path, a `ScreenPathAlias` row, the extra path after a transition (`/r/<token>/submit/x`) and a render-mode
//! extension on its name (`/rest/login.json`), and a token route, whose screen is the render's root (`/r/<token>`). Once the screen walk has resolved the path, the request's render asks
//! the check the host installed for this request ([`with_reached_check`]) with the path it reached
//! (`screen::render`), and answers its refusal before any action runs.

use std::cell::RefCell;

/// The host's refusal of a reached path: the status (404 for a path rule, 429 for a rate limit) and, for a rate limit,
/// the seconds a client should wait.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PathRefusal {
    pub status: u16,
    pub retry_after: Option<u64>,
}

/// The host's check of the paths a request reached (its screen or transition, and that with the extra path after a
/// transition): `None` lets it go on.
pub type ReachedCheck = Box<dyn Fn(&[String]) -> Option<PathRefusal> + Send>;

thread_local! {
    static REACHED_CHECK: RefCell<Option<ReachedCheck>> = const { RefCell::new(None) };
}

/// Run `body` (a request's job) with `check` asked for the screen path the request reaches; it is gone afterwards,
/// whatever `body` did.
pub fn with_reached_check<R>(check: ReachedCheck, body: impl FnOnce() -> R) -> R {
    struct Clear;
    impl Drop for Clear {
        fn drop(&mut self) {
            REACHED_CHECK.with(|slot| slot.borrow_mut().take());
        }
    }
    REACHED_CHECK.with(|slot| *slot.borrow_mut() = Some(check));
    let _clear = Clear;
    body()
}

/// The host's answer for `reached`, the screen paths a request resolved to; `None` with no check installed (a render
/// that is no request of the host's, a test).
pub(crate) fn refused(reached: &[String]) -> Option<PathRefusal> {
    REACHED_CHECK.with(|slot| slot.borrow().as_ref().and_then(|check| check(reached)))
}
