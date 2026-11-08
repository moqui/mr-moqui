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

//! A script's text and the hash both engines keep its compiled form by (plans/archive/Script-Engine-Oxide.md section 5.1).

use std::hash::{Hash, Hasher};
use std::sync::Arc;

/// A script's text with the hash its compiled form is kept by, made once for a text that runs many times.
#[derive(Clone)]
pub(crate) struct ScriptText {
    text: Arc<str>,
    file_name: Arc<str>,
    hash: u64,
}

impl ScriptText {
    pub(crate) fn new(text: impl Into<Arc<str>>) -> Self {
        let text = text.into();
        Self::named(text, "eval_script")
    }

    pub(crate) fn named(text: impl Into<Arc<str>>, file_name: impl Into<Arc<str>>) -> Self {
        let text = text.into();
        let file_name = file_name.into();
        Self {
            hash: hash_text(&text),
            text,
            file_name,
        }
    }

    pub(crate) fn with_file_name(&self, file_name: &str) -> Self {
        Self {
            text: Arc::clone(&self.text),
            file_name: Arc::from(file_name),
            hash: self.hash,
        }
    }

    pub(crate) fn text(&self) -> &str {
        &self.text
    }

    /// The hash of the text alone.
    pub(crate) fn hash(&self) -> u64 {
        self.hash
    }

    /// The name the script has in stack traces.
    pub(crate) fn file_name(&self) -> &str {
        &self.file_name
    }

    /// The text, shared.
    pub(crate) fn shared_text(&self) -> &Arc<str> {
        &self.text
    }
}

pub(crate) fn hash_text(text: &str) -> u64 {
    use std::hash::BuildHasher as _;
    // foldhash: a script's text is hashed at each lookup that has no kept hash, and SipHash of it showed in profiles
    foldhash::fast::FixedState::default().hash_one(text)
}

pub(crate) fn hash_of(text_hash: u64, file_name: &str) -> u64 {
    use std::hash::BuildHasher as _;
    let mut hasher = foldhash::fast::FixedState::default().build_hasher();
    text_hash.hash(&mut hasher);
    file_name.hash(&mut hasher);
    hasher.finish()
}
