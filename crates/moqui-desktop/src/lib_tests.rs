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


/// SEC-CLI-123: Chromium's Safe Browsing list updates are off (the windows' links and downloads are checked where they
/// go), and its component updater, which keeps the revocation and Certificate Transparency lists, is not turned off.
#[test]
fn chromium_runs_without_safe_browsing_and_keeps_its_component_updates() {
    let settings = format!("{:?}", super::cef());
    assert!(settings.contains("(\"safebrowsing.enabled\", Bool(false))"), "{settings}");
    assert!(!settings.contains("disable-component-update"), "{settings}");
}
