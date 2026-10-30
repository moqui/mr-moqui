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

use std::path::PathBuf;

#[test]
fn email_services_resolve_rust_locations() {
    let service_dir =
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../runtime/base-component/framework/service/org/moqui");
    let paths = [
        service_dir.join("EmailServices.xml"),
        service_dir.join("impl/EmailServices.xml"),
    ];
    let (defs, failures) = super::definitions_in(&paths).unwrap();
    assert!(failures.is_empty(), "{failures:?}");
    // moqui-mail's, which core's tests do not link: each still loads, and a call reports its feature missing
    let expect = [
        ("send#Email", "moqui_mail::send_template", "smtp"),
        ("send#EmailTemplate", "moqui_mail::send_template", "smtp"),
        ("send#EmailMessage", "moqui_mail::send_message", "smtp"),
        ("poll#EmailServer", "moqui_mail::poll_server", "imap"),
    ];
    for (name, location, feature) in expect {
        let full = format!("org.moqui.impl.EmailServices.{name}");
        let def = defs.iter().find(|def| def.name == full).expect(name);
        assert_eq!(def.kind, super::ServiceKind::Rust, "{full}");
        assert_eq!(def.location, location, "{full}");
        assert!(def.rust_fn.is_none(), "{full}");
        assert_eq!(
            super::super::rust_services::missing_feature(location),
            Some(feature),
            "{full}"
        );
    }
    for name in ["save#EcaEmailMessage", "send#ScreenRenderEmail"] {
        let full = format!("org.moqui.impl.EmailServices.{name}");
        let def = defs.iter().find(|def| def.name == full).expect(name);
        assert_eq!(def.kind, super::ServiceKind::Inline, "{full}");
    }
    // A Groovy script location from a component written for Java Moqui is still not runnable.
    assert_eq!(
        super::kind_of("script", "component://x/Send.groovy"),
        super::ServiceKind::Unsupported
    );
}

