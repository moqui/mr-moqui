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

//! `type="rust"` services bound in the catalogue. `service_location!` names each function by its Rust path, which is
//! the `location` in `service/example/ExampleServices.xml`: `example::services::echo`.

use moqui_core::context::{Error, ExecutionContext};
use moqui_core::native::{NativeRegistry, Parameters, ServiceInfo};
use moqui_lib::error::Error as LibError;
use moqui_lib::object::Value;

pub(crate) fn register(registry: &mut NativeRegistry) -> Result<(), Error> {
    registry.service_function(moqui_core::service_location!(echo), echo)
}

/// `example.ExampleServices.echo#Message`: the message back, with the user the call runs as. The service XML defines the
/// parameters, authentication and transaction; the parameters are already validated, and `ec` is the caller's, so the
/// user and the transaction are the caller's.
///
/// # Errors
///
/// Returns an error when the session cannot be read.
pub fn echo(
    _ec: &ExecutionContext,
    _service: &ServiceInfo<'_>,
    parameters: &Parameters,
) -> Result<Parameters, LibError> {
    let message = parameters.get("message").cloned().unwrap_or(Value::Null);
    let username = moqui_core::user::current_username()?.map_or(Value::Null, Value::String);
    let mut out = Parameters::new();
    out.insert("message".to_string(), message);
    out.insert("username".to_string(), username);
    Ok(out)
}
