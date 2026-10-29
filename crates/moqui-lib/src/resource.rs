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

//! Resource references for a file and an in-memory tree.
//!
//! There is no scheme router. `file://`, `runtime://`, `component://`, and
//! `dbresource://` are not interpreted here. A missing path is still a reference
//! whose `exists` is false. Child search follows `ResourceReference` and does
//! not keep a location cache.

use std::cell::RefCell;
use std::collections::HashSet;
use std::fs;
use std::io::Write;
use std::path::PathBuf;
use std::rc::Rc;
use std::time::{SystemTime, UNIX_EPOCH};

use indexmap::IndexMap;

use crate::error::Error;
use crate::string::to_string_clean_bom;

pub trait Resource: Sized + Clone {
    fn location(&self) -> &str;
    fn exists(&self) -> bool;
    fn is_file(&self) -> bool;
    fn is_directory(&self) -> bool;
    fn read_bytes(&self) -> Result<Vec<u8>, Error>;
    fn write_bytes(&self, bytes: &[u8]) -> Result<(), Error>;
    fn last_modified(&self) -> Result<i64, Error>;
    fn size(&self) -> Result<u64, Error>;
    fn directory_entries(&self) -> Result<Vec<Self>, Error>;
    fn child(&self, name: &str) -> Option<Self>;
    fn make_directory(&self, name: &str) -> Result<Self, Error>;
    fn make_file(&self, name: &str) -> Result<Self, Error>;
    fn delete(&self) -> Result<bool, Error>;
    fn with_location(&self, location: &str) -> Self;

    fn read_text(&self) -> Result<String, Error> {
        Ok(to_string_clean_bom(&self.read_bytes()?))
    }

    fn write_text(&self, text: &str) -> Result<(), Error> {
        self.write_bytes(text.as_bytes())
    }

    fn file_name(&self) -> &str {
        file_tail(self.location())
    }

    fn content_type(&self) -> Option<&'static str> {
        content_type_for_name(self.file_name())
    }

    fn is_text(&self) -> bool {
        self.content_type().is_some_and(is_text_content_type)
    }

    fn is_binary(&self) -> bool {
        self.content_type().is_some_and(is_binary_content_type)
    }

    fn find_matching_directory(&self) -> Self {
        if self.is_directory() {
            return self.clone();
        }
        let mut dir_loc = self.location().to_string();
        let mut directory = self.clone();
        while !(directory.exists() && directory.is_directory()) {
            let Some(dot) = dir_loc.rfind('.') else {
                break;
            };
            if dot == 0 {
                break;
            }
            dir_loc.truncate(dot);
            directory = self.with_location(&dir_loc);
        }
        directory
    }

    /// Files in this directory, or in the directory that matches this file name.
    fn children(&self) -> Result<Vec<Self>, Error> {
        let directory = self.find_matching_directory();
        if !directory.exists() {
            return Ok(Vec::new());
        }
        let mut files = Vec::new();
        for entry in directory.directory_entries()? {
            if entry.is_file() {
                files.push(entry);
            }
        }
        Ok(files)
    }

    fn find_child_file(&self, relative_path: &str) -> Self {
        if relative_path.is_empty() {
            return self.clone();
        }
        let directory = self.find_matching_directory();
        if directory.exists() {
            let literal = directory.with_location(&join_location(directory.location(), relative_path));
            if literal.exists() && literal.is_file() {
                return literal;
            }
            let (dir_path, filename) = split_relative(relative_path);
            let child_dir = directory.find_child_directory(dir_path);
            if let Some(found) = child_dir.internal_find_child_file(filename, &mut HashSet::new()) {
                return found;
            }
            return literal;
        }
        let base = strip_extension_after_slash(self.location());
        self.with_location(&join_location(&base, relative_path))
    }

    fn find_child_directory(&self, relative_path: &str) -> Self {
        if relative_path.is_empty() {
            return self.clone();
        }
        let mut current = if self.is_file() {
            self.find_matching_directory()
        } else {
            self.clone()
        };
        let mut missing = false;
        for name in relative_path.split('/').filter(|segment| !segment.is_empty()) {
            match current.internal_find_child_dir(name, &mut HashSet::new()) {
                Some(found) => current = found,
                None => {
                    missing = true;
                    break;
                }
            }
        }
        if missing {
            let mut base = self.location().to_string();
            if self.is_file() {
                base = strip_extension_after_slash(&base);
            }
            return self.with_location(&join_location(&base, relative_path));
        }
        current
    }

    fn internal_find_child_dir(&self, child_name: &str, parents: &mut HashSet<String>) -> Option<Self> {
        if !self.exists() {
            return None;
        }
        if child_name.is_empty() {
            return Some(self.clone());
        }
        let direct = self.with_location(&join_location(self.location(), child_name));
        if direct.exists() {
            return Some(direct);
        }
        let entries = self.directory_entries().ok()?;
        for child in entries {
            if !child.is_directory() {
                continue;
            }
            let name = child.file_name().to_string();
            if name == child_name || name.contains(&format!("{child_name}.")) {
                return Some(child);
            }
            if parents.contains(child.location()) {
                continue;
            }
            let mut next = parents.clone();
            next.insert(self.location().to_string());
            next.insert(child.location().to_string());
            if let Some(found) = child.internal_find_child_dir(child_name, &mut next) {
                return Some(found);
            }
        }
        None
    }

    fn internal_find_child_file(&self, filename: &str, parents: &mut HashSet<String>) -> Option<Self> {
        if !self.exists() || filename.is_empty() {
            return None;
        }
        if let Some(exact) = self.child(filename) {
            if exact.exists() && exact.is_file() {
                return Some(exact);
            }
        }
        let entries = self.directory_entries().ok()?;
        for child in &entries {
            if child.is_file() {
                let name = child.file_name();
                if name == filename || name.starts_with(&format!("{filename}.")) {
                    return Some(child.clone());
                }
            }
        }
        for child in &entries {
            if !child.is_directory() || parents.contains(child.location()) {
                continue;
            }
            let mut next = parents.clone();
            next.insert(self.location().to_string());
            next.insert(child.location().to_string());
            if let Some(found) = child.internal_find_child_file(filename, &mut next) {
                return Some(found);
            }
        }
        None
    }
}

