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

use super::server::lfs::{pointer, pointer_text, Asked, GIT_LFS_OBJECT, MAX_POINTER};
use super::{attributes, ops, GitProvider, Repo};
use moqui_core::context::{ContentAddress, ContentKind, Error, ResourceStat};
use moqui_core::entity::EntityValue;
use moqui_core::resource::blob::{BlobInfo, BlobStore, Expected};
use moqui_core::resource::source::{ByteSource, LazySource};

/// A file's content to store.
pub(super) enum FileContent<'a> {
    Bytes(&'a [u8]),
    /// `size` bytes to read.
    Stream(&'a mut dyn Read, u64),
}

impl GitProvider {
    /// Whether the file at `path` is an LFS file by the attributes at the branch head.
    pub(super) fn lfs_tracked(&self, repo: &Repo, path: &[String]) -> Result<bool, Error> {
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
        self.link_lfs(&repo.definition.id, &info.id, info.size)?;
        repo.store
            .write(Kind::Blob, pointer_text(&info.id, info.size).as_bytes())
    }

    /// Make the content `oid`, of `size` bytes, an LFS object of the repository, when it is not one yet (whether it
    /// was not): the application's upload check is asked as for an upload, and its refusal is the error.
    pub(super) fn link_lfs(&self, repository_id: &str, oid: &str, size: u64) -> Result<bool, Error> {
        let _rows = super::rows_guard();
        let entity = self.entity()?;
        let linked = entity
            .find(GIT_LFS_OBJECT)?
            .eq("repositoryId", Value::string(repository_id))
            .eq("oid", Value::string(oid))
            .one()?
            .is_some();
        if linked {
            return Ok(false);
        }
        // new to the repository: the application's check is asked as for an upload by git-lfs, before and after
        let object = [(oid.to_string(), size)];
        crate::server::lfs::check_upload(repository_id, &object, Asked::Before).map_err(Error::new)?;
        let mut row = EntityValue::make(&entity, GIT_LFS_OBJECT)?;
        row.set("repositoryId", Value::string(repository_id));
        row.set("oid", Value::string(oid));
        row.set("objectSize", Value::I64(i64::try_from(size).unwrap_or(i64::MAX)));
        if let Some(user) = moqui_core::user::current_user_id().ok().flatten() {
            row.set("uploadedByUserId", Value::string(user));
        }
        row.set("uploadedDate", moqui_core::user::now_effective_value());
        row.create(&entity)?;
        if let Err(refusal) = crate::server::lfs::check_upload(repository_id, &object, Asked::After) {
            row.delete(&entity)?;
            return Err(Error::new(refusal));
        }
        Ok(true)
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

    /// The file whose object is `blob` to read a window at a time, as it is in `repo`: an LFS object's content blob
    /// or a `db` repository's large object by range, with its SHA-256 as the tag; any other object (a deflated object
    /// of the filesystem store, a small row) read whole on the first read, tagged by its blob id. `size` is the
    /// object's when the caller knows it. An object the store does not have is an error.
    pub(super) fn open_blob(
        &self,
        repo: Arc<Repo>,
        blob: ObjectId,
        size: Option<u64>,
        last_modified: Option<i64>,
    ) -> Result<(ResourceStat, Arc<dyn ByteSource>), Error> {
        let stat = |size| ResourceStat {
            is_file: true,
            size,
            last_modified,
        };
        if let Some(info) = self.lfs_content(&repo, &blob)? {
            return Ok((stat(info.size), self.lfs_source(&info)?));
        }
        if let Some(source) = repo.store.content_source(&blob)? {
            return Ok((stat(source.len()), source));
        }
        let size = match size {
            Some(size) => size,
            None => object_size(&repo, &blob)?,
        };
        let source = LazySource::new(size, true, move || {
            Ok(ops::object(repo.store.as_ref(), &blob, Kind::Blob)?)
        })
        .with_tag(format!("git-{blob}"));
        Ok((stat(size), Arc::new(source)))
    }

    /// What a reader pins the file whose object is `blob` by: its LFS object, when the repository has the content,
    /// else the git object; with the tag [`GitProvider::open_blob`] gives its source. An object the store does not
    /// have is an error.
    pub(super) fn blob_address(&self, repo: &Repo, blob: &ObjectId) -> Result<ContentAddress, Error> {
        if let Some(info) = self.lfs_content(repo, blob)? {
            return Ok(ContentAddress {
                kind: ContentKind::GitLfs,
                tag: format!("sha256-{}", info.id),
                id: info.id,
                size: info.size,
            });
        }
        let (size, tag) = match repo.store.content_source(blob)? {
            Some(source) => (source.len(), source.tag()),
            None => (object_size(repo, blob)?, None),
        };
        Ok(ContentAddress {
            kind: ContentKind::GitObject,
            id: blob.to_string(),
            size,
            tag: tag.unwrap_or_else(|| format!("git-{blob}")),
        })
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

/// The size of the object `blob` of `repo`; one the store does not have is an error (a tree names it, so the
/// repository is missing an object).
fn object_size(repo: &Repo, blob: &ObjectId) -> Result<u64, Error> {
    repo.store
        .object_size(blob)?
        .ok_or_else(|| Error::new(format!("the object {blob} of {} is missing", repo.definition.id)))
}
