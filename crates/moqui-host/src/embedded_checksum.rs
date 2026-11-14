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

//! The SHA-256 of each part built into the binary (`moqui embedded checksum`), and the check at startup against the
//! checksums a deployment expects: a file in `sha256sum` format, and the `MOQUI_EMBEDDED_SHA256_<PART>` environment
//! variables. A part that does not match refuses the start.
//!
//! The `runtime` part is over the packed index and blob as built (no unpacking, so it is cheap); `postgres`,
//! `opensearch` and `chromium` are over each archive's bytes, the `sha256sum` of the archive file the build embedded.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use moqui_lib::error::Error;
use sha2::{Digest, Sha256};

/// The setting naming the checksums file (a process property, the environment or `.env`).
pub(crate) const CHECKSUMS_SETTING: &str = "moqui_embedded_checksums";

/// The checksums file a runtime has when no setting names one.
pub(crate) const DEFAULT_FILE: &str = "embedded.sha256";

/// One part built into the binary that has a checksum of its own.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) enum Part {
    Runtime,
    Postgres,
    OpenSearch,
    Chromium,
}

impl Part {
    pub(crate) const ALL: [Part; 4] = [Part::Runtime, Part::Postgres, Part::OpenSearch, Part::Chromium];

    /// `runtime`, `postgres`, `opensearch` or `chromium`: its name in a checksums file.
    pub(crate) fn name(self) -> &'static str {
        match self {
            Self::Runtime => "runtime",
            Self::Postgres => "postgres",
            Self::OpenSearch => "opensearch",
            Self::Chromium => "chromium",
        }
    }

    pub(crate) fn named(name: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|part| part.name() == name)
    }

    /// `MOQUI_EMBEDDED_SHA256_RUNTIME` and the others.
    pub(crate) fn env_var(self) -> String {
        format!("MOQUI_EMBEDDED_SHA256_{}", self.name().to_ascii_uppercase())
    }

    /// The bytes of the part, in order; `None` when the binary does not have it.
    fn bytes(self) -> Option<Vec<&'static [u8]>> {
        match self {
            Self::Runtime => {
                let (index, blob) = moqui_embedded::packed();
                Some(vec![index, blob])
            }
            Self::Postgres => crate::db::embedded::archive().map(|(bytes, _)| vec![bytes]),
            Self::OpenSearch => moqui_embedded::programs::opensearch().map(|archive| vec![archive.bytes]),
            Self::Chromium => moqui_embedded::programs::chromium().map(|archive| vec![archive.bytes]),
        }
    }

    /// The part's SHA-256 in lowercase hex; `None` when the binary does not have it.
    pub(crate) fn sha256(self) -> Option<String> {
        self.bytes().map(|slices| {
            let mut hasher = Sha256::new();
            for slice in slices {
                hasher.update(slice);
            }
            moqui_lib::codec::hex_encode(&hasher.finalize())
        })
    }
}

/// The checksum a deployment expects of one part, and where it was given.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Expected {
    pub(crate) sha256: String,
    pub(crate) source: String,
}

/// The `sha256sum` lines of `text` that name a part: `<hex>  <name>` (or `<hex> *<name>`). Blank lines, `#` comments
/// and lines for other files (the gzipped binary beside them) are skipped, so the file also serves `sha256sum -c
/// --ignore-missing`.
///
/// # Errors
///
/// Returns an error naming the line when a line naming a part has no 64-digit hex checksum.
pub(crate) fn parse_sums(text: &str, source: &str) -> Result<BTreeMap<Part, Expected>, Error> {
    let mut expected = BTreeMap::new();
    for (number, line) in text.lines().enumerate() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let Some((hex, name)) = line.split_once(char::is_whitespace) else {
            continue;
        };
        let name = name.trim_start().trim_start_matches('*');
        let Some(part) = Part::named(name) else {
            continue;
        };
        let sha256 = checked_hex(hex).ok_or_else(|| {
            Error::new(format!(
                "{source} line {}: the checksum of {name} is not 64 hex digits",
                number + 1
            ))
        })?;
        expected.insert(
            part,
            Expected {
                sha256,
                source: source.to_string(),
            },
        );
    }
    Ok(expected)
}

/// `hex` in lowercase when it is a SHA-256 in hex.
fn checked_hex(hex: &str) -> Option<String> {
    (hex.len() == 64 && hex.bytes().all(|byte| byte.is_ascii_hexdigit())).then(|| hex.to_ascii_lowercase())
}