fn join_location(parent: &str, child: &str) -> String {
    let parent = parent.trim_end_matches('/');
    if child.starts_with('/') {
        format!("{parent}{child}")
    } else if parent.is_empty() {
        child.to_string()
    } else {
        format!("{parent}/{child}")
    }
}

fn split_relative(relative: &str) -> (&str, &str) {
    relative.rsplit_once('/').unwrap_or(("", relative))
}

fn strip_extension_after_slash(location: &str) -> String {
    let (directory, name) = location.split_at(location.rfind('/').map_or(0, |slash| slash + 1));
    let stem = name.split_once('.').map_or(name, |(stem, _)| stem);
    format!("{directory}{stem}")
}

pub fn content_type_for_name(filename: &str) -> Option<&'static str> {
    let (_, ext) = filename.rsplit_once('.')?;
    if ext.is_empty() || ext == filename {
        return None;
    }
    Some(match ext.to_ascii_lowercase().as_str() {
        "xml" | "xsd" | "xsl" => "application/xml",
        "txt" | "csv" | "ftl" | "md" | "properties" | "log" => "text/plain",
        "html" | "htm" => "text/html",
        "css" => "text/css",
        "js" => "application/javascript",
        "json" => "application/json",
        "yaml" | "yml" => "application/yaml",
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "svg" => "image/svg+xml",
        "pdf" => "application/pdf",
        "gz" => "application/gzip",
        "zip" => "application/zip",
        _ => return None,
    })
}

pub fn is_text_content_type(content_type: &str) -> bool {
    let content_type = content_type.split(';').next().unwrap_or(content_type).trim();
    if content_type.is_empty() {
        return false;
    }
    content_type.starts_with("text/")
        || content_type == "application/javascript"
        || content_type == "application/json"
        || content_type == "application/jwt"
        || content_type.ends_with("+json")
        || content_type == "application/rtf"
        || content_type.starts_with("application/xml")
        || content_type.ends_with("+xml")
        || content_type.starts_with("application/yaml")
        || content_type.ends_with("+yaml")
}

