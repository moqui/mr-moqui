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

// The framework's dev components (runtime/dev-component), in the packages with the dev tools (Dev and Debug). Naming
// each keeps its `submit_component!` entry in the binary; it adds nothing unless its component loads. `./moq build`
// generates a binary with these plus the add-on component crates in runtime/component (target/moqui-app).
#[cfg(feature = "devtools")]
use dev_aide as _;
#[cfg(feature = "devtools")]
use example as _;

fn main() {
    moqui_host::main();
}
