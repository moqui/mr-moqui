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

//! The Gate 3 browser smoke's Chromium: the host's driver (`moqui_host::test_client::cdp`, which `moqui test-client`'s
//! browser uses) with each failure a panic, as a test wants. One headless Chromium with a temporary profile, a page
//! attached with a flat session (more in windows of their own, [`Browser::open_tab`]), `Runtime.evaluate`, and the
//! console errors, uncaught exceptions and log entries the pages report.

use std::path::PathBuf;
use std::time::Duration;

use moqui_host::test_client::cdp;
use serde_json::Value;

pub use cdp::Problem;

/// The browser of the smoke: the one `moqui test-client` finds (`moqui_chromium_path`, else `runtime/chromium`, else
/// an installed Chromium, Google Chrome or Microsoft Edge). A missing binary fails the smoke.
pub fn chromium_binary() -> PathBuf {
    match moqui_host::chromium::find_test_browser(Some(&super::repo_runtime()), "") {
        Ok(found) => found,
        Err(reason) => panic!("no Chromium for the Gate 3 browser smoke: {reason}"),
    }
}

pub struct Browser(cdp::Browser);

fn ok<T>(result: Result<T, String>) -> T {
    result.unwrap_or_else(|err| panic!("{err}"))
}

impl Browser {
    pub fn launch() -> Browser {
        super::share_install();
        let launch = cdp::Launch {
            binary: chromium_binary(),
            headed: false,
            window: (1280, 900),
            profile_prefix: "moqui-gate3-chromium".to_string(),
            extra_args: Vec::new(),
        };
        Browser(ok(cdp::Browser::launch(&launch)))
    }

    /// Open another page (a second tab of the same profile, so the same sign-in, in a window of its own) and make it the
    /// current one. The answer names it for [`Browser::use_tab`]; the first page is [`Browser::first_tab`].
    pub fn open_tab(&mut self) -> String {
        ok(self.0.open_tab())
    }

    /// The page [`Browser::launch`] opened.
    pub fn first_tab(&self) -> String {
        self.0.first_tab()
    }

    /// Make `tab` the page that the commands, the waits and the clicks go to.
    pub fn use_tab(&mut self, tab: &str) {
        self.0.use_tab(tab);
    }

    /// A command to the page.
    pub fn call(&mut self, method: &str, params: Value) -> Value {
        ok(self.0.call(method, params))
    }

    /// Read events for `duration`.
    pub fn pump(&mut self, duration: Duration) {
        ok(self.0.pump(duration));
    }

    /// Go to `url` and wait for its load event.
    pub fn navigate(&mut self, url: &str) {
        ok(self.0.navigate(url));
    }

    /// Load the current page again and wait for its load event.
    pub fn reload(&mut self) {
        ok(self.0.reload());
    }

    /// Wait for an event of the current page named `method` that came after the last [`Browser::navigate`] or
    /// [`Browser::clear_events`].
    pub fn wait_event(&mut self, method: &str, timeout: Duration) {
        ok(self.0.wait_event(method, timeout));
    }

    pub fn clear_events(&mut self) {
        self.0.clear_events();
    }

    /// The value of a JavaScript expression in the page, after its promise settles. An exception is an `Err` with its text.
    pub fn try_eval(&mut self, expression: &str) -> Result<Value, String> {
        self.0.try_eval(expression)
    }

    pub fn eval(&mut self, expression: &str) -> Value {
        ok(self.0.eval(expression))
    }

    /// Poll `expression` until it is truthy. The failure names `what` and the last value.
    pub fn wait_for(&mut self, what: &str, expression: &str, timeout: Duration) -> Value {
        ok(self.0.wait_for(what, expression, timeout))
    }

    /// A mouse click at the middle of the element `element` (an expression), after it is scrolled into view, as a user's click:
    /// Quasar opens menus and popups on pointer events that `element.click()` does not send.
    pub fn click(&mut self, what: &str, element: &str) {
        ok(self.0.click(what, element));
    }

    /// Type text into the focused element.
    pub fn type_text(&mut self, text: &str) {
        ok(self.0.type_text(text));
    }

    /// Press a key such as `Enter` or `Escape` on the focused element.
    pub fn press(&mut self, key: &str) {
        ok(self.0.press(key));
    }

    /// The problems reported since the last call, and forget them.
    pub fn take_problems(&mut self) -> Vec<Problem> {
        ok(self.0.take_problems())
    }
}