pub fn is_binary_content_type(content_type: &str) -> bool {
    !content_type.is_empty() && !is_text_content_type(content_type)
}

/// A filesystem path. The location string is the path that was passed in.
#[derive(Clone, Debug)]
pub struct FileResource {
    location: String,
    path: PathBuf,
}

impl FileResource {
    pub fn new(location: impl Into<String>) -> Self {
        let location = location.into();
        let path = PathBuf::from(&location);
        Self { location, path }
    }
}

impl FileResource {
    pub fn path(&self) -> &std::path::Path {
        &self.path
    }
}

impl Resource for FileResource {
    fn location(&self) -> &str {
        &self.location
    }

    fn exists(&self) -> bool {
        self.path.exists()
    }

    fn is_file(&self) -> bool {
        self.path.is_file()
    }

    fn is_directory(&self) -> bool {
        self.path.is_dir()
    }

    fn read_bytes(&self) -> Result<Vec<u8>, Error> {
        fs::read(&self.path).map_err(|err| Error::new(format!("could not read {}", self.location)).with_source(err))
    }

    fn write_bytes(&self, bytes: &[u8]) -> Result<(), Error> {
        if let Some(parent) = self.path.parent() {
            if !parent.as_os_str().is_empty() && !parent.exists() {
                return Err(Error::new(format!(
                    "parent directory does not exist for {}",
                    self.location
                )));
            }
        }
        // A temp file beside the target, then a rename: a reader sees the old file or the new one, never a part, and
        // a failed write leaves the old one. A symlink is followed, so the link stays a link.
        let target = match fs::symlink_metadata(&self.path) {
            Ok(meta) if meta.file_type().is_symlink() => fs::canonicalize(&self.path)
                .map_err(|err| Error::new(format!("could not write {}", self.location)).with_source(err))?,
            _ => self.path.clone(),
        };
        let temp = temp_beside(&target);
        let written = write_then_rename(&temp, &target, bytes);
        if written.is_err() {
            let _ = fs::remove_file(&temp);
        }
        written.map_err(|err| Error::new(format!("could not write {}", self.location)).with_source(err))
    }

    fn last_modified(&self) -> Result<i64, Error> {
        let modified = fs::metadata(&self.path)
            .and_then(|meta| meta.modified())
            .map_err(|err| Error::new(format!("no last modified for {}", self.location)).with_source(err))?;
        Ok(system_millis(modified))
    }

    fn size(&self) -> Result<u64, Error> {
        fs::metadata(&self.path)
            .map(|meta| meta.len())
            .map_err(|err| Error::new(format!("no size for {}", self.location)).with_source(err))
    }

    fn directory_entries(&self) -> Result<Vec<Self>, Error> {
        let mut names = Vec::new();
        for entry in fs::read_dir(&self.path)
            .map_err(|err| Error::new(format!("could not list {}", self.location)).with_source(err))?
        {
            let entry =
                entry.map_err(|err| Error::new(format!("could not list {}", self.location)).with_source(err))?;
            names.push(entry.file_name());
        }
        names.sort();
        Ok(names
            .into_iter()
            .map(|name| {
                let name = name.to_string_lossy();
                self.with_location(&join_location(&self.location, &name))
            })
            .collect())
    }

    fn child(&self, name: &str) -> Option<Self> {
        if name.is_empty() {
            return None;
        }
        let directory = self.find_matching_directory();
        Some(self.with_location(&join_location(directory.location(), name)))
    }

    fn make_directory(&self, name: &str) -> Result<Self, Error> {
        if !self.exists() || !self.is_directory() {
            return Err(Error::new(format!("{} is not a directory", self.location)));
        }
        let child = self.child(name).ok_or_else(|| Error::new("directory name is empty"))?;
        fs::create_dir(&child.path)
            .map_err(|err| Error::new(format!("could not create {}", child.location)).with_source(err))?;
        Ok(child)
    }

    fn make_file(&self, name: &str) -> Result<Self, Error> {
        if !self.exists() || !self.is_directory() {
            return Err(Error::new(format!("{} is not a directory", self.location)));
        }
        let child = self.child(name).ok_or_else(|| Error::new("file name is empty"))?;
        child.write_bytes(&[])?;
        Ok(child)
    }

