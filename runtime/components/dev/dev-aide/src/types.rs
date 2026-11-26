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

//! The declarations of the TypeScript checker (plans/archive/design-dev-aide.md 5.4, question 10): `get#TypeDeclarations`
//! answers the `moqui types` files a server script is checked against (`moqui_devkit::typegen`: `moqui-host.d.ts`,
//! `moqui-globals.d.ts`, and the entities, services and tools of the loaded definitions), which the Workspace's worker
//! (`screen/devstatic/DevAideTsWorker.js`) runs the compiler over. The screens' declarations are left out: they type
//! client code and a screen's context, not a script file. Made at most once a minute, kept between.

use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

use moqui_core::context::{Error, ExecutionContext};
use moqui_core::native::{NativeRegistry, Parameters, ServiceInfo};
use moqui_lib::error::Error as LibError;
use moqui_lib::object::Value;
use moqui_lib::sync::lock;

use crate::tier::{self, Tier};
use crate::workspace::{json_value, sha256};

pub(crate) fn register(registry: &mut NativeRegistry) -> Result<(), Error> {
    registry.service_function(
        moqui_core::service_location!(get_type_declarations),
        get_type_declarations,
    )
}

/// The files of `moqui types` a script is checked against, in the order the compiler takes them.
pub(crate) const FILES: &[&str] = &[
    "moqui-host.d.ts",
    "moqui-globals.d.ts",
    "entities.d.ts",
    "services.d.ts",
    "tools.d.ts",
];

/// Declarations made this long ago are made again.
const KEEP: Duration = Duration::from_secs(60);

struct Made {
    at: Instant,
    files: Arc<Vec<(String, String)>>,
    hash: String,
}

fn made() -> &'static Mutex<Option<Made>> {
    static MADE: OnceLock<Mutex<Option<Made>>> = OnceLock::new();
    MADE.get_or_init(|| Mutex::new(None))
}

/// [`FILES`] of the declarations `files` holds, and their hash.
pub(crate) fn chosen(files: &moqui_devkit::typegen::TypeFiles) -> (Vec<(String, String)>, String) {
    let chosen: Vec<(String, String)> = FILES
        .iter()
        .filter_map(|name| files.get(name).map(|text| ((*name).to_string(), text.to_string())))
        .collect();
    let joined: String = chosen.iter().map(|(name, text)| format!("{name}\n{text}\n")).collect();
    let hash = sha256(joined.as_bytes());
    (chosen, hash)
}

/// `get#TypeDeclarations`: the files ({name, text}) and their hash; with `knownHash` the same as theirs, the hash only.
fn get_type_declarations(
    ec: &ExecutionContext,
    _service: &ServiceInfo<'_>,
    parameters: &Parameters,
) -> Result<Parameters, LibError> {
    tier::require(Tier::View)?;
    let (files, hash) = {
        let mut kept = lock(made());
        match kept.as_ref().filter(|made| made.at.elapsed() < KEEP) {
            Some(made) => (Arc::clone(&made.files), made.hash.clone()),
            None => {
                let factory = Arc::clone(ec.factory());
                let generated = moqui_devkit::typegen::generate(&factory, &[])
                    .map_err(|err| LibError::new(format!("the declarations were not made: {err}")))?;
                let (chosen, hash) = chosen(&generated);
                let files = Arc::new(chosen);
                *kept = Some(Made {
                    at: Instant::now(),
                    files: Arc::clone(&files),
                    hash: hash.clone(),
                });
                (files, hash)
            }
        }
    };
    let mut out = Parameters::new();
    out.insert("hash".into(), Value::string(&hash));
    let known = crate::workspace::text(parameters, "knownHash");
    if known != hash {
        let list: Vec<serde_json::Value> = files
            .iter()
            .map(|(name, text)| serde_json::json!({ "name": name, "text": text }))
            .collect();
        out.insert("files".into(), json_value(&list));
    }
    Ok(out)
}
