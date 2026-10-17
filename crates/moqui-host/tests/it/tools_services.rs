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

//! The tools services on the seeded embedded Postgres, called through the Service Run screen's `runJson` transition as
//! an admin: a print job is created and its status moves to a failed send (its print server, port 9, does not answer), a screen document reports that PDF is not
//! available, the clean services delete by a time basis, an entity data snapshot goes
//! out to a zip and back in, and the cache admin screen clears a cache.

#![cfg(all(unix, feature = "embedded-postgres", feature = "postgres", feature = "axum"))]

use std::net::SocketAddr;

use crate::common::{basic, exchange, http_request};

const DATA: &str = r#"<entity-facade-xml type="seed">
    <moqui.basic.print.NetworkPrinter networkPrinterId="GATE_PRINTER" serverHost="127.0.0.1" serverPort="9" printerName="gate"/>
    <moqui.basic.print.PrintJob printJobId="GATE_OLD_PJ" networkPrinterId="GATE_PRINTER" statusId="PtjNotSent"
        createdDate="2020-01-02 03:04:05"/>
    <moqui.basic.Uom uomId="GATE_A" description="Gate A" uomTypeEnumId="UT_LENGTH_MEASURE"/>
    <moqui.basic.Uom uomId="GATE_B" description="Gate B" uomTypeEnumId="UT_LENGTH_MEASURE"/>
    <moqui.basic.UomConversion uomConversionId="GATE_AB" uomId="GATE_A" toUomId="GATE_B" conversionFactor="2.5"
        conversionOffset="1" fromDate="2020-01-01 00:00:00"/>
</entity-facade-xml>"#;

fn encode(text: &str) -> String {
    text.bytes()
        .map(|byte| {
            if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.') {
                char::from(byte).to_string()
            } else {
                format!("%{byte:02X}")
            }
        })
        .collect()
}

/// Run a service through `ServiceRun/runJson`. Returns the status and the body.
fn run(address: SocketAddr, auth: &str, service: &str, pairs: &[(&str, &str)]) -> (u16, String) {
    let mut body = format!("serviceName={}", encode(service));
    for (key, value) in pairs {
        body.push_str(&format!("&{}={}", encode(key), encode(value)));
    }
    let request = http_request(
        "POST",
        "/Tools/Service/ServiceRun/runJson",
        &[
            ("Authorization", auth),
            ("Content-Type", "application/x-www-form-urlencoded"),
        ],
        &body,
    );
    let (status, _, text) = exchange(address, &request);
    (status, text)
}

fn page(address: SocketAddr, auth: &str, path: &str) -> String {
    let (status, _, text) = exchange(address, &http_request("GET", path, &[("Authorization", auth)], ""));
    assert_eq!(status, 200, "{path}: {text}");
    text
}

/// A browser for the PDF check: `pdf_chromium_path` when set, else `chromium`, `chromium-browser` or `google-chrome` on the
/// `PATH`. With none the screen document reports that PDF is not available, which also passes gate 4.
fn browser() -> Option<String> {
    let configured = moqui_lib::env::get_prop_or_env("pdf_chromium_path");
    if !configured.is_empty() {
        return Some(configured);
    }
    let path = std::env::var_os("PATH").unwrap_or_default();
    ["chromium", "chromium-browser", "google-chrome"]
        .iter()
        .find_map(|name| {
            std::env::split_paths(&path)
                .map(|dir| dir.join(name))
                .find(|candidate| candidate.is_file())
                .map(|found| found.display().to_string())
        })
}