    fn delete(&self) -> Result<bool, Error> {
        if !self.exists() {
            return Ok(false);
        }
        let result = if self.is_directory() {
            fs::remove_dir(&self.path)
        } else {
            fs::remove_file(&self.path)
        };
        match result {
            Ok(()) => Ok(true),
            // Gone between the check and the remove: nothing was deleted here.
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(false),
            Err(err) => Err(Error::new(format!("could not delete {}", self.location)).with_source(err)),
        }
    }

    fn with_location(&self, location: &str) -> Self {
        Self::new(location)
    }
}

/// A hidden name in `target`'s directory that no other write in this process uses.
fn temp_beside(target: &std::path::Path) -> PathBuf {
    static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let serial = NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let name = target
        .file_name()
        .map(|name| name.to_string_lossy())
        .unwrap_or_default();
    target.with_file_name(format!(".{name}.{}.{serial}.tmp", std::process::id()))
}

/// Write `bytes` to `temp` with the permissions `target` has, flush them to disk, and rename over `target`.
fn write_then_rename(temp: &std::path::Path, target: &std::path::Path, bytes: &[u8]) -> std::io::Result<()> {
    let mut file = fs::OpenOptions::new().write(true).create_new(true).open(temp)?;
    if let Ok(meta) = fs::metadata(target) {
        file.set_permissions(meta.permissions())?;
    }
    file.write_all(bytes)?;
    file.sync_all()?;
    drop(file);
    fs::rename(temp, target)
}

struct MemFile {
    bytes: Vec<u8>,
    modified: i64,
}

struct MemTree {
    dirs: IndexMap<String, i64>,
    files: IndexMap<String, MemFile>,
}

/// An in-memory directory tree. Clones share the tree.
#[derive(Clone)]
pub struct MemoryResource {
    tree: Rc<RefCell<MemTree>>,
    root_location: String,
    relative: String,
    location: String,
}

impl MemoryResource {
    pub fn new_root(location: impl Into<String>) -> Self {
        let root_location = location.into();
        let mut dirs = IndexMap::new();
        dirs.insert(String::new(), now_millis());
        Self {
            tree: Rc::new(RefCell::new(MemTree {
                dirs,
                files: IndexMap::new(),
            })),
            root_location: root_location.clone(),
            relative: String::new(),
            location: root_location,
        }
    }

    fn relative_of(&self, location: &str) -> String {
        if location == self.root_location {
            return String::new();
        }
        let prefix = format!("{}/", self.root_location);
        location
            .strip_prefix(&prefix)
            .unwrap_or(location)
            .trim_start_matches('/')
            .trim_end_matches('/')
            .to_string()
    }
}

impl Resource for MemoryResource {
    fn location(&self) -> &str {
        &self.location
    }

    fn exists(&self) -> bool {
        let tree = self.tree.borrow();
        tree.dirs.contains_key(&self.relative) || tree.files.contains_key(&self.relative)
    }

    fn is_file(&self) -> bool {
        self.tree.borrow().files.contains_key(&self.relative)
    }

    fn is_directory(&self) -> bool {
        self.tree.borrow().dirs.contains_key(&self.relative)
    }

    fn read_bytes(&self) -> Result<Vec<u8>, Error> {
        self.tree
            .borrow()
            .files
            .get(&self.relative)
            .map(|file| file.bytes.clone())
            .ok_or_else(|| Error::new(format!("could not read {}", self.display_location())))
    }

    fn write_bytes(&self, bytes: &[u8]) -> Result<(), Error> {
        let mut tree = self.tree.borrow_mut();
        if tree.dirs.contains_key(&self.relative) {
            return Err(Error::new(format!("{} is a directory", self.display_location())));
        }
        if !tree.files.contains_key(&self.relative) {
            let parent = parent_path(&self.relative);
            if !tree.dirs.contains_key(parent) {
                return Err(Error::new(format!(
                    "parent directory does not exist for {}",
                    self.display_location()
                )));
            }
        }
        tree.files.insert(
            self.relative.clone(),
            MemFile {
                bytes: bytes.to_vec(),
                modified: now_millis(),
            },
        );
        Ok(())
    }

