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

fn url(text: &str) -> Url {
    Url::parse(text).unwrap()
}

/// SEC-CLI-07: the shell's page gets a nonce as its load finishes, and a command is the shell's only with it, from the
/// shell's page; a new load takes it away, and a document that is not the shell's page gets none.
#[test]
fn only_the_shells_current_load_has_its_nonce() {
    let load = ShellLoad::default();
    let shell = url("http://moqui-shell.localhost/?load=abc");
    assert!(!load.is_shell(Some(&shell), None), "nothing loaded yet");
    load.started(&shell);
    assert!(!load.is_shell(Some(&shell), None), "loading: no nonce yet");
    let nonce = load.finished(&shell).expect("the shell's page gets a nonce");
    assert!(load.is_shell(Some(&shell), Some(&nonce)));
    assert!(!load.is_shell(Some(&shell), None), "a command with no nonce");
    assert!(
        !load.is_shell(Some(&shell), Some("guessed")),
        "a command with another nonce"
    );
    // the same window, at another document: a data: or blob: page, a server path on the shell's origin
    for other in [
        "data:text/html,<script>1</script>",
        "blob:http://moqui-shell.localhost/0b3c",
        "http://moqui-shell.localhost/qapps/Chat",
        "http://moqui-app.localhost/",
    ] {
        assert!(
            !load.is_shell(Some(&url(other)), Some(&nonce)),
            "{other} is taken as the shell"
        );
    }
    assert!(!load.is_shell(None, Some(&nonce)), "a window with no URL");

    // a reload: the old nonce goes as the load begins, and a new one comes as it finishes
    load.started(&shell);
    assert!(!load.is_shell(Some(&shell), Some(&nonce)));
    let again = load.finished(&shell).expect("a nonce for the new load");
    assert_ne!(again, nonce);
    assert!(load.is_shell(Some(&shell), Some(&again)));
    // a load of another document: none until the shell's page loads again
    load.started(&url("http://moqui-app.localhost/qapps"));
    assert!(!load.is_shell(Some(&shell), Some(&again)));

    // another document's load gets no nonce, nor does a shell page whose load did not begin as the shell's
    load.started(&url("data:text/html,x"));
    assert_eq!(load.finished(&shell), None);
    load.started(&url("http://moqui-shell.localhost/qapps/Chat"));
    assert_eq!(load.finished(&url("http://moqui-shell.localhost/qapps/Chat")), None);
    assert!(is_shell_document(&url("http://moqui-shell.localhost/index.html")));
}

#[test]
fn the_nonce_is_handed_only_to_a_document_on_the_shells_origin() {
    let script = nonce_script("n\"1");
    assert!(script.starts_with("if (location.origin === \"http://moqui-shell.localhost\" && window.__moquiShellNonce)"));
    assert!(script.contains("window.__moquiShellNonce(\"n\\\"1\");"), "{script}");
}
