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

//! The `dev` seal (plans/Sealed-Secrets.md §3.3): the root key in `seal.json` itself, so a development or test
//! instance starts with no operator, as it does today with its keys in `.env`. It protects nothing, and a production
//! instance refuses it (§3.8): the keeper checks the purpose before this method runs.

use serde_json::{Map, Value};

use super::{Method, MethodKind};
use crate::context::Error;
use crate::seal::{RootKey, SealConf};

/// The `dev` method.
pub struct Dev;

impl Method for Dev {
    fn kind(&self) -> MethodKind {
        MethodKind::Dev
    }

    fn wrap(&self, root: &RootKey, _conf: &SealConf) -> Result<Map<String, Value>, Error> {
        let mut metadata = Map::new();
        metadata.insert("key".to_string(), Value::String(crate::seal::encode(root.expose())));
        Ok(metadata)
    }

    fn unwrap(&self, metadata: &Map<String, Value>, _conf: &SealConf) -> Result<RootKey, Error> {
        RootKey::from_slice(&super::bytes_field(metadata, "key")?)
    }
}
