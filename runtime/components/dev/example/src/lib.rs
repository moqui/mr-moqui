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

//! Examples of Rust in a component. `submit_component!` puts this crate's implementations in the linked catalogue under
//! the component name `example`. A binary that links the crate (`use example as _;`) has them, and a factory boot uses
//! them only when it loads the component. The component's XML picks them: `MoquiConf.xml` selects the tool factory by
//! class, and the service definitions name the service locations.

mod example_tool;
pub mod services;

use moqui_core::context::Error;
use moqui_core::native::{submit_component, NativeComponent, NativeRegistry};

submit_component! {
    NativeComponent { name: "example", register }
}

/// This component's implementations, in a factory's catalogue.
fn register(registry: &mut NativeRegistry) -> Result<(), Error> {
    services::register(registry)?;
    example_tool::register(registry)
}