    fn last_modified(&self) -> Result<i64, Error> {
        let tree = self.tree.borrow();
        tree.files
            .get(&self.relative)
            .map(|file| file.modified)
            .or_else(|| tree.dirs.get(&self.relative).copied())
            .ok_or_else(|| Error::new(format!("no last modified for {}", self.display_location())))
    }

    fn size(&self) -> Result<u64, Error> {
        let tree = self.tree.borrow();
        if let Some(file) = tree.files.get(&self.relative) {
            return Ok(file.bytes.len() as u64);
        }
        if tree.dirs.contains_key(&self.relative) {
            return Ok(0);
        }
        Err(Error::new(format!("no size for {}", self.display_location())))
    }

    fn directory_entries(&self) -> Result<Vec<Self>, Error> {
        let tree = self.tree.borrow();
        if !tree.dirs.contains_key(&self.relative) {
            return Err(Error::new(format!("could not list {}", self.display_location())));
        }
        let mut names = Vec::new();
        for path in tree.dirs.keys().chain(tree.files.keys()) {
            if path.is_empty() {
                continue;
            }
            if parent_path(path) == self.relative {
                names.push(file_tail(path).to_string());
            }
        }
        names.sort();
        names.dedup();
        drop(tree);
        Ok(names
            .into_iter()
            .map(|name| self.with_location(&join_location(self.display_location(), &name)))
            .collect())
    }

    fn child(&self, name: &str) -> Option<Self> {
        if name.is_empty() {
            return None;
        }
        let directory = self.find_matching_directory();
        Some(self.with_location(&join_location(directory.display_location(), name)))
    }

    fn make_directory(&self, name: &str) -> Result<Self, Error> {
        if !self.is_directory() {
            return Err(Error::new(format!("{} is not a directory", self.display_location())));
        }
        let child = self.child(name).ok_or_else(|| Error::new("directory name is empty"))?;
        self.tree.borrow_mut().dirs.insert(child.relative.clone(), now_millis());
        Ok(child)
    }

    fn make_file(&self, name: &str) -> Result<Self, Error> {
        if !self.is_directory() {
            return Err(Error::new(format!("{} is not a directory", self.display_location())));
        }
        let child = self.child(name).ok_or_else(|| Error::new("file name is empty"))?;
        child.write_bytes(&[])?;
        Ok(child)
    }

    fn delete(&self) -> Result<bool, Error> {
        if self.relative.is_empty() {
            return Ok(false);
        }
        let mut tree = self.tree.borrow_mut();
        if tree.files.shift_remove(&self.relative).is_some() {
            return Ok(true);
        }
        if !tree.dirs.contains_key(&self.relative) {
            return Ok(false);
        }
        let prefix = format!("{}/", self.relative);
        let occupied = tree
            .dirs
            .keys()
            .chain(tree.files.keys())
            .any(|path| path.starts_with(&prefix));
        if occupied {
            return Ok(false);
        }
        tree.dirs.shift_remove(&self.relative);
        Ok(true)
    }

    fn with_location(&self, location: &str) -> Self {
        Self {
            tree: Rc::clone(&self.tree),
            root_location: self.root_location.clone(),
            relative: self.relative_of(location),
            location: location.to_string(),
        }
    }
}

impl MemoryResource {
    fn display_location(&self) -> &str {
        &self.location
    }
}

fn parent_path(path: &str) -> &str {
    path.rsplit_once('/').map_or("", |(parent, _)| parent)
}

fn file_tail(path: &str) -> &str {
    path.rsplit_once('/').map_or(path, |(_, tail)| tail)
}

fn now_millis() -> i64 {
    system_millis(SystemTime::now())
}