#[test]
fn tools_services_run_on_postgres() {
    let browser = browser();
    let chromium: Vec<(&str, &str)> = browser
        .iter()
        .map(|binary| ("pdf_chromium_path", binary.as_str()))
        .collect();
    let gate = crate::common::start_with_properties("tools-services", Some(DATA), &chromium);
    let address = gate.server.address();
    let john = basic("john.doe", "moqui");

    // print#Document: the job is created not sent, then the send records that the print server did not answer. A
    // server that answers is tool_components.rs.
    let (_, printed) = run(
        address,
        &john,
        "org.moqui.impl.PrintServices.print#Document",
        &[
            ("networkPrinterId", "GATE_PRINTER"),
            ("inputStream", "hello printer"),
            ("contentType", "text/plain"),
            ("jobName", "Gate print"),
        ],
    );
    assert!(
        printed.contains("Could not reach print server at http://127.0.0.1:9"),
        "{printed}"
    );
    let jobs = page(
        address,
        &john,
        "/System/Print/PrintJob/PrintJobList.html?jobName=Gate%20print",
    );
    assert!(jobs.contains("Gate print") && jobs.contains("Send Failed"), "{jobs}");

    // A screen document asks for PDF: with no Chromium it is not available; with one, the PDF prints and the send
    // fails as any other.
    let (_, pdf) = run(
        address,
        &john,
        "org.moqui.impl.PrintServices.print#ScreenDocument",
        &[
            ("networkPrinterId", "GATE_PRINTER"),
            ("screenLocation", "component://tools/screen/System/dashboard.xml"),
        ],
    );
    if browser.is_none() {
        assert!(pdf.contains("PDF is not available"), "{pdf}");
    } else {
        // Gate 4: the html-print document came back as PDF bytes, and the print job holds them.
        assert!(pdf.contains("Could not reach print server"), "{pdf}");
        let factory = std::sync::Arc::clone(gate.server.factory());
        let context = factory.new_context();
        let _entered = context.enter();
        let _authz = context.disable_authz();
        let jobs = factory
            .entity()
            .unwrap()
            .find("moqui.basic.print.PrintJob")
            .unwrap()
            .eq("contentType", moqui_lib::object::Value::string("application/pdf"))
            .list()
            .unwrap();
        let document = jobs.values().iter().find_map(|job| match job.get("document") {
            Some(moqui_lib::object::Value::Bytes(bytes)) => Some(bytes.clone()),
            _ => None,
        });
        assert!(
            document.is_some_and(|bytes| bytes.starts_with(b"%PDF-")),
            "no PDF on a print job"
        );
    }

    // The clean services compare a time basis with the stamp columns.
    let (status, cleaned) = run(
        address,
        &john,
        "org.moqui.impl.ServerServices.clean#PrintJobData",
        &[("daysToKeep", "30")],
    );
    assert_eq!(status, 200, "{cleaned}");
    // The seed job clean_PrintJobData_daily may have removed the old job first, on the first scheduler pass.
    assert!(cleaned.contains("\"printJobsRemoved\""), "{cleaned}");
    let jobs = page(address, &john, "/System/Print/PrintJob/PrintJobList.html");
    assert!(!jobs.contains("GATE_OLD_PJ") && jobs.contains("Gate print"), "{jobs}");
    let (status, cleaned) = run(
        address,
        &john,
        "org.moqui.impl.ServerServices.clean#ArtifactData",
        &[("daysToKeep", "90")],
    );
    assert_eq!(status, 200, "{cleaned}");
    assert!(cleaned.contains("artifactHitsRemoved"), "{cleaned}");

    // Decimal arithmetic, both directions.
    let (_, converted) = run(
        address,
        &john,
        "org.moqui.impl.BasicServices.convert#Uom",
        &[("uomId", "GATE_A"), ("toUomId", "GATE_B"), ("amount", "4")],
    );
    assert!(converted.contains("\"convertedAmount\" : 11"), "{converted}");
    let (_, back) = run(
        address,
        &john,
        "org.moqui.impl.BasicServices.convert#Uom",
        &[("uomId", "GATE_B"), ("toUomId", "GATE_A"), ("amount", "11")],
    );
    assert!(back.contains("\"convertedAmount\" : 4"), "{back}");

    // An entity data snapshot out to a zip in runtime/db/snapshot, then back in.
    let (status, exported) = run(
        address,
        &john,
        "org.moqui.impl.EntityServices.export#EntityDataSnapshot",
        &[
            ("entitiesToInclude", "moqui.basic.Uom"),
            ("baseFilename", "GateSnapshot"),
        ],
    );
    assert_eq!(status, 200, "{exported}");
    assert!(
        exported.contains("\"recordsWritten\"") && !exported.contains("\"recordsWritten\" : 0"),
        "{exported}"
    );
    let snapshots = page(address, &john, "/Tools/Entity/DataSnapshot.html");
    assert!(snapshots.contains("GateSnapshot.zip"), "{snapshots}");
    let (status, imported) = run(
        address,
        &john,
        "org.moqui.impl.EntityServices.import#EntityDataSnapshot",
        &[("zipFilename", "GateSnapshot.zip")],
    );
    assert_eq!(status, 200, "{imported}");
    assert!(
        imported.contains("\"recordsLoaded\"") && !imported.contains("\"recordsLoaded\" : 0"),
        "{imported}"
    );

    // Cache admin: the conversion found above is in the entity list cache, and Clear All empties it.
    let size = |text: &str| -> String {
        let at = text
            .find(">entity.record.list.moqui.basic.UomConversion<")
            .unwrap_or_else(|| panic!("{text}"));
        let cells: Vec<&str> = text[at..].split("<span").skip(1).take(1).collect();
        cells
            .first()
            .and_then(|cell| cell.split('>').nth(1))
            .and_then(|cell| cell.split('<').next())
            .unwrap_or("")
            .to_string()
    };
    let caches = page(
        address,
        &john,
        "/System/Cache/CacheList.html?filterRegexp=UomConversion",
    );
    assert_ne!(size(&caches), "0", "{caches}");
    let request = http_request(
        "POST",
        "/System/Cache/CacheList/clearAllCaches",
        &[
            ("Authorization", &john),
            ("Content-Type", "application/x-www-form-urlencoded"),
        ],
        "",
    );
    let (status, _, cleared) = exchange(address, &request);
    assert!(status == 200 || status == 302, "{status}: {cleared}");
    let caches = page(
        address,
        &john,
        "/System/Cache/CacheList.html?filterRegexp=UomConversion",
    );
    assert_eq!(size(&caches), "0", "{caches}");
}
