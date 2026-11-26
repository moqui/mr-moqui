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

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::Arc;

use super::{with, OverlayFile, ServiceOverlay};

/// A framework directory whose `service/test/over` holds `files`, its services found by name.
fn framework(name: &str, files: &[(&str, &str)]) -> PathBuf {
    let root = std::env::temp_dir().join(format!("moqui-svc-overlay-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    let dir = root.join("service/test/over");
    std::fs::create_dir_all(&dir).unwrap();
    for (file, body) in files {
        std::fs::write(dir.join(file), body).unwrap();
    }
    root
}

fn context(root: PathBuf) -> crate::context::ExecutionContext {
    let context =
        crate::context::ExecutionContextFactory::test_context_in("", crate::service::ServicePools::idle(), root);
    context.factory().set_entity(crate::testing::open());
    context
}

/// The out-parameter `name` of a call of `service` with no parameters.
fn out(service: &str, name: &str) -> String {
    let result = crate::service::sync(service, &BTreeMap::new()).unwrap_or_else(|err| panic!("{service}: {err}"));
    result
        .get(name)
        .map(moqui_lib::object::to_plain_string)
        .unwrap_or_default()
}

fn things(word: &str, more: &str) -> String {
    format!(
        r#"<services>
            <service authenticate="false" verb="get" noun="Greeting">
                <out-parameters><parameter name="text"/></out-parameters>
                <actions>
                    <service-call name="test.over.Things.make#Word" out-map="made"/>
                    <set field="text" from="'hello ' + made.word"/>
                </actions>
            </service>
            <service authenticate="false" verb="make" noun="Word">
                <out-parameters><parameter name="word"/></out-parameters>
                <actions><set field="word" value="{word}"/></actions>
            </service>
            {more}
        </services>"#
    )
}

/// F14 for services: a buffer's service file is seen by the calls on the thread that holds it, the service a call
/// reaches through another of the file included, a service it defines anew too, and one it no longer defines is gone
/// there; another thread at the same time, and this one after, see the registry, and nothing is written.
#[test]
fn a_service_files_buffer_stands_for_the_calls_on_its_thread_only() {
    let root = framework("buffer", &[("Things.xml", &things("disk", ""))]);
    let file = root.join("service/test/over/Things.xml");
    let ctx = context(root);
    let _entered = ctx.enter();
    assert_eq!(out("test.over.Things.get#Greeting", "text"), "hello disk");

    let buffer = things(
        "buffer",
        r#"<service authenticate="false" verb="count" noun="Things"><out-parameters><parameter name="count"/></out-parameters>
            <actions><set field="count" from="3"/></actions></service>"#,
    );
    let overlay = ServiceOverlay::build(&[OverlayFile {
        location: "component://test/service/test/over/Things.xml".into(),
        path: file.clone(),
        text: buffer,
    }])
    .unwrap();
    assert!(overlay.defines("test.over.Things.count#Things"), "{overlay:?}");
    assert!(overlay.failures().is_empty(), "{:?}", overlay.failures());
    let factory = Arc::clone(ctx.factory());
    with(Arc::new(overlay), || {
        assert_eq!(out("test.over.Things.get#Greeting", "text"), "hello buffer");
        assert_eq!(out("test.over.Things.count#Things", "count"), "3");
        // another thread at the same time has the registry
        let elsewhere = std::thread::scope(|scope| {
            scope
                .spawn(|| {
                    let other = factory.new_context();
                    let _entered = other.enter();
                    (
                        out("test.over.Things.get#Greeting", "text"),
                        crate::service::define::find("test.over.Things.count#Things").is_some(),
                    )
                })
                .join()
                .unwrap()
        });
        assert_eq!(elsewhere, ("hello disk".to_string(), false));
    });
    assert_eq!(out("test.over.Things.get#Greeting", "text"), "hello disk");
    assert!(crate::service::define::find("test.over.Things.count#Things").is_none());
    assert_eq!(std::fs::read_to_string(&file).unwrap(), things("disk", ""));

    // a buffer that no longer defines a service takes it away, on its thread
    let without = ServiceOverlay::build(&[OverlayFile {
        location: "component://test/service/test/over/Things.xml".into(),
        path: file,
        text: r#"<services><service authenticate="false" verb="make" noun="Word"><actions/></service></services>"#
            .into(),
    }])
    .unwrap();
    with(Arc::new(without), || {
        assert!(crate::service::define::find("test.over.Things.get#Greeting").is_none());
        assert!(crate::service::define::find("test.over.Things.make#Word").is_some());
    });
    assert!(crate::service::define::find("test.over.Things.get#Greeting").is_some());
}

/// A new service file of a buffer (not on the disk yet) defines its services, and a script service runs its script
/// file's buffer text.
#[test]
fn a_new_file_and_a_script_files_text_stand_at_their_locations() {
    let root = framework("script", &[]);
    let script = root.join("service/test/over/run-it.js");
    std::fs::write(&script, "said = 'from the disk'").unwrap();
    let services = format!(
        r#"<services><service authenticate="false" verb="run" noun="It" type="script" location="{}">
            <out-parameters><parameter name="said"/></out-parameters></service></services>"#,
        script.display()
    );
    let file = root.join("service/test/over/Fresh.xml");
    let ctx = context(root);
    let _entered = ctx.enter();
    assert!(crate::service::define::find("test.over.Fresh.run#It").is_none());
    let overlay = ServiceOverlay::build(&[
        OverlayFile {
            location: "component://test/service/test/over/Fresh.xml".into(),
            path: file.clone(),
            text: services,
        },
        OverlayFile {
            location: script.display().to_string(),
            path: script.clone(),
            text: "said = 'from the buffer'".into(),
        },
    ])
    .unwrap();
    assert_eq!(overlay.script_locations(), [script.display().to_string()]);
    let overlay = Arc::new(overlay);
    with(Arc::clone(&overlay), || {
        assert_eq!(out("test.over.Fresh.run#It", "said"), "from the buffer");
    });
    assert!(!file.exists() && crate::service::define::find("test.over.Fresh.run#It").is_none());
    // the script's text kept for the run is the disk's, not the buffer's
    let disk_only = ServiceOverlay::build(&[OverlayFile {
        location: "component://test/service/test/over/Fresh.xml".into(),
        path: file,
        text: format!(
            r#"<services><service authenticate="false" verb="run" noun="It" type="script" location="{}">
                <out-parameters><parameter name="said"/></out-parameters></service></services>"#,
            script.display()
        ),
    }])
    .unwrap();
    with(Arc::new(disk_only), || {
        assert_eq!(out("test.over.Fresh.run#It", "said"), "from the disk");
    });
}

/// A buffer may not define a Rust service anew or point one at another function: that would run the server's code by
/// a name, and so an authorization, the text chose.
#[test]
fn a_rust_service_the_text_defines_anew_is_refused() {
    let root = framework("rust", &[]);
    let ctx = context(root.clone());
    let _entered = ctx.enter();
    let err = ServiceOverlay::build(&[OverlayFile {
        location: "component://test/service/test/over/Native.xml".into(),
        path: root.join("service/test/over/Native.xml"),
        text: r#"<services><service verb="run" noun="Native" type="rust" location="moqui_core::service::sso::login_user"/></services>"#
            .into(),
    }])
    .unwrap_err();
    assert!(err.message().contains("Rust service"), "{err:?}");
}

#[test]
fn a_service_file_that_does_not_parse_is_refused_naming_it() {
    let root = framework("bad", &[]);
    let ctx = context(root.clone());
    let _entered = ctx.enter();
    let err = ServiceOverlay::build(&[OverlayFile {
        location: "component://test/service/test/over/Bad.xml".into(),
        path: root.join("service/test/over/Bad.xml"),
        text: "<services><service verb=\"run\"".into(),
    }])
    .unwrap_err();
    assert!(
        err.message()
            .contains("component://test/service/test/over/Bad.xml does not parse"),
        "{err:?}"
    );
}
