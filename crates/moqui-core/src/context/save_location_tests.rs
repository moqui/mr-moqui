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

use super::{report_save_location_problem, DEFAULT_REPORT_SAVE_ROOTS};

/// SEC-TLS-06: a report file is saved only under a configured root, never by `..`, in a component, the conf or a
/// `.env` file.
#[test]
fn a_report_is_saved_only_under_a_save_root() {
    let roots = DEFAULT_REPORT_SAVE_ROOTS;
    assert_eq!(report_save_location_problem("dbresource://report/daily.csv", roots), None);
    assert_eq!(report_save_location_problem("dbresource://report/team/daily.csv", roots), None);
    for refused in [
        "",
        "dbresource://report",
        "dbresource://reports/x.csv",
        "dbresource://report/../conf/x.csv",
        "dbresource://report/%2e%2e/x.csv",
        "file:/etc/passwd",
        "runtime://conf/MoquiProductionConf.xml",
        "component://mantle/data/Seed.xml",
        "file:/opt/moqui/runtime/.env",
    ] {
        assert!(report_save_location_problem(refused, roots).is_some(), "{refused}");
    }
    let roots = "file:/srv/reports, dbresource://report/";
    assert_eq!(report_save_location_problem("file:/srv/reports/a.pdf", roots), None);
    for refused in [
        "file:/srv/reports-other/a.pdf",
        "file:/srv/reports/../moqui/runtime/.env",
        "file:/srv/reports/conf/a.xml",
        "file:/srv/reports/prod.env",
        "file:/srv/reports\\..\\a.pdf",
    ] {
        assert!(report_save_location_problem(refused, roots).is_some(), "{refused}");
    }
}
