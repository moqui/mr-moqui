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

//! `org.moqui.impl.ResourceServices`: the content blobs of `crate::resource::blob`. No Java counterpart. A failure is
//! an error in the message facade, as a Java service's is.

use std::collections::BTreeMap;

use moqui_lib::error::Error as LibError;
use moqui_lib::object::{is_empty, to_plain_string, Value};

use crate::context::ExecutionContext;
use crate::entity::EntityFacade;
use crate::resource::blob::BlobStore;

pub(in crate::service) fn register(
    map: &mut BTreeMap<&'static str, crate::service::rust_services::RustServiceFn>,
) -> Result<(), String> {
    rust_service!(map, gc_content_blobs)?;
    rust_service!(map, migrate_db_resource_blobs)?;
    Ok(())
}

/// `migrate#DbResourceBlobs`: move `dbresource` files over the blob threshold into content blobs. Out: `movedCount`,
/// `sizedCount`.
fn migrate_db_resource_blobs(
    parameters: &BTreeMap<String, Value>,
    facade: &EntityFacade,
) -> Result<BTreeMap<String, Value>, LibError> {
    let minutes = parameters
        .get("timeoutMinutes")
        .map(to_plain_string)
        .and_then(|text| text.trim().parse::<u64>().ok())
        .unwrap_or(60)
        .max(1);
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(minutes * 60);
    match facade.migrate_db_resource_blobs(deadline) {
        Ok((moved, sized)) => Ok([
            ("movedCount".to_string(), number(moved)),
            ("sizedCount".to_string(), number(sized)),
        ]
        .into_iter()
        .collect()),
        Err(err) => {
            let text = err.message().to_string();
            ExecutionContext::with_current(|ctx| ctx.message().add_error(text));
            Ok(BTreeMap::new())
        }
    }
}

fn number(value: u64) -> Value {
    Value::I64(i64::try_from(value).unwrap_or(i64::MAX))
}

/// `gc#ContentBlobs`: delete the blobs no row refers to, not written or referenced in the last `resource-facade` `blob-gc-prune-days`
/// days, and the chunks of abandoned uploads. Out: `deletedCount`, `deletedBytes`, `abandonedCount`.
fn gc_content_blobs(
    parameters: &BTreeMap<String, Value>,
    facade: &EntityFacade,
) -> Result<BTreeMap<String, Value>, LibError> {
    let dry_run = parameters
        .get("dryRun")
        .filter(|value| !is_empty(value))
        .is_some_and(|value| matches!(to_plain_string(value).as_str(), "true" | "Y" | "y"));
    let days = crate::resource::setting("blob-gc-prune-days")
        .parse::<i64>()
        .unwrap_or(14)
        .max(1);
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .ok()
        .and_then(|duration| i64::try_from(duration.as_millis()).ok())
        .unwrap_or(0);
    match BlobStore::new(facade.clone()).collect(now - days * 24 * 60 * 60 * 1000, dry_run) {
        Ok((deleted, bytes, abandoned)) => {
            tracing::info!("gc#ContentBlobs: deleted {deleted} blobs ({bytes} bytes), {abandoned} abandoned uploads");
            Ok([
                ("deletedCount".to_string(), number(deleted)),
                ("deletedBytes".to_string(), number(bytes)),
                ("abandonedCount".to_string(), number(abandoned)),
            ]
            .into_iter()
            .collect())
        }
        Err(err) => {
            let text = err.message().to_string();
            ExecutionContext::with_current(|ctx| ctx.message().add_error(text));
            Ok(BTreeMap::new())
        }
    }
}