/// A service file that does not parse, or an include that cannot be resolved, loses only its own services; the
/// other files still register.
#[test]
fn a_malformed_service_file_loses_only_its_own_services() {
    let broken = write_fixture("broken", r#"<services><service verb="bad" noun="File">"#);
    let good = write_fixture(
        "good-neighbor",
        r#"<services>
                <service verb="plain" noun="Neighbor"/>
                <service-include verb="lost" noun="Include" location="component://no-such-component/service/X.xml"/>
            </services>"#,
    );
    let (defs, failures) = super::definitions_in(&[broken.clone(), good.clone()]).unwrap();
    let _ = std::fs::remove_file(&broken);
    let _ = std::fs::remove_file(&good);
    let text = failures.join("\n");
    assert!(text.contains("could not be read"), "{text}");
    assert!(text.contains("could not be resolved"), "{text}");
    assert!(
        defs.iter().any(|def| def.name.ends_with("plain#Neighbor")),
        "{failures:?}"
    );
}

#[test]
fn rust_service_with_a_bad_location_stays_out_and_keeps_the_neighbor() {
    let ctx = crate::context::ExecutionContextFactory::test_context();
    let _entered = ctx.enter();
    let path = write_fixture(
        "rust",
        r#"<services>
                <service verb="missing" noun="Location" type="rust"/>
                <service verb="unknown" noun="Location" type="rust" location="moqui_core::service::contacts::no_such"/>
                <service verb="has" noun="Actions" type="rust" location="moqui_core::service::contacts::filter_contact_addresses">
                    <actions><set field="x" value="y"/></actions>
                </service>
                <service verb="plain" noun="Neighbor"/>
            </services>"#,
    );
    let failures = super::lint(&[path]).unwrap();
    let text = failures.join("\n");
    assert!(text.contains("missing#Location"), "{text}");
    assert!(text.contains("missing a location"), "{text}");
    assert!(text.contains("unknown#Location"), "{text}");
    assert!(text.contains("is not registered"), "{text}");
    assert!(text.contains("has#Actions"), "{text}");
    assert!(text.contains("has actions"), "{text}");
    assert!(super::is_registered("plain#Neighbor"), "{text}");
    assert!(!super::is_registered("missing#Location"));
    assert!(!super::is_registered("unknown#Location"));
    assert!(!super::is_registered("has#Actions"));
}

#[test]
fn groovy_leftover_fails_that_service_and_keeps_the_neighbor() {
    let ctx = crate::context::ExecutionContextFactory::test_context();
    let _entered = ctx.enter();
    let path = write_fixture(
        "groovy",
        r#"<services>
                <service verb="groovy" noun="Broken">
                    <actions><set field="x" from="a ?: b"/></actions>
                </service>
                <service verb="plain" noun="Neighbor">
                    <in-parameters><parameter name="ok" default-value="yes"/></in-parameters>
                </service>
            </services>"#,
    );
    let failures = super::lint(&[path]).unwrap();
    let text = failures.join("\n");
    assert!(text.contains("groovy#Broken"), "{text}");
    assert!(text.contains("?:"), "{text}");
    assert!(super::is_registered("plain#Neighbor"), "{text}");
    assert!(!super::is_registered("groovy#Broken"), "{text}");
}

#[test]
fn remote_json_rpc_is_not_registered() {
    let ctx = crate::context::ExecutionContextFactory::test_context();
    let _entered = ctx.enter();
    let path = write_fixture(
        "rpc",
        r#"<services>
                <service verb="remote" noun="Rpc" type="remote-json-rpc"/>
                <service verb="plain" noun="Neighbor"/>
            </services>"#,
    );
    let failures = super::lint(&[path]).unwrap();
    let text = failures.join("\n");
    assert!(text.contains("JSON-RPC"), "{text}");
    assert!(!super::is_registered("remote#Rpc"));
    assert!(super::is_registered("plain#Neighbor"));
}

#[test]
fn load_definitions_registers_email_services() {
    let ctx = crate::context::ExecutionContextFactory::test_context_in(
        "",
        crate::service::ServicePools::idle(),
        crate::context::framework_dir(),
    );
    let _entered = ctx.enter();
    super::load_definitions().unwrap();
    assert!(
        super::is_registered("org.moqui.EmailServices.send#EmailTemplate"),
        "{}",
        explain("Email")
    );
    assert!(
        super::is_registered("org.moqui.impl.EmailServices.poll#EmailServer"),
        "{}",
        explain("poll")
    );
}

fn explain(needle: &str) -> String {
    super::snapshot(&super::entered().unwrap())
        .failures
        .iter()
        .filter(|line| line.contains(needle))
        .take(12)
        .cloned()
        .collect::<Vec<_>>()
        .join("\n")
}

/// A framework directory whose `service/test/lazy` holds `files`, found by service name.
fn lazy_framework(name: &str, files: &[(&str, &str)]) -> PathBuf {
    let root = std::env::temp_dir().join(format!("moqui-svc-lazy-{name}-{}", std::process::id()));
    let dir = root.join("service/test/lazy");
    std::fs::create_dir_all(&dir).unwrap();
    for (file, body) in files {
        std::fs::write(dir.join(file), body).unwrap();
    }
    root
}

fn lazy_context(root: PathBuf) -> crate::context::ExecutionContext {
    crate::context::ExecutionContextFactory::test_context_in("", crate::service::ServicePools::idle(), root)
}

fn parsed_count(file: &str) -> usize {
    super::PARSED.with(|parsed| parsed.borrow().iter().filter(|path| path.ends_with(file)).count())
}

#[test]
fn a_file_found_by_name_is_read_alone_and_merged_with_the_files_before() {
    let root = lazy_framework(
        "merge",
        &[
            (
                "A.xml",
                r#"<services>
                        <service verb="run" noun="One">
                            <implements service="test.lazy.C.define#Shape"/>
                            <actions><set field="x" value="1"/></actions>
                        </service>
                        <service verb="run" noun="Base"><in-parameters><parameter name="fromBase"/></in-parameters></service>
                    </services>"#,
            ),
            (
                "B.xml",
                r#"<services>
                        <service verb="run" noun="Two">
                            <implements service="test.lazy.A.run#Base"/>
                            <actions><set field="y" value="2"/></actions>
                        </service>
                    </services>"#,
            ),
            (
                "C.xml",
                r#"<services>
                        <service verb="define" noun="Shape" type="interface">
                            <in-parameters><parameter name="fromShape"/></in-parameters>
                        </service>
                    </services>"#,
            ),
        ],
    );
    let ctx = lazy_context(root);
    let _entered = ctx.enter();
    super::PARSED.with(|parsed| parsed.borrow_mut().clear());

    // A's run#One implements a service of a file not read yet: that file is read on demand.
    assert!(super::find("test.lazy.A.run#Base").is_some());
    let two = super::find("test.lazy.B.run#Two").expect("B merged");
    assert!(
        two.in_params.iter().any(|param| param.name == "fromBase"),
        "implements across files"
    );
    assert_eq!(parsed_count("A.xml"), 1, "A is not read again when B is merged");
    assert_eq!(parsed_count("B.xml"), 1);

    assert!(super::find("test.lazy.C.define#Shape").is_some());
    let one = super::find("test.lazy.A.run#One").expect("resolved once C is read");
    assert!(one.in_params.iter().any(|param| param.name == "fromShape"));
    assert_eq!(parsed_count("C.xml"), 1, "C is read once, on demand");
    assert_eq!(parsed_count("A.xml"), 1);
    assert_eq!(parsed_count("B.xml"), 1);
    assert!(super::registration_failures("test.lazy.A.run#One").is_empty());
}

#[test]
fn two_threads_loading_different_files_both_register() {
    let files: Vec<(String, String)> = (0..4)
        .map(|index| {
            (
                format!("T{index}.xml"),
                format!(
                    r#"<services><service verb="run" noun="Some"><actions><set field="n" value="{index}"/></actions></service></services>"#
                ),
            )
        })
        .collect();
    let borrowed: Vec<(&str, &str)> = files
        .iter()
        .map(|(file, body)| (file.as_str(), body.as_str()))
        .collect();
    let ctx = lazy_context(lazy_framework("threads", &borrowed));
    let factory = std::sync::Arc::clone(ctx.factory());
    let start = std::sync::Arc::new(std::sync::Barrier::new(files.len()));
    let threads: Vec<_> = (0..files.len())
        .map(|index| {
            let factory = std::sync::Arc::clone(&factory);
            let start = std::sync::Arc::clone(&start);
            std::thread::spawn(move || {
                let ec = factory.new_context();
                let _entered = ec.enter();
                start.wait();
                super::find(&format!("test.lazy.T{index}.run#Some")).is_some()
            })
        })
        .collect();
    for thread in threads {
        assert!(thread.join().unwrap());
    }
    let _entered = ctx.enter();
    let registry = super::snapshot(&super::entered().unwrap());
    for index in 0..files.len() {
        assert!(
            registry.services.contains_key(&format!("test.lazy.T{index}.run#Some")),
            "T{index} kept"
        );
    }
}

/// A framework directory and two components, each with `service/test/ovr/Same.xml` holding `files`, with a context
/// whose components are those two, in that order.
fn override_context(name: &str, files: [&str; 3]) -> crate::context::ExecutionContext {
    let root = std::env::temp_dir().join(format!("moqui-svc-override-{name}-{}", std::process::id()));
    let mut components = Vec::new();
    for (index, body) in files.iter().enumerate() {
        let dir = root.join(["framework", "first", "second"][index]);
        std::fs::create_dir_all(dir.join("service/test/ovr")).unwrap();
        std::fs::write(dir.join("service/test/ovr/Same.xml"), body).unwrap();
        if index > 0 {
            components.push(crate::context::ComponentInfo {
                name: ["framework", "first", "second"][index].to_string(),
                version: String::new(),
                location: dir,
                depends_on: Vec::new(),
                purposes: Vec::new(),
            });
        }
    }
    crate::context::ExecutionContextFactory::test_context_with_components(
        "",
        crate::service::ServicePools::idle(),
        root.join("framework"),
        components,
    )
}

fn service_with(verb: &str, location: &str) -> String {
    format!(
        r#"<service verb="{verb}" noun="Thing"><actions><set field="from" value="{location}"/></actions></service>"#
    )
}

/// Java's `findServiceNode`: of the files with the same path, the framework's then each component's in order, the
/// last that has the service defines it, whichever file is read first.
#[test]
fn a_later_component_overrides_a_service_of_the_same_file_path() {
    let ctx = override_context(
        "lookup",
        [
            &format!(
                "<services>{}{}</services>",
                service_with("run", "framework"),
                service_with("keep", "framework")
            ),
            &format!("<services>{}</services>", service_with("run", "first")),
            &format!("<services>{}</services>", service_with("other", "second")),
        ],
    );
    let _entered = ctx.enter();
    let defined_in = |name: &str| super::find(name).map(|def| def.actions.clone()).unwrap_or_default();
    assert!(defined_in("test.ovr.Same.run#Thing").contains("first"));
    assert!(defined_in("test.ovr.Same.keep#Thing").contains("framework"));
    assert!(defined_in("test.ovr.Same.other#Thing").contains("second"));
    // read as a configured file, the framework's, the component's still wins
    let framework_file = ctx.factory().framework_dir().join("service/test/ovr/Same.xml");
    assert!(super::lint(&[framework_file]).unwrap().is_empty());
    assert!(defined_in("test.ovr.Same.run#Thing").contains("first"));
    // and every component's service files are known names, read or not
    let names = crate::service::info::known_service_names();
    assert!(
        names.iter().any(|name| name == "test.ovr.Same.other#Thing"),
        "{names:?}"
    );
}

/// As Java's `Integer.valueOf` and `Long.parseLong`, a numeric attribute that is not a number fails the service.
#[test]
fn a_timeout_that_is_not_a_number_fails_the_service() {
    let ctx = crate::context::ExecutionContextFactory::test_context();
    let _entered = ctx.enter();
    let path = write_fixture(
        "timeout",
        r#"<services>
                <service verb="slow" noun="Words" transaction-timeout="ten"/>
                <service verb="slow" noun="Sleep" semaphore="wait" semaphore-sleep="1s"/>
                <service verb="zero" noun="Timeout" transaction-timeout="0"/>
            </services>"#,
    );
    let failures = super::lint(&[path]).unwrap().join("\n");
    assert!(
        failures.contains("slow#Words transaction-timeout ten is not a whole number"),
        "{failures}"
    );
    assert!(
        failures.contains("slow#Sleep semaphore-sleep 1s is not a whole number"),
        "{failures}"
    );
    assert!(!super::is_registered("slow#Words"));
    assert!(super::find("zero#Timeout").is_some_and(|def| def.timeout.is_none()));
}

/// `moqui lint <file>`: a service implementing an interface of another file (the framework's EmailServices.xml)
/// resolves it by name as a server does, and only the file's own failures are reported, not those of the files
/// read for its interfaces; an interface that is nowhere is still a failure.
#[test]
fn check_of_one_file_resolves_interfaces_of_other_files() {
    let ctx = crate::context::ExecutionContextFactory::test_context_in(
        "",
        crate::service::ServicePools::idle(),
        crate::context::framework_dir(),
    );
    let _entered = ctx.enter();
    let path = write_fixture(
        "lint-interfaces",
        r#"<services>
                <service verb="process" noun="UnsubscribeEmail">
                    <implements service="org.moqui.EmailServices.process#EmailEca"/>
                    <actions><script>const subject = fields?.subject</script></actions>
                </service>
                <service verb="process" noun="Nothing">
                    <implements service="org.moqui.EmailServices.no#SuchInterface"/>
                </service>
            </services>"#,
    );
    let failures = super::check(std::slice::from_ref(&path)).unwrap();
    let _ = std::fs::remove_file(&path);
    assert_eq!(failures.len(), 1, "{failures:?}");
    assert!(
        failures[0].contains("process#Nothing implements unknown service org.moqui.EmailServices.no#SuchInterface"),
        "{failures:?}"
    );
}

fn write_fixture(name: &str, body: &str) -> PathBuf {
    let path = std::env::temp_dir().join(format!("moqui-svc-{name}-{}.xml", std::process::id()));
    std::fs::write(&path, body).unwrap();
    path
}