/// The checksums a deployment expects for the runtime `runtime_dir`: the file the [`CHECKSUMS_SETTING`] names (which
/// must be there), else `<runtime>/embedded.sha256` when it is there, then each `MOQUI_EMBEDDED_SHA256_<PART>` that
/// is set, in place of the file's line. `lookup` reads a setting or variable (empty is unset).
///
/// # Errors
///
/// Returns an error when the named file cannot be read, a line naming a part or a variable is not a SHA-256 in hex.
pub(crate) fn expected(runtime_dir: &Path, lookup: &dyn Fn(&str) -> String) -> Result<BTreeMap<Part, Expected>, Error> {
    let named = lookup(CHECKSUMS_SETTING);
    let file = if named.is_empty() {
        Some(runtime_dir.join(DEFAULT_FILE)).filter(|file| file.is_file())
    } else {
        Some(PathBuf::from(named))
    };
    let mut expected = match file {
        Some(file) => {
            let text = std::fs::read_to_string(&file).map_err(|err| {
                Error::new(format!("could not read the embedded checksums file {}", file.display())).with_source(err)
            })?;
            parse_sums(&text, &file.display().to_string())?
        }
        None => BTreeMap::new(),
    };
    for part in Part::ALL {
        let var = part.env_var();
        let value = lookup(&var);
        if value.is_empty() {
            continue;
        }
        let sha256 = checked_hex(value.trim())
            .ok_or_else(|| Error::new(format!("{var} is not a SHA-256 in hex (64 hex digits)")))?;
        expected.insert(part, Expected { sha256, source: var });
    }
    Ok(expected)
}

/// The parts of `expected` that do not match: each with the expected checksum, its source and what the binary has
/// (`None` when it does not have the part). `actual` gives a part's checksum ([`Part::sha256`]).
pub(crate) fn mismatches(
    expected: &BTreeMap<Part, Expected>,
    actual: &dyn Fn(Part) -> Option<String>,
) -> Vec<(Part, Expected, Option<String>)> {
    expected
        .iter()
        .filter_map(|(part, want)| {
            let have = actual(*part);
            (have.as_deref() != Some(want.sha256.as_str())).then(|| (*part, want.clone(), have))
        })
        .collect()
}

/// Check the parts built into this binary against the checksums the runtime `runtime_dir` expects ([`expected`]),
/// before it starts: nothing to do when none are given; an error line for each part that does not match and an error
/// that refuses the start; one line when all match.
///
/// # Errors
///
/// Returns an error when the expected checksums cannot be read, or a part does not match.
pub(crate) fn verify(runtime_dir: &Path) -> Result<(), Error> {
    let expected = expected(runtime_dir, &moqui_lib::env::get_prop_or_env)?;
    if expected.is_empty() {
        return Ok(());
    }
    let failed = mismatches(&expected, &Part::sha256);
    if failed.is_empty() {
        let parts: Vec<&str> = expected.keys().map(|part| part.name()).collect();
        tracing::info!(
            "The embedded {} match the expected checksums ({})",
            parts.join(", "),
            sources(&expected)
        );
        return Ok(());
    }
    let lines: Vec<String> = failed
        .iter()
        .map(|(part, want, have)| match have {
            Some(have) => format!(
                "the embedded {} does not match its expected checksum: {} expects {}, this binary has {have}",
                part.name(),
                want.source,
                want.sha256
            ),
            None => format!(
                "{} expects a checksum for {}, which this binary does not embed",
                want.source,
                part.name()
            ),
        })
        .collect();
    for line in &lines {
        tracing::error!("{line}");
    }
    // the lines go in the error too: the start may stop before the log has a console or a file
    Err(Error::new(format!(
        "this binary does not match the expected embedded checksums; not starting (`moqui embedded checksum` prints \
         what it has): {}",
        lines.join("; ")
    )))
}

/// The distinct sources of `expected`, in a list.
fn sources(expected: &BTreeMap<Part, Expected>) -> String {
    let mut sources: Vec<&str> = expected.values().map(|want| want.source.as_str()).collect();
    sources.sort_unstable();
    sources.dedup();
    sources.join(", ")
}

/// `moqui embedded checksum [--env] [part...]`: the checksum of each part named, or of every part the binary has, in
/// `sha256sum` format, or as environment variable assignments with `--env`.
///
/// # Errors
///
/// Returns an error for an unknown option or part, or a part named that the binary does not have.
pub(crate) fn print(args: &[String]) -> Result<(), Error> {
    let mut env = false;
    let mut parts = Vec::new();
    for arg in args {
        if arg == "--env" {
            env = true;
        } else if let Some(part) = Part::named(arg) {
            parts.push(part);
        } else {
            return Err(Error::new(format!(
                "{arg}: name a part as runtime, postgres, opensearch or chromium, or --env"
            )));
        }
    }
    let named = !parts.is_empty();
    if !named {
        parts = Part::ALL.to_vec();
    }
    for part in parts {
        match part.sha256() {
            Some(sha256) if env => println!("{}={sha256}", part.env_var()),
            Some(sha256) => println!("{sha256}  {}", part.name()),
            None if named => return Err(Error::new(format!("this binary does not embed {}", part.name()))),
            None => {}
        }
    }
    Ok(())
}

#[cfg(test)]
#[path = "embedded_checksum_tests.rs"]
mod tests;
