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

//! LFS files through `git://` locations, as git-lfs's filters make them in a working copy: a file the
//! `.gitattributes` of the branch head keep in LFS (`filter=lfs`) is written as a content blob of the database
//! (`moqui_core::resource::blob`), linked to the repository as its LFS object, and committed as an LFS pointer; reading it
//! gives the content (streamed for a response), and its size is the content's. A pointer whose content this
//! repository does not have (a mirror's, or one pushed with no upload) reads as the pointer.

use std::io::Read;
use std::sync::Arc;

use gix::objs::Kind;
use gix::ObjectId;
use moqui_lib::object::Value;

use super::server::lfs::{pointer, pointer_text, GIT_LFS_OBJECT, MAX_POINTER};
use super::{attributes, ops, GitProvider, Repo};
use moqui_core::context::Error;
use moqui_core::entity::EntityValue;
use moqui_core::resource::blob::{BlobInfo, BlobStore, Expected};
use moqui_core::resource::source::ByteSource;

/// A file's content to store.
pub(super) enum FileContent<'a> {
    Bytes(&'a [u8]),
    /// `size` bytes to read.
    Stream(&'a mut dyn Read, u64),
}

impl GitProvider {
    /// Whether the file at `path` is an LFS file by the attributes at the branch head.
    fn lfs_tracked(&self, repo: &Repo, path: &[String]) -> Result<bool, Error> {
        let Some(head) = Self::head(repo)? else {
            return Ok(false);
        };
        attributes::is_lfs(repo.store.as_ref(), &self.inner.attributes, &head.tree, path)
    }

    /// Store the content of the file at `path`: an LFS file's into a content blob of the repository, and its pointer
    /// as the git object; any other's as the object. The object to commit.
    pub(super) fn store_file(&self, repo: &Repo, path: &[String], content: FileContent<'_>) -> Result<ObjectId, Error> {
        if !self.lfs_tracked(repo, path)? {
            return match content {
                FileContent::Bytes(bytes) => repo.store.write(Kind::Blob, bytes),
                FileContent::Stream(input, size) => repo.store.write_stream(Kind::Blob, size, input),
            };
        }
        let blobs = BlobStore::new(self.entity()?);
        let info = match content {
            FileContent::Bytes(bytes) => blobs.put_bytes(bytes)?,
            FileContent::Stream(input, size) => blobs.put(
                input,
                &Expected {
                    id: None,
                    size: Some(size),
                },
                u64::MAX,
            )?,
        };
        self.link_lfs(&repo.definition.id, &info)?;
        repo.store
            .write(Kind::Blob, pointer_text(&info.id, info.size).as_bytes())
    }

    /// Make the content `info` an LFS object of the repository, when it is not one yet.
    pub(super) fn link_lfs(&self, repository_id: &str, info: &BlobInfo) -> Result<(), Error> {
        let _rows = super::rows_guard();
        let entity = self.entity()?;
        let linked = entity
            .find(GIT_LFS_OBJECT)?
            .eq("repositoryId", Value::string(repository_id))
            .eq("oid", Value::string(&info.id))
            .one()?
            .is_some();
        if linked {
            return Ok(());
        }
        let mut row = EntityValue::make(&entity, GIT_LFS_OBJECT)?;
        row.set("repositoryId", Value::string(repository_id));
        row.set("oid", Value::string(&info.id));
        row.set("objectSize", Value::I64(i64::try_from(info.size).unwrap_or(i64::MAX)));
        if let Some(user) = moqui_core::user::current_user_id().ok().flatten() {
            row.set("uploadedByUserId", Value::string(user));
        }
        row.set("uploadedDate", moqui_core::user::now_effective_value());
        Ok(row.create(&entity)?)
    }

    /// The LFS content the git object `blob` names, when it is a pointer whose content the repository has.
    pub(super) fn lfs_content(&self, repo: &Repo, blob: &ObjectId) -> Result<Option<BlobInfo>, Error> {
        if repo.store.object_size(blob)?.is_none_or(|size| size > MAX_POINTER) {
            return Ok(None);
        }
        let data = ops::object(repo.store.as_ref(), blob, Kind::Blob)?;
        let Some((oid, _)) = pointer(&data) else {
            return Ok(None);
        };
        let _rows = super::rows_guard();
        let entity = self.entity()?;
        let linked = entity
            .find(GIT_LFS_OBJECT)?
            .eq("repositoryId", Value::string(&repo.definition.id))
            .eq("oid", Value::string(&oid))
            .one()?
            .is_some();
        if !linked {
            return Ok(None);
        }
        Ok(BlobStore::new(entity).stat(&oid)?)
    }

    /// The content of the file whose object is `blob`: an LFS pointer's content, when the repository has it.
    pub(super) fn file_content(&self, repo: &Repo, blob: &ObjectId) -> Result<Vec<u8>, Error> {
        match self.lfs_content(repo, blob)? {
            Some(info) => Ok(moqui_core::resource::source::read_all(
                self.lfs_source(&info)?.as_ref(),
            )?),
            None => ops::object(repo.store.as_ref(), blob, Kind::Blob),
        }
    }

    /// The size of the file whose object is `blob`: an LFS pointer's content's, when the repository has it.
    pub(super) fn file_size(&self, repo: &Repo, blob: &ObjectId) -> Result<u64, Error> {
        match self.lfs_content(repo, blob)? {
            Some(info) => Ok(info.size),
            None => Ok(repo.store.object_size(blob)?.unwrap_or(0)),
        }
    }

    /// The source of the LFS content `info`.
    pub(super) fn lfs_source(&self, info: &BlobInfo) -> Result<Arc<dyn ByteSource>, Error> {
        Ok(BlobStore::new(self.entity()?).source(info))
    }

    /// `setup#GitLfs`: a commit of the repository's `.gitattributes` with `filter=lfs` for each of `patterns` it does
    /// not have yet. The commit, `None` when it has them all.
    pub fn setup_lfs(&self, repository_id: &str, patterns: &[&str]) -> Result<Option<ObjectId>, Error> {
        let repo = self.repo(repository_id)?;
        let path = vec![attributes::FILE.to_string()];
        let existing = match Self::head(&repo)? {
            Some(head) => match ops::resolve(repo.store.as_ref(), &head.tree, &path)? {
                Some((mode, blob)) if !mode.is_tree() => {
                    String::from_utf8_lossy(&ops::object(repo.store.as_ref(), &blob, Kind::Blob)?).into_owned()
                }
                _ => String::new(),
            },
            None => String::new(),
        };
        let text = attributes::with_lfs_patterns(&existing, patterns);
        if text == existing {
            return Ok(None);
        }
        self.commit_files(
            repository_id,
            "Keep large files in Git LFS",
            &[(path, text.into_bytes())],
            &[],
        )
    }
}
