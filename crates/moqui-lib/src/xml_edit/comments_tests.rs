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

// shaped as MoquiDefaultConf.xml is
const CONF: &str = r#"<?xml version="1.0" encoding="UTF-8" ?>
<!-- No copyright or license for configuration file. -->
<moqui-conf>

    <!-- Settings in this file can be overridden in a component (MoquiConf.xml) and/or runtime conf file that has
        any of the desired sub-elements. -->

    <!-- Locale and Time Zone Properties -->
    <default-property name="default_locale" value="en_US"/>
    <default-property name="default_time_zone" value="UTC"/>
    <user-facade>
        <login-key expire-hours="144"/><!-- default expire 6 days, 144 hours -->
        <!--
            webapp_client_ip_header is how Moqui reads the client IP.

            nginx-proxy: X-Real-IP
        -->
        <login max-failures="3"/>
        <!-- trailing in user-facade -->
    </user-facade>
</moqui-conf>
"#;

fn at(found: &[ElementComments], line: usize) -> &ElementComments {
    found
        .iter()
        .find(|comments| comments.line == line)
        .unwrap_or_else(|| panic!("no comments at line {line}: {found:?}"))
}

#[test]
fn comments_attach_to_the_element_after_them_or_on_its_line() {
    let found = element_comments(CONF).unwrap();
    assert_eq!(
        at(&found, 3).before,
        ["No copyright or license for configuration file."]
    );
    assert_eq!(
        at(&found, 9).before,
        [
            "Settings in this file can be overridden in a component (MoquiConf.xml) and/or runtime conf file that has\nany of the desired sub-elements.",
            "Locale and Time Zone Properties"
        ]
    );
    // the next property has none of its own
    assert!(found.iter().all(|comments| comments.line != 10));
    assert_eq!(at(&found, 12).after, ["default expire 6 days, 144 hours"]);
    assert_eq!(
        at(&found, 18).before,
        ["webapp_client_ip_header is how Moqui reads the client IP.\n\nnginx-proxy: X-Real-IP"]
    );
    assert_eq!(at(&found, 11).trailing, ["trailing in user-facade"]);
}

#[test]
fn every_comment_of_the_default_conf_attaches_to_an_element() {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../runtime/components/base/framework/conf/MoquiDefaultConf.xml");
    let text = std::fs::read_to_string(path).unwrap();
    let found = element_comments(&text).unwrap();
    let attached: usize = found
        .iter()
        .map(|comments| comments.before.len() + comments.after.len() + comments.trailing.len())
        .sum();
    assert_eq!(attached, text.matches("<!--").count());
}