fn system_millis(time: SystemTime) -> i64 {
    time.duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis() as i64)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn path_helpers_split_at_the_last_slash() {
        assert_eq!(split_relative("a/b/c.xml"), ("a/b", "c.xml"));
        assert_eq!(split_relative("c.xml"), ("", "c.xml"));
        assert_eq!(parent_path("a/b"), "a");
        assert_eq!(parent_path("a"), "");
        assert_eq!(file_tail("a/b"), "b");
        assert_eq!(file_tail("b"), "b");
        assert_eq!(file_tail("a/"), "");
        assert_eq!(strip_extension_after_slash("a/b.c/d.tar.gz"), "a/b.c/d");
        assert_eq!(strip_extension_after_slash("d.xml"), "d");
        assert_eq!(strip_extension_after_slash("a.b/"), "a.b/");
        assert_eq!(strip_extension_after_slash("noext"), "noext");
    }

    #[test]
    fn memory_tree_finds_children_under_a_matching_directory() {
        let root = MemoryResource::new_root("memory");
        let screen = root.make_directory("screen").unwrap();
        screen.make_file("sub.xml").unwrap().write_text("<screen/>").unwrap();
        let nested = screen.make_directory("nested").unwrap();
        nested.make_file("deep.xml").unwrap().write_text("deep").unwrap();
        let screen_file = root.make_file("screen.xml").unwrap();
        screen_file.write_text("<file/>").unwrap();

        let found = screen_file.find_child_file("sub.xml");
        assert!(found.exists() && found.is_file());
        assert_eq!(found.read_text().unwrap(), "<screen/>");
        let deep = screen_file.find_child_file("deep.xml");
        assert_eq!(deep.read_text().unwrap(), "deep");
        let nested_dir = screen_file.find_child_directory("nested");
        assert!(nested_dir.is_directory());
        let missing = screen_file.find_child_file("nope.xml");
        assert!(!missing.exists());
        let children = screen_file.children().unwrap();
        assert!(children.iter().any(|child| child.file_name() == "sub.xml"));
        assert!(screen_file.is_text());
        assert_eq!(content_type_for_name("page.xml"), Some("application/xml"));
    }

    #[test]
    fn file_resource_round_trips_in_a_temp_directory() {
        let dir = std::env::temp_dir().join(format!("moqui-lib-resource-{}-{}", std::process::id(), now_millis()));
        fs::create_dir_all(&dir).unwrap();
        let root = FileResource::new(dir.to_string_lossy());
        let file = root.make_file("note.txt").unwrap();
        file.write_text("hello").unwrap();
        assert_eq!(file.read_text().unwrap(), "hello");
        assert!(file.size().unwrap() >= 5);
        assert!(file.last_modified().unwrap() > 0);
        let listed = root.children().unwrap();
        assert_eq!(listed.len(), 1);
        assert!(file.delete().unwrap());
        assert!(!file.exists());
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn file_write_replaces_whole_and_delete_reports_failures() {
        use std::os::unix::fs::PermissionsExt;
        let dir = std::env::temp_dir().join(format!("moqui-lib-replace-{}-{}", std::process::id(), now_millis()));
        fs::create_dir_all(&dir).unwrap();
        let root = FileResource::new(dir.to_string_lossy());
        let file = root.make_file("data.txt").unwrap();
        file.write_text("the first and longer text").unwrap();
        fs::set_permissions(dir.join("data.txt"), fs::Permissions::from_mode(0o640)).unwrap();
        file.write_text("short").unwrap();
        assert_eq!(file.read_text().unwrap(), "short");
        let mode = fs::metadata(dir.join("data.txt")).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o640);
        // No temp file is left beside it.
        assert_eq!(fs::read_dir(&dir).unwrap().count(), 1);

        // Through a symlink the target is written and the link stays.
        std::os::unix::fs::symlink(dir.join("data.txt"), dir.join("link.txt")).unwrap();
        FileResource::new(dir.join("link.txt").to_string_lossy())
            .write_text("by link")
            .unwrap();
        assert!(fs::symlink_metadata(dir.join("link.txt"))
            .unwrap()
            .file_type()
            .is_symlink());
        assert_eq!(file.read_text().unwrap(), "by link");

        // A directory that is not empty is an error, not a quiet false; a missing file is false.
        let sub = root.make_directory("sub").unwrap();
        sub.make_file("inner.txt").unwrap();
        assert!(sub.delete().is_err());
        assert!(!FileResource::new(dir.join("missing.txt").to_string_lossy())
            .delete()
            .unwrap());
        let _ = fs::remove_dir_all(&dir);
    }
}
