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

//! Stages the theseus Postgres archive the `embedded-postgres` feature embeds (`db::embedded`). It is the archive
//! `scripts/pg-payload.sh strip` keeps in `~/.theseus/postgresql` (downloaded once, with psql and readline taken out),
//! or the file `MOQUI_PG_ARCHIVE` names; nothing is downloaded here. An archive that still holds `bin/psql` or a
//! readline library is refused: readline is GPL (plans/archive/License-Review.md).
//!
//! postgresql_embedded's own `bundled` feature did this from a build script that compiled a second HTTP and TLS stack
//! (reqwest, rustls, aws-lc) for the build machine, about two minutes of CPU on every clean build.

use std::io::Read;
use std::path::PathBuf;

fn main() {
    println!("cargo:rerun-if-env-changed=MOQUI_PG_ARCHIVE");
    println!("cargo:rerun-if-env-changed=POSTGRESQL_VERSION");
    if std::env::var_os("CARGO_FEATURE_EMBEDDED_POSTGRES").is_none() {
        return;
    }
    // .cargo/config.toml forces it to the reviewed exact version, as "=18.6.0".
    let version = std::env::var("POSTGRESQL_VERSION").unwrap_or_default();
    let version = version.trim_start_matches('=');
    assert!(
        !version.is_empty() && version.chars().all(|c| c.is_ascii_digit() || c == '.'),
        "POSTGRESQL_VERSION must be an exact version like =18.6.0 (.cargo/config.toml), not {version:?}"
    );
    let target = std::env::var("TARGET").expect("cargo sets TARGET");
    let archive = std::env::var_os("MOQUI_PG_ARCHIVE").map_or_else(
        || {
            home_dir()
                .join(".theseus/postgresql")
                .join(format!("postgresql-{version}-{target}.tar.gz"))
        },
        PathBuf::from,
    );
    println!("cargo:rerun-if-changed={}", archive.display());
    let bytes = std::fs::read(&archive).unwrap_or_else(|err| {
        panic!(
            "embedded-postgres needs the Postgres {version} archive at {} ({err}). Run scripts/pg-payload.sh strip {target} \
             to download it and take out psql, or set MOQUI_PG_ARCHIVE; ./moq build runs it when the archive is missing",
            archive.display()
        )
    });
    if let Some(entry) = gpl_entry(&bytes) {
        panic!(
            "{} holds {entry} (readline, GPL); run scripts/pg-payload.sh strip {target} first",
            archive.display()
        );
    }
    let out = PathBuf::from(std::env::var_os("OUT_DIR").expect("cargo sets OUT_DIR")).join("postgresql.tar.gz");
    std::fs::write(&out, &bytes).unwrap_or_else(|err| panic!("could not write {}: {err}", out.display()));
    println!("cargo:rustc-env=MOQUI_PG_VERSION={version}");
}

/// The first entry of the archive that is psql or a readline library, the check of `pg-payload.sh check`.
fn gpl_entry(bytes: &[u8]) -> Option<String> {
    let mut archive = tar::Archive::new(flate2::read::GzDecoder::new(bytes));
    let entries = archive
        .entries()
        .unwrap_or_else(|err| panic!("the Postgres archive is not a tar.gz: {err}"));
    for entry in entries {
        let mut entry = entry.unwrap_or_else(|err| panic!("the Postgres archive is not a tar.gz: {err}"));
        let path = entry
            .path()
            .map(|path| path.to_string_lossy().into_owned())
            .unwrap_or_default();
        let name = path.rsplit('/').next().unwrap_or_default();
        if path.ends_with("bin/psql")
            || path.ends_with("bin/psql.exe")
            || name.starts_with("libreadline")
            || name.starts_with("libhistory")
        {
            return Some(path);
        }
        // Read through so a truncated archive is an error here, not at the first start.
        let _ = std::io::copy(&mut entry.by_ref(), &mut std::io::sink());
    }
    None
}

fn home_dir() -> PathBuf {
    std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .map_or_else(|| PathBuf::from("."), PathBuf::from)
}
