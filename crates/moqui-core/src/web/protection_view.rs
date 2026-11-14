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

//! The view of a server's web protection for the System app (plans/Edge-Node2.md §4.3): what each rule refused and the
//! clients banned now, and lifting a ban. Web protection is the host's, in memory per server; the host installs its
//! view on the factory when its listeners start, and `org.moqui.admin.OperationServices.get#WebProtection` and
//! `lift#WebBan` read it ([`view`]). A factory whose host started no listener has none.

use std::sync::{Arc, Mutex};

use crate::context::ExecutionContextFactory;

/// A server's web protection, as the host shows it.
pub trait ProtectionView: Send + Sync {
    /// `{"enabled", "counts": {name: n}, "rules": [{kind, name, hosts, detail, count}], "bans": [{address, seconds,
    /// source, node}]}`.
    fn report(&self) -> serde_json::Value;
    /// Lift every ban of `address` here, and tell the other members and the links. Ok(false) when there was none; Err
    /// for an address that is not one.
    fn lift(&self, address: &str) -> Result<bool, String>;
}

#[derive(Default)]
struct Slot(Mutex<Option<Arc<dyn ProtectionView>>>);

/// Show `view` as `factory`'s web protection.
pub fn install(factory: &ExecutionContextFactory, view: Arc<dyn ProtectionView>) {
    *moqui_lib::sync::lock(&factory.extension(|_| Slot::default()).0) = Some(view);
}

/// `factory`'s web protection, once its host's listeners started.
pub fn view(factory: &ExecutionContextFactory) -> Option<Arc<dyn ProtectionView>> {
    factory
        .extension_if_present::<Slot>()
        .and_then(|slot| moqui_lib::sync::lock(&slot.0).clone())
}
