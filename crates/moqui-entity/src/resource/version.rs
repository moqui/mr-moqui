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

//! A version of a resource file and who wrote it, which `dbresource://` and `git://` storage both write
//! (`moqui_core::context::resource` has the facade's side, docs/data/Revision-Management.md).

use std::cell::RefCell;

use moqui_lib::object::Value;

/// One version of a file, as Java's `ResourceReference.Version`: a `dbresource` history row, or a commit of a `git`
/// file. `version_date` is a Timestamp value. `note` is what was written with the version: the commit message (without
/// its `Moqui-User-Id` trailer) of a `git` file, the `versionNote` of a `dbresource` one.
#[derive(Clone, Debug, PartialEq)]
pub struct ResourceVersion {
    pub version_name: String,
    pub previous_version_name: Option<String>,
    pub version_date: Value,
    pub user_id: Option<String>,
    pub note: Option<String>,
}

/// Who wrote a version and when, when that is not the current user now: a version copied with its history from other
/// storage. Either part may be empty, which is the current user or now.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct VersionAuthor {
    pub user_id: Option<String>,
    /// Milliseconds since the epoch.
    pub date: Option<i64>,
}

thread_local! {
    static VERSION_AUTHOR: RefCell<Option<VersionAuthor>> = const { RefCell::new(None) };
}

/// Run `body` with the versions it writes (on `git://` and `dbresource://`) attributed to `author`, then put back the
/// author that was set before. `None` writes as the current user, now.
pub fn with_version_author<T>(author: Option<VersionAuthor>, body: impl FnOnce() -> T) -> T {
    struct Restore(Option<VersionAuthor>);
    impl Drop for Restore {
        fn drop(&mut self) {
            let previous = self.0.take();
            VERSION_AUTHOR.with(|cell| *cell.borrow_mut() = previous);
        }
    }
    let previous = VERSION_AUTHOR.with(|cell| cell.replace(author));
    let _restore = Restore(previous);
    body()
}

/// The author [`with_version_author`] set for the writes on this thread, if any.
pub fn version_author() -> Option<VersionAuthor> {
    VERSION_AUTHOR.with(|cell| cell.borrow().clone())
}

/// The last change of a file or directory.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ResourceChangeInfo {
    pub is_file: bool,
    /// A file's bytes.
    pub size: Option<u64>,
    /// The version that last changed it: a commit id on `git://` (for a directory, the last commit under it), the
    /// current version on `dbresource://`.
    pub version_name: Option<String>,
    /// Milliseconds since the epoch.
    pub date: Option<i64>,
    pub user_id: Option<String>,
    pub note: Option<String>,
}

impl ResourceChangeInfo {
    /// `{isFile, size, versionName, date, userId, note}` for a script.
    pub fn to_value(&self) -> Value {
        let mut map = indexmap::IndexMap::new();
        map.insert("isFile".to_string(), Value::Bool(self.is_file));
        map.insert(
            "size".to_string(),
            self.size
                .map_or(Value::Null, |size| Value::I64(i64::try_from(size).unwrap_or(i64::MAX))),
        );
        let text = |value: &Option<String>| value.as_ref().map_or(Value::Null, Value::string);
        map.insert("versionName".to_string(), text(&self.version_name));
        map.insert("date".to_string(), self.date.map_or(Value::Null, Value::I64));
        map.insert("userId".to_string(), text(&self.user_id));
        map.insert("note".to_string(), text(&self.note));
        Value::Map(map)
    }
}
