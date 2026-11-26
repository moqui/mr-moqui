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

//! The Rust of @Title@. `submit_component!` puts this crate's implementations in the linked catalogue under the
//! component name `@component@`, the name in `component.xml`: a binary that links the crate (`./moq build` does) has
//! them, and uses them only when it loads the component. The service definitions pick them by location
//! (`service/@package@/@Class@Services.xml`).

use moqui_core::context::{Error, ExecutionContext};
use moqui_core::native::{submit_component, NativeComponent, NativeRegistry, Parameters, ServiceInfo};
use moqui_lib::error::Error as LibError;
use moqui_lib::object::Value;

submit_component! {
    NativeComponent { name: "@component@", register }
}

/// This component's implementations, in a factory's catalogue. A location is any name the service XML gives in
/// `location`; `moqui_core::service_location!(echo)` would give the function's Rust path instead.
fn register(registry: &mut NativeRegistry) -> Result<(), Error> {
    registry.service_function("@package@::echo", echo)
}

/// `@package@.@Class@Services.echo#Message`: the message back, with the user the call runs as. The service XML defines
/// the parameters, authentication and transaction; the parameters are already validated, and `ec` is the caller's.
///
/// # Errors
///
/// Returns an error when the session cannot be read.
pub fn echo(_ec: &ExecutionContext, _service: &ServiceInfo<'_>, parameters: &Parameters) -> Result<Parameters, LibError> {
    let mut out = Parameters::new();
    out.insert(
        "message".to_string(),
        parameters.get("message").cloned().unwrap_or(Value::Null),
    );
    out.insert(
        "username".to_string(),
        moqui_core::user::current_username()?.map_or(Value::Null, Value::String),
    );
    Ok(out)
}
