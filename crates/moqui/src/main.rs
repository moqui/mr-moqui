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

// jemalloc, not the C library's malloc: 5 to 15% off script-heavy rows (plans/Profiling.md Round 37). To go back to the
// system allocator, remove these lines (and the dependency); nothing else names the allocator.
#[cfg(not(target_env = "msvc"))]
#[global_allocator]
static GLOBAL: tikv_jemallocator::Jemalloc = tikv_jemallocator::Jemalloc;

fn main() {
    moqui_host::main();
}
