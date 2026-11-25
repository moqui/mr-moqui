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

//! Randomness, passed in (plans/archive/Vault-Instances.md §3.2). Native builds use [`OsRng`]; the wasm module fills from the
//! page's `crypto.getRandomValues`; the known-answer vectors use [`TestRng`], a fixed stream.

/// A source of random bytes. Every key, nonce, salt and share coefficient the crate makes comes from one.
pub trait Rng {
    /// Fills `out` with random bytes.
    fn fill(&mut self, out: &mut [u8]);
}

/// The operating system's random source, through getrandom.
#[cfg(feature = "std")]
#[derive(Clone, Copy, Debug, Default)]
pub struct OsRng;

#[cfg(feature = "std")]
impl Rng for OsRng {
    fn fill(&mut self, out: &mut [u8]) {
        // A host whose random source fails cannot make a key safely; carrying on with a zeroed buffer would be worse.
        getrandom::fill(out).expect("the operating system's random source answers");
    }
}

/// A deterministic stream for known-answer vectors and tests only: SHA-256 over a seed and a block counter. Never use
/// it for a key that protects anything.
#[cfg(any(test, feature = "test-vectors"))]
#[derive(Clone, Debug)]
pub struct TestRng {
    seed: [u8; 32],
    counter: u64,
    block: [u8; 32],
    used: usize,
}

#[cfg(any(test, feature = "test-vectors"))]
impl TestRng {
    /// A stream from a label; the same label gives the same bytes everywhere.
    pub fn new(label: &str) -> Self {
        use sha2::Digest as _;
        let seed: [u8; 32] = sha2::Sha256::digest(label.as_bytes()).into();
        Self {
            seed,
            counter: 0,
            block: [0; 32],
            used: 32,
        }
    }
}

#[cfg(any(test, feature = "test-vectors"))]
impl Rng for TestRng {
    fn fill(&mut self, out: &mut [u8]) {
        use sha2::Digest as _;
        for byte in out.iter_mut() {
            if self.used == self.block.len() {
                let mut hasher = sha2::Sha256::new();
                hasher.update(self.seed);
                hasher.update(self.counter.to_be_bytes());
                self.block = hasher.finalize().into();
                self.counter += 1;
                self.used = 0;
            }
            *byte = self.block[self.used];
            self.used += 1;
        }
    }
}
