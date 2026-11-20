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

//! The manager link in a host built without the `manage` feature: the HTTPS listener takes no `moqui-manage/1`, the
//! gate starts no dialer, and `org.moqui.dev.InstanceLinkServices` answers that the link is not built. The same API as
//! `manage_link/` for `http.rs` and the operator gate.

/// What [`start`] gives the HTTPS listener: nothing.
#[derive(Default)]
pub(crate) struct Started {
    pub acceptor: Option<std::sync::Arc<accept::Acceptor>>,
}

/// Nothing to start; a server set to be managed is told this binary cannot be.
pub(crate) fn start(
    _factory: &std::sync::Arc<moqui_core::context::ExecutionContextFactory>,
    _shutdown: tokio::sync::watch::Receiver<bool>,
    _edge_node: bool,
) -> Started {
    if !moqui_core::context::conf_env::setting("manage@url").trim().is_empty() {
        tracing::error!("manage@url is set, but this binary has no manager link (the manage feature)");
    }
    Started::default()
}

pub(crate) mod accept {
    use std::net::SocketAddr;
    use std::sync::{Arc, Weak};

    use moqui_core::context::ExecutionContextFactory;
    use tokio::net::TcpStream;
    use tokio_rustls::server::TlsStream;

    /// Never made.
    pub(crate) enum Acceptor {}

    // never made: no acceptor exists
    #[allow(dead_code)]
    pub(crate) enum Secured {
        Http(TlsStream<TcpStream>),
        Link(TlsStream<TcpStream>),
    }

    impl Acceptor {
        pub(crate) async fn accept(
            &self,
            _stream: TcpStream,
            _http: &Arc<rustls::ServerConfig>,
            _factory: &Weak<ExecutionContextFactory>,
        ) -> std::io::Result<Secured> {
            match *self {}
        }

        pub(crate) async fn serve(&self, _stream: TlsStream<TcpStream>, _remote: SocketAddr) {
            match *self {}
        }
    }
}

pub(crate) mod dial {
    use std::path::Path;

    /// No dialer to start.
    pub(crate) fn start_for_gate(_runtime_dir: &Path, _conf: &moqui_lib::node::MNode) {}

    /// No bootstrap bundle to wait for.
    pub(crate) fn after_unseal(_runtime_dir: &Path, _conf: &moqui_lib::node::MNode) {}
}

pub(crate) mod services {
    use std::collections::BTreeMap;

    use moqui_core::entity::EntityFacade;
    use moqui_core::extension::RustServiceFn;
    use moqui_lib::error::Error as LibError;
    use moqui_lib::object::Value;

    moqui_core::extension::submit! {
        moqui_core::extension::Extension {
            register_services,
            ..moqui_core::extension::Extension::EMPTY
        }
    }

    fn register_services(map: &mut BTreeMap<&'static str, RustServiceFn>) -> Result<(), String> {
        moqui_core::rust_service!(map, get_instance_link)?;
        moqui_core::rust_service!(map, create_instance_pairing_code)?;
        moqui_core::rust_service!(map, call_instance_command)?;
        moqui_core::rust_service!(map, update_instance_approvers)?;
        moqui_core::rust_service!(map, revoke_instance_link)?;
        moqui_core::rust_service!(map, confirm_instance_link_address)?;
        moqui_core::rust_service!(map, pair_manager)?;
        moqui_core::rust_service!(map, get_manager_link)?;
        Ok(())
    }

    type Out = Result<BTreeMap<String, Value>, LibError>;

    fn not_built() -> Out {
        Err(LibError::new(
            "the manager link is not built into this host (the manage feature)",
        ))
    }

    fn get_instance_link(_params: &BTreeMap<String, Value>, _facade: &EntityFacade) -> Out {
        not_built()
    }
    fn create_instance_pairing_code(_params: &BTreeMap<String, Value>, _facade: &EntityFacade) -> Out {
        not_built()
    }
    fn call_instance_command(_params: &BTreeMap<String, Value>, _facade: &EntityFacade) -> Out {
        not_built()
    }
    fn update_instance_approvers(_params: &BTreeMap<String, Value>, _facade: &EntityFacade) -> Out {
        not_built()
    }
    fn revoke_instance_link(_params: &BTreeMap<String, Value>, _facade: &EntityFacade) -> Out {
        not_built()
    }
    fn confirm_instance_link_address(_params: &BTreeMap<String, Value>, _facade: &EntityFacade) -> Out {
        not_built()
    }
    fn pair_manager(_params: &BTreeMap<String, Value>, _facade: &EntityFacade) -> Out {
        not_built()
    }
    fn get_manager_link(_params: &BTreeMap<String, Value>, _facade: &EntityFacade) -> Out {
        not_built()
    }
}
