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

//! The artifact index the docs are read from (plans/Autodoc.md, "Generator"): a memory index (an export and
//! `moqui autodoc`: the files as they are, references read from them) or a dev server's rows (the Dev AIDE's viewer: one
//! doc at a time, with the calls observed at run time).

use std::sync::Arc;

use moqui_core::context::ExecutionContextFactory;
use moqui_lib::error::Error;

use crate::index::{self, ArtifactRow, Direction, MemoryIndex, RefEntry, RefTable};

/// What the docs read of an index.
pub trait DocIndex {
    /// Every artifact of `kinds`.
    ///
    /// # Errors
    ///
    /// Returns an error when the index cannot be read.
    fn artifacts(&self, kinds: &[&str]) -> Result<Vec<ArtifactRow>, Error>;

    /// The artifacts whose parent is `id`, in file order.
    ///
    /// # Errors
    ///
    /// Returns an error when the index cannot be read.
    fn children(&self, id: &str) -> Result<Vec<ArtifactRow>, Error>;

    /// What uses `id` (`In`) or what it uses (`Out`); an entity's or screen's parts count as it.
    ///
    /// # Errors
    ///
    /// Returns an error when the index cannot be read.
    fn refs(&self, id: &str, direction: Direction) -> Result<Vec<RefEntry>, Error>;

    /// [`DocIndex::refs`] of each of `ids`, one after another; a source that reads a database does it in a few queries.
    ///
    /// # Errors
    ///
    /// Returns an error when the index cannot be read.
    fn refs_of_ids(&self, ids: &[&str], direction: Direction) -> Result<Vec<RefEntry>, Error> {
        let mut entries = Vec::new();
        for id in ids {
            entries.extend(self.refs(id, direction)?);
        }
        Ok(entries)
    }

    /// What identifies the files read: the same files give the same stamp.
    fn stamp(&self) -> String;
}

/// A memory index with its reference table.
pub struct MemorySource<'a> {
    index: &'a MemoryIndex,
    table: RefTable<'a>,
}

impl<'a> MemorySource<'a> {
    #[must_use]
    pub fn new(index: &'a MemoryIndex) -> Self {
        Self {
            index,
            table: index.ref_table(),
        }
    }
}

impl DocIndex for MemorySource<'_> {
    fn artifacts(&self, kinds: &[&str]) -> Result<Vec<ArtifactRow>, Error> {
        Ok(self
            .index
            .artifacts
            .iter()
            .filter(|artifact| kinds.contains(&artifact.kind.as_str()))
            .cloned()
            .collect())
    }

    fn children(&self, id: &str) -> Result<Vec<ArtifactRow>, Error> {
        Ok(self.table.children(id))
    }

    fn refs(&self, id: &str, direction: Direction) -> Result<Vec<RefEntry>, Error> {
        Ok(self.table.refs(id, direction))
    }

    fn stamp(&self) -> String {
        self.index.stamp().hash
    }
}

/// The rows of a dev server's index.
pub struct DbSource {
    factory: Arc<ExecutionContextFactory>,
}

impl DbSource {
    #[must_use]
    pub fn new(factory: Arc<ExecutionContextFactory>) -> Self {
        Self { factory }
    }
}

impl DocIndex for DbSource {
    fn artifacts(&self, kinds: &[&str]) -> Result<Vec<ArtifactRow>, Error> {
        let kinds: Vec<String> = kinds.iter().map(|kind| (*kind).to_string()).collect();
        index::of_kinds(&self.factory, &kinds, &[], usize::MAX)
    }

    fn children(&self, id: &str) -> Result<Vec<ArtifactRow>, Error> {
        index::children(&self.factory, id)
    }

    fn refs(&self, id: &str, direction: Direction) -> Result<Vec<RefEntry>, Error> {
        index::refs(&self.factory, id, direction, &[])
    }

    fn refs_of_ids(&self, ids: &[&str], direction: Direction) -> Result<Vec<RefEntry>, Error> {
        index::refs_of_ids(&self.factory, ids, direction, &[])
    }

    fn stamp(&self) -> String {
        index::stamp(&self.factory).map(|stamp| stamp.hash).unwrap_or_default()
    }
}
