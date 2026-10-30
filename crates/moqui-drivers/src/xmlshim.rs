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

//! theseus 18.6.0 links `postgres` against `libxml2.so.2`. This host may only ship `libxml2.so.16`.
//! [`crate::embedded`] builds a forwarding `libxml2.so.2` into the install's `lib` before any server binary runs.
//! Every theseus binary searches that directory through its `$ORIGIN/../lib` runpath, so the process
//! environment is never changed: `set_var` with other threads running is undefined behaviour.

use std::path::{Path, PathBuf};

use moqui_lib::error::Error;

/// The host's `libxml2.so.16` when theseus needs the shim: the host has no `libxml2.so.2`.
#[cfg(all(target_os = "linux", target_arch = "x86_64"))]
pub fn needed() -> Option<PathBuf> {
    let cache = ldconfig_cache();
    if cache.lines().any(|line| ldconfig_soname(line) == "libxml2.so.2") {
        return None;
    }
    libxml2_16_path(&cache)
}

/// The shim is only built on x86-64 Linux.
#[cfg(not(all(target_os = "linux", target_arch = "x86_64")))]
pub fn needed() -> Option<PathBuf> {
    None
}

/// Build `libxml2.so.2` into `lib_dir`, forwarding to `host`, unless an earlier start already did.
///
/// # Errors
///
/// Returns an error when the shim cannot be built.
#[cfg(all(target_os = "linux", target_arch = "x86_64"))]
pub fn provide(lib_dir: &Path, host: &Path) -> Result<(), Error> {
    // Two clusters sharing one install may start at once. The library is built aside and renamed into
    // place, so a postmaster never maps a half-written file.
    static BUILD_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
    let _lock = moqui_lib::sync::lock(&BUILD_LOCK);
    let shared = lib_dir.join("libxml2.so.2");
    if shared.is_file() {
        return Ok(());
    }
    let build_dir = lib_dir.join(format!(".xmlshim-{}", std::process::id()));
    let built = compile_libxml2_shim(&build_dir, host).and_then(|built| {
        std::fs::rename(&built, &shared)
            .map_err(|err| Error::new("could not install the libxml2 shim").with_source(err))
    });
    let _ = std::fs::remove_dir_all(&build_dir);
    built
}

/// The shim is only built on x86-64 Linux.
///
/// # Errors
///
/// Never.
#[cfg(not(all(target_os = "linux", target_arch = "x86_64")))]
pub fn provide(_lib_dir: &Path, _host: &Path) -> Result<(), Error> {
    Ok(())
}

#[cfg(all(target_os = "linux", target_arch = "x86_64"))]
fn ldconfig_cache() -> String {
    std::process::Command::new("ldconfig")
        .arg("-p")
        .output()
        .ok()
        .filter(|output| output.status.success())
        .map(|output| String::from_utf8_lossy(&output.stdout).into_owned())
        .unwrap_or_default()
}

#[cfg(all(target_os = "linux", target_arch = "x86_64"))]
fn ldconfig_soname(line: &str) -> &str {
    line.split_whitespace().next().unwrap_or("")
}

#[cfg(all(target_os = "linux", target_arch = "x86_64"))]
fn libxml2_16_path(cache: &str) -> Option<PathBuf> {
    for line in cache.lines() {
        if ldconfig_soname(line) == "libxml2.so.16" && line.contains("x86-64") {
            let path = PathBuf::from(line.split("=>").nth(1)?.trim());
            if path.is_file() {
                return Some(path);
            }
        }
    }
    None
}

/// Assemble and link the shim in `shim_dir`. Returns the built library.
#[cfg(all(target_os = "linux", target_arch = "x86_64"))]
fn compile_libxml2_shim(shim_dir: &Path, host: &Path) -> Result<PathBuf, Error> {
    std::fs::create_dir_all(shim_dir)
        .map_err(|err| Error::new("could not create libxml2 shim dir").with_source(err))?;
    let assembly = shim_dir.join("shim.s");
    let map = shim_dir.join("ver.map");
    let shared = shim_dir.join("libxml2.so.2");
    std::fs::write(&assembly, shim_assembly())
        .map_err(|err| Error::new("could not write libxml2 shim").with_source(err))?;
    std::fs::write(&map, version_script())
        .map_err(|err| Error::new("could not write libxml2 shim").with_source(err))?;
    let output = std::process::Command::new("gcc")
        .args(["-shared", "-fPIC", "-o"])
        .arg(&shared)
        .arg(&assembly)
        .arg("-Wl,-soname,libxml2.so.2")
        .arg(format!("-Wl,--version-script={}", map.display()))
        .arg(host)
        .output()
        .map_err(|err| Error::new("gcc could not build the libxml2 shim").with_source(err))?;
    if !output.status.success() {
        return Err(Error::new(format!(
            "libxml2 shim: {}",
            String::from_utf8_lossy(&output.stderr)
        )));
    }
    Ok(shared)
}

#[cfg(all(target_os = "linux", target_arch = "x86_64"))]
fn shim_assembly() -> String {
    use std::fmt::Write as _;

    let mut out = String::new();
    out.push_str(".text\n");
    for (index, (name, version)) in XML_SYMBOLS.iter().enumerate() {
        writeln!(
            out,
            ".globl shim_{index}\n.type shim_{index}, @function\nshim_{index}:\n    jmp {name}@PLT\n.size shim_{index}, .-shim_{index}\n.symver shim_{index}, {name}@{version}"
        )
        .unwrap();
    }
    out
}

