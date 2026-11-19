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

use super::*;

/// SEC-CLI-84: a download with no name of its own takes its extension from the `Content-Type` only when that is a few
/// ASCII letters and digits, so a server's `\..\` (a separator on Windows) or any other character never reaches the
/// path; the server's own name keeps its safe characters as before.
#[test]
fn a_download_name_is_one_segment_whatever_the_content_type() {
    assert_eq!(download_name("", "text/csv; charset=utf-8"), "export.csv");
    assert_eq!(download_name("..", "application/pdf"), "export.pdf");
    let startup = r"x/\..\..\AppData\Roaming\Microsoft\Windows\Start Menu\Programs\Startup\a.cmd";
    assert_eq!(download_name("", startup), "export.bin");
    assert_eq!(download_name("_._", r"a/..\b"), "export.bin");
    assert_eq!(
        download_name("", "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet"),
        "export.bin"
    );
    assert_eq!(download_name("", "nothing"), "export.bin");
    assert_eq!(download_name(r"..\..\a.cmd", "text/plain"), ".._.._a.cmd");
    assert_eq!(download_name("Orders 2026.xlsx", ""), "Orders_2026.xlsx");
}
