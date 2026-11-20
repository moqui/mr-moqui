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

//! The vault screens' policy by the screen a request resolves to (crates/moqui-core/src/web/vault_policy.rs): the
//! `/qapps` page and the screen's own answers at a vault screen get it, built on the site's policy, with the site's
//! `X-Frame-Options`, and the page is told it is a vault screen's; an extra-path name, a path parameter or another
//! screen does not.

use super::walk::Tree;
use super::{request, Env};
use crate::context::ExecutionContextFactory;

const POLICY: &str = "default-src 'self'; worker-src 'self' blob:; script-src 'self' 'unsafe-eval' 'wasm-unsafe-eval'";

fn site(tree: &Tree) -> String {
    let screen = |text: &str| format!(r#"<screen require-authentication="false" {text}</screen>"#);
    tree.write("Root.xml", &screen("><widgets><subscreens-active/></widgets>"));
    tree.write(
        "Root/qapps.xml",
        &screen(
            r#"allow-extra-path="true"><widgets><label text="vault document: ${ec.web.request.getAttribute('moqui.vault.document') == true}"/></widgets>"#,
        ),
    );
    tree.write("Root/apps.xml", &screen("><widgets><subscreens-active/></widgets>"));
    tree.write(
        "Root/apps/Commons.xml",
        &screen("><widgets><subscreens-active/></widgets>"),
    );
    tree.write(
        "Root/apps/Commons/Vault.xml",
        &screen(r#"><widgets><label text="the vault"/></widgets>"#),
    );
    tree.write(
        "Root/apps/Commons/Notes.xml",
        &screen(r#"><widgets><label text="notes"/></widgets>"#),
    );
    tree.write(
        "Root/apps/Commons/Knowledge.xml",
        &screen(r#"allow-extra-path="true"><widgets><label text="a page"/></widgets>"#),
    );
    format!(
        r#"<moqui-conf><webapp-list><webapp name="webroot"><root-screen host=".*" location="{}"/></webapp></webapp-list>
            <vault screen-names="Vault,Vaults" screen-policy="{POLICY}"/></moqui-conf>"#,
        tree.path("Root.xml")
    )
}

#[test]
fn a_vault_screen_and_its_qapps_page_get_the_vault_policy_and_an_extra_path_name_does_not() {
    let tree = Tree::new("vault-pages");
    let context = ExecutionContextFactory::test_context_with_conf(&site(&tree));
    let _env = Env::new();
    let factory = context.factory();
    let get = |path: &str| {
        let mut request = request("GET", path);
        request.set_header("Host", "erp.test");
        request.set_header("Accept", "text/html");
        crate::screen::serve(factory, request)
    };
    let policy = |path: &str| {
        let response = get(path);
        (
            response.status,
            response.body_text(),
            response
                .header("Content-Security-Policy")
                .unwrap_or_default()
                .to_string(),
            response.header("X-Frame-Options").map(str::to_string),
        )
    };

    let (status, body, csp, frame) = policy("/qapps/Commons/Vault");
    assert_eq!(status, 200, "{body}");
    assert!(body.contains("vault document: true"), "{body}");
    assert!(
        csp.contains("worker-src 'self' blob:") && csp.contains("default-src 'self'"),
        "{csp}"
    );
    // built on the site's screen-render policy, whose X-Frame-Options stays
    assert!(
        csp.contains("frame-ancestors 'none'") && csp.contains("form-action 'self'"),
        "{csp}"
    );
    assert_eq!(frame.as_deref(), Some("sameorigin"));
    let (status, body, csp, _) = policy("/apps/Commons/Vault");
    assert_eq!(status, 200, "{body}");
    assert!(body.contains("the vault") && csp.contains("worker-src"), "{csp}");

    // a page named Vault under a screen that takes an extra path is not a vault screen, nor is another screen
    for path in [
        "/qapps/Commons/Knowledge/Vault",
        "/apps/Commons/Knowledge/Vault",
        "/qapps/Commons/Notes",
    ] {
        let (status, body, csp, _) = policy(path);
        assert_eq!(status, 200, "{path}: {body}");
        assert!(
            !csp.contains("worker-src") && csp.contains("frame-ancestors"),
            "{path}: {csp}"
        );
        assert!(!body.contains("vault document: true"), "{path}: {body}");
    }
    // a path that finds the vault screen another way still gets its policy
    let (status, body, csp, _) = policy("/apps//Vault");
    if status == 200 && body.contains("the vault") {
        assert!(csp.contains("worker-src"), "{csp}");
    }
}