#[cfg(all(target_os = "linux", target_arch = "x86_64"))]
fn version_script() -> String {
    use std::fmt::Write as _;

    let mut out = String::new();
    for (index, version) in XML_VERSIONS.iter().enumerate() {
        out.push_str(version);
        out.push_str(" {\n  global:\n");
        for (name, symbol_version) in XML_SYMBOLS {
            if symbol_version == version {
                writeln!(out, "    {name};").unwrap();
            }
        }
        out.push_str("  local: *;\n}");
        if index == 0 {
            out.push_str(";\n");
        } else {
            writeln!(out, " {};", XML_VERSIONS[index - 1]).unwrap();
        }
    }
    out
}

#[cfg(all(target_os = "linux", target_arch = "x86_64"))]
const XML_VERSIONS: &[&str] = &[
    "LIBXML2_2.4.30",
    "LIBXML2_2.6.0",
    "LIBXML2_2.6.5",
    "LIBXML2_2.6.8",
    "LIBXML2_2.6.23",
];

#[cfg(all(target_os = "linux", target_arch = "x86_64"))]
const XML_SYMBOLS: &[(&str, &str)] = &[
    ("xmlAddChildList", "LIBXML2_2.4.30"),
    ("xmlBufferContent", "LIBXML2_2.4.30"),
    ("xmlBufferCreate", "LIBXML2_2.4.30"),
    ("xmlBufferFree", "LIBXML2_2.4.30"),
    ("xmlBufferLength", "LIBXML2_2.4.30"),
    ("xmlCharInRange", "LIBXML2_2.6.0"),
    ("xmlCheckVersion", "LIBXML2_2.4.30"),
    ("xmlCopyNode", "LIBXML2_2.4.30"),
    ("xmlCtxtReadDoc", "LIBXML2_2.6.0"),
    ("xmlCtxtReadMemory", "LIBXML2_2.6.0"),
    ("xmlDocSetRootElement", "LIBXML2_2.4.30"),
    ("xmlEncodeSpecialChars", "LIBXML2_2.4.30"),
    ("xmlFreeDoc", "LIBXML2_2.4.30"),
    ("xmlFreeNode", "LIBXML2_2.4.30"),
    ("xmlFreeParserCtxt", "LIBXML2_2.4.30"),
    ("xmlFreeTextWriter", "LIBXML2_2.6.0"),
    ("xmlGetExternalEntityLoader", "LIBXML2_2.4.30"),
    ("xmlGetUTF8Char", "LIBXML2_2.4.30"),
    ("xmlInitParser", "LIBXML2_2.4.30"),
    ("xmlKeepBlanksDefault", "LIBXML2_2.4.30"),
    ("xmlNewDoc", "LIBXML2_2.4.30"),
    ("xmlNewDocText", "LIBXML2_2.4.30"),
    ("xmlNewNode", "LIBXML2_2.4.30"),
    ("xmlNewParserCtxt", "LIBXML2_2.4.30"),
    ("xmlNewStringInputStream", "LIBXML2_2.4.30"),
    ("xmlNewTextWriterMemory", "LIBXML2_2.6.0"),
    ("xmlNodeDump", "LIBXML2_2.4.30"),
    ("xmlParseBalancedChunkMemory", "LIBXML2_2.4.30"),
    ("xmlParserPrintFileContext", "LIBXML2_2.4.30"),
    ("xmlSaveClose", "LIBXML2_2.6.8"),
    ("xmlSaveDoc", "LIBXML2_2.6.8"),
    ("xmlSaveToBuffer", "LIBXML2_2.6.23"),
    ("xmlSaveTree", "LIBXML2_2.6.8"),
    ("xmlSetExternalEntityLoader", "LIBXML2_2.4.30"),
    ("xmlSetGenericErrorFunc", "LIBXML2_2.4.30"),
    ("xmlSetStructuredErrorFunc", "LIBXML2_2.6.0"),
    ("xmlStrchr", "LIBXML2_2.4.30"),
    ("xmlStrcmp", "LIBXML2_2.4.30"),
    ("xmlStrdup", "LIBXML2_2.4.30"),
    ("xmlStrlen", "LIBXML2_2.4.30"),
    ("xmlStrncmp", "LIBXML2_2.4.30"),
    ("xmlStrstr", "LIBXML2_2.4.30"),
    ("xmlTextWriterEndElement", "LIBXML2_2.6.0"),
    ("xmlTextWriterStartElement", "LIBXML2_2.6.0"),
    ("xmlTextWriterWriteAttribute", "LIBXML2_2.6.0"),
    ("xmlTextWriterWriteBase64", "LIBXML2_2.6.0"),
    ("xmlTextWriterWriteBinHex", "LIBXML2_2.6.0"),
    ("xmlTextWriterWriteRaw", "LIBXML2_2.6.0"),
    ("xmlXPathCastBooleanToNumber", "LIBXML2_2.4.30"),
    ("xmlXPathCastBooleanToString", "LIBXML2_2.4.30"),
    ("xmlXPathCastNodeSetToString", "LIBXML2_2.4.30"),
    ("xmlXPathCastNodeToString", "LIBXML2_2.4.30"),
    ("xmlXPathCastNumberToString", "LIBXML2_2.4.30"),
    ("xmlXPathCompiledEval", "LIBXML2_2.4.30"),
    ("xmlXPathCtxtCompile", "LIBXML2_2.6.5"),
    ("xmlXPathFreeCompExpr", "LIBXML2_2.4.30"),
    ("xmlXPathFreeContext", "LIBXML2_2.4.30"),
    ("xmlXPathFreeObject", "LIBXML2_2.4.30"),
    ("xmlXPathNewContext", "LIBXML2_2.4.30"),
    ("xmlXPathRegisterNs", "LIBXML2_2.4.30"),
];
