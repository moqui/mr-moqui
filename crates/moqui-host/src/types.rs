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

//! `moqui types [--out dir]`: TypeScript declarations from the loaded definitions (`plans/archive/TypeGeneration.md`). The
//! definitions load as they do for a server, against an entity model with no database, then
//! `moqui_devkit::typegen::generate` writes the files. The default output is `types/` in the runtime directory.

use std::path::{Path, PathBuf};

use moqui_devkit::typegen::TypeFiles;
use moqui_lib::error::Error;

use crate::qvt_report::{component_roots, model_only_factory, screen_files};

/// Generate the declarations for the framework, its components, and their screens.
///
/// # Errors
///
/// Returns an error when the factory, the entity model, or the service files do not load.
pub fn generate_types(runtime: &Path, framework: &Path) -> Result<TypeFiles, Error> {
    let factory = model_only_factory(runtime, framework)?;
    // `screen_files` takes the files below a directory named `screen`. `component_roots` includes the framework
    // directory only when it is not already inside the runtime component directories.
    let roots = component_roots(runtime, framework);
    moqui_devkit::typegen::generate(&factory, &screen_files(&roots)).map_err(|err| Error::new(err.to_string()))
}

/// Generate the declarations and write them into `out`. Returns the summary line the command prints.
///
/// # Errors
///
/// Returns an error when generation fails or a file cannot be written.
pub fn write_types(runtime: &Path, framework: &Path, out: &Path) -> Result<String, Error> {
    let files = generate_types(runtime, framework)?;
    files.write(out).map_err(|err| Error::new(err.to_string()))?;
    for note in &files.notes {
        tracing::info!("types: {}", note.lines().next().unwrap_or_default());
    }
    Ok(format!(
        "wrote {} files to {} ({} notes)",
        files.files().len(),
        out.display(),
        files.notes.len()
    ))
}

/// The output directory of `types [--out dir]`: `--out dir` or `--out=dir`, else `types/` in the runtime directory.
///
/// # Errors
///
/// Returns an error for any other argument.
pub fn parse_types_args(args: &[impl AsRef<str>], runtime: &Path) -> Result<PathBuf, Error> {
    let mut out = None;
    let mut index = 0;
    while index < args.len() {
        let arg = args[index].as_ref();
        if let Some(value) = arg.strip_prefix("--out=") {
            out = Some(PathBuf::from(value));
        } else if arg == "--out" {
            let value = args
                .get(index + 1)
                .ok_or_else(|| Error::new("types: --out needs a directory"))?;
            out = Some(PathBuf::from(value.as_ref()));
            index += 1;
        } else {
            return Err(Error::new(format!("types: unknown argument {arg}")));
        }
        index += 1;
    }
    Ok(out.unwrap_or_else(|| runtime.join("types")))
}

#[cfg(test)]
mod tests {
    use std::path::{Path, PathBuf};

    use super::parse_types_args;

    #[test]
    fn out_defaults_to_types_in_the_runtime_directory() {
        let runtime = Path::new("runtime");
        let none: [&str; 0] = [];
        assert_eq!(
            parse_types_args(&none, runtime).unwrap(),
            PathBuf::from("runtime/types")
        );
        assert_eq!(
            parse_types_args(&["--out", "x/y"], runtime).unwrap(),
            PathBuf::from("x/y")
        );
        assert_eq!(parse_types_args(&["--out=z"], runtime).unwrap(), PathBuf::from("z"));
        assert!(parse_types_args(&["--out"], runtime).is_err());
        assert!(parse_types_args(&["--bogus"], runtime).is_err());
    }
}
