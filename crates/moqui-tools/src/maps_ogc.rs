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

//! `org.moqui.maps.MapServices.read#OgcCapabilities` (plans/Maps.md §4.1, P4a): an OGC service's GetCapabilities text,
//! which the provider's test fetched (`MapOgc.js`), read by [`moqui_lib::geo::ogc`] (no DTD, no entity declaration) into
//! the JSON the provider records. In Rust because a large service's document (a few MB, thousands of layers) is more
//! than a script's memory holds as a tree. It sends and writes nothing, so no group is granted it: the test calls it.

use std::collections::BTreeMap;

use moqui_lib::error::Error as LibError;
use moqui_lib::geo::ogc::{read_capabilities, Capabilities, OgcService};
use moqui_lib::object::Value;

use moqui_core::entity::EntityFacade;

use crate::group_db::{text, Out};

/// The layers a document keeps when the caller names no limit
const DEFAULT_MAX_LAYERS: usize = 2000;

pub(crate) fn register(map: &mut BTreeMap<&'static str, moqui_core::extension::RustServiceFn>) -> Result<(), String> {
    use moqui_core::rust_service;
    rust_service!(map, read_ogc_capabilities)?;
    Ok(())
}

/// `service` (wms, wmts, wfs) and `text`: `{read, refused, capabilitiesJson}`; refused a short fixed text, never the
/// document's own
pub(crate) fn read_ogc_capabilities(
    parameters: &BTreeMap<String, Value>,
    _facade: &EntityFacade,
) -> Result<BTreeMap<String, Value>, LibError> {
    let mut out = Out::new();
    let refuse = |mut out: Out, why: &str| {
        out.insert("read".to_string(), Value::Bool(false));
        out.insert("refused".to_string(), Value::string(why));
        Ok(out)
    };
    let Some(service) = OgcService::parse(&text(parameters, "service")) else {
        return refuse(out, "a service that is not WMS, WMTS or WFS");
    };
    let max_layers = text(parameters, "maxLayers")
        .trim()
        .parse::<usize>()
        .ok()
        .filter(|n| *n > 0)
        .unwrap_or(DEFAULT_MAX_LAYERS);
    match read_capabilities(service, &text(parameters, "text"), max_layers) {
        Ok(Capabilities::Read(found)) => {
            out.insert("read".to_string(), Value::Bool(true));
            out.insert("capabilitiesJson".to_string(), Value::string(found.to_string()));
            Ok(out)
        }
        Ok(Capabilities::Refused(why)) => refuse(out, why),
        Err(_) => refuse(
            out,
            "something that is not XML this server reads (a DOCTYPE or an entity declaration is refused)",
        ),
    }
}
