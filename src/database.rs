use crate::mode::Mode;
use crate::{Error, Result};
use rayon::prelude::*;
use regex::Regex;
use serde::{Deserialize, Serialize};
use std::fs::{DirEntry, FileType, Metadata};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::SystemTime;

#[derive(Debug, Serialize, Deserialize)]
pub struct Database {
    files: Vec<EntryNode>,
    dirs: Vec<EntryNode>,
    file_statuses: EntryStatusVec,
    dir_statuses: EntryStatusVec,
}

impl<'a> Database {
    pub fn num_entries(&self) -> usize {
        self.files.len() + self.dirs.len()
    }

    pub fn search(&self, pattern: &Regex, in_path: bool) -> Vec<EntryId> {
        let match_file = |(i, node): (usize, &EntryNode)| {
            if self.node_matches(node, pattern, in_path) {
                Some(EntryId::File(i))
            } else {
                None
            }
        };
        let match_dir = |(i, node): (usize, &EntryNode)| {
            if self.node_matches(node, pattern, in_path) {
                Some(EntryId::Directory(i))
            } else {
                None
            }
        };

        let files = (0..self.files.len())
            .into_par_iter()
            .zip(self.files.par_iter())
            .filter_map(match_file);
        let dirs = (0..self.dirs.len())
            .into_par_iter()
            .zip(self.dirs.par_iter())
            .filter_map(match_dir);

        files.chain(dirs).collect()
    }

    pub fn abortable_search(
        &self,
        pattern: &Regex,
        in_path: bool,
        aborted: Arc<AtomicBool>,
    ) -> Result<Vec<EntryId>> {
        let match_file = |(i, node): (usize, &EntryNode)| {
            if aborted.load(Ordering::Relaxed) {
                Some(Err(Error::SearchAbort))
            } else if self.node_matches(node, pattern, in_path) {
                Some(Ok(EntryId::File(i)))
            } else {
                None
            }
        };
        let match_dir = |(i, node): (usize, &EntryNode)| {
            if aborted.load(Ordering::Relaxed) {
                Some(Err(Error::SearchAbort))
            } else if self.node_matches(node, pattern, in_path) {
                Some(Ok(EntryId::Directory(i)))
            } else {
                None
            }
        };

        let files = (0..self.files.len())
            .into_par_iter()
            .zip(self.files.par_iter())
            .filter_map(match_file);
        let dirs = (0..self.dirs.len())
            .into_par_iter()
            .zip(self.dirs.par_iter())
            .filter_map(match_dir);

        files.chain(dirs).collect()
    }

    pub fn entry<'b>(&'a self, id: &'b EntryId) -> Entry<'a, 'b> {
        Entry { database: self, id }
    }

    fn node_matches(&self, node: &EntryNode, pattern: &Regex, in_path: bool) -> bool {
        if in_path {
            if let Some(path) = self.path_from_node(node).to_str() {
                if pattern.is_match(path) {
                    return true;
                }
            }
        } else if pattern.is_match(&node.name) {
            return true;
        }
        false
    }

    fn push_file(&mut self, precursor: EntryPrecursor, parent: usize) {
        self.files.push(EntryNode {
            name: precursor.name,
            parent,
        });
        self.file_statuses.push(&precursor.status);
    }

    fn push_dir(&mut self, precursor: EntryPrecursor, parent: usize) {
        self.dirs.push(EntryNode {
            name: precursor.name,
            parent,
        });
        self.dir_statuses.push(&precursor.status);
    }

    fn path_from_node(&self, node: &EntryNode) -> PathBuf {
        let mut buf = PathBuf::new();
        self.path_from_node_impl(node.parent, &mut buf);
        buf.push(&node.name);
        buf
    }

    fn path_from_node_impl(&self, index: usize, mut buf: &mut PathBuf) {
        let dir = &self.dirs[index];
        if dir.parent == 0 {
            buf.push(&self.dirs[dir.parent].name);
        } else {
            self.path_from_node_impl(dir.parent, &mut buf);
        }
        buf.push(&dir.name);
    }
}

pub struct DatabaseBuilder {
    path: PathBuf,
    index_flags: IndexFlags,
}

impl DatabaseBuilder {
    pub fn new<P: AsRef<Path>>(path: P) -> Self {
        Self {
            path: path.as_ref().to_path_buf(),
            index_flags: IndexFlags {
                size: false,
                mode: false,
                created: false,
                modified: false,
                accessed: false,
                ignore_hidden: false,
            },
        }
    }

    pub fn size(&mut self, yes: bool) -> &mut Self {
        self.index_flags.size = yes;
        self
    }

    pub fn mode(&mut self, yes: bool) -> &mut Self {
        self.index_flags.mode = yes;
        self
    }

    pub fn created(&mut self, yes: bool) -> &mut Self {
        self.index_flags.created = yes;
        self
    }

    pub fn modified(&mut self, yes: bool) -> &mut Self {
        self.index_flags.modified = yes;
        self
    }

    pub fn accessed(&mut self, yes: bool) -> &mut Self {
        self.index_flags.accessed = yes;
        self
    }

    pub fn ignore_hidden(&mut self, yes: bool) -> &mut Self {
        self.index_flags.ignore_hidden = yes;
        self
    }

    pub fn build(&self) -> Result<Database> {
        let root_precursor = EntryPrecursor::from_path(&self.path, &self.index_flags)?;
        let root_node = EntryNode {
            name: root_precursor.name,
            parent: 0, // points to itself
        };
        let mut database = Database {
            files: Vec::new(),
            dirs: vec![root_node],
            file_statuses: EntryStatusVec::new(&self.index_flags),
            dir_statuses: EntryStatusVec::new(&IndexFlags {
                size: false,
                ..self.index_flags
            }),
        };
        database.dir_statuses.push(&root_precursor.status);

        let database = Arc::new(Mutex::new(database));
        walk_file_system(database.clone(), &self.index_flags, &self.path, 0);

        // safe to unwrap since create_node, which is the only user of database, has returned
        Ok(Arc::try_unwrap(database).unwrap().into_inner().unwrap())
    }
}

#[derive(Debug)]
pub enum EntryId {
    File(usize),
    Directory(usize),
}

impl EntryId {
    fn is_dir(&self) -> bool {
        if let EntryId::Directory(_) = self {
            true
        } else {
            false
        }
    }
}

pub struct Entry<'a, 'b> {
    database: &'a Database,
    id: &'b EntryId,
}

impl Entry<'_, '_> {
    pub fn basename(&self) -> &str {
        &self.node().name
    }

    pub fn path(&self) -> PathBuf {
        self.database.path_from_node(&self.node())
    }

    pub fn extension(&self) -> Option<&str> {
        if self.id.is_dir() {
            return None;
        }

        let name = &self.node().name;
        if name.contains('.') {
            Some(name.split('.').last().unwrap())
        } else {
            None
        }
    }

    pub fn size(&self) -> Option<u64> {
        let (status_vec, i) = self.status_vec_index();
        status_vec.size.as_ref().map(|v| v[i])
    }

    pub fn mode(&self) -> Option<Mode> {
        let (status_vec, i) = self.status_vec_index();
        status_vec.mode.as_ref().map(|v| v[i])
    }

    pub fn created(&self) -> Option<&SystemTime> {
        let (status_vec, i) = self.status_vec_index();
        status_vec.created.as_ref().map(|v| &v[i])
    }

    pub fn modified(&self) -> Option<&SystemTime> {
        let (status_vec, i) = self.status_vec_index();
        status_vec.modified.as_ref().map(|v| &v[i])
    }

    pub fn accessed(&self) -> Option<&SystemTime> {
        let (status_vec, i) = self.status_vec_index();
        status_vec.accessed.as_ref().map(|v| &v[i])
    }

    fn node(&self) -> &EntryNode {
        match self.id {
            EntryId::File(i) => &self.database.files[*i],
            EntryId::Directory(i) => &self.database.dirs[*i],
        }
    }

    fn status_vec_index(&self) -> (&EntryStatusVec, usize) {
        match self.id {
            EntryId::File(i) => (&self.database.file_statuses, *i),
            EntryId::Directory(i) => (&self.database.dir_statuses, *i),
        }
    }
}

#[derive(Debug, Serialize, Deserialize)]
struct EntryNode {
    name: String,
    parent: usize,
}

#[derive(Debug, Serialize, Deserialize)]
struct EntryStatusVec {
    size: Option<Vec<u64>>,
    mode: Option<Vec<Mode>>,
    created: Option<Vec<SystemTime>>,
    modified: Option<Vec<SystemTime>>,
    accessed: Option<Vec<SystemTime>>,
}

impl EntryStatusVec {
    fn new(index_flags: &IndexFlags) -> Self {
        Self {
            size: if index_flags.size {
                Some(Vec::new())
            } else {
                None
            },
            mode: if index_flags.mode {
                Some(Vec::new())
            } else {
                None
            },
            created: if index_flags.created {
                Some(Vec::new())
            } else {
                None
            },
            modified: if index_flags.modified {
                Some(Vec::new())
            } else {
                None
            },
            accessed: if index_flags.accessed {
                Some(Vec::new())
            } else {
                None
            },
        }
    }
}

impl EntryStatusVec {
    fn push(&mut self, status: &EntryStatus) {
        if let Some(size) = &mut self.size {
            size.push(status.size.unwrap());
        }
        if let Some(mode) = &mut self.mode {
            mode.push(status.mode.unwrap());
        }
        if let Some(created) = &mut self.created {
            created.push(status.created.unwrap());
        }
        if let Some(modified) = &mut self.modified {
            modified.push(status.modified.unwrap());
        }
        if let Some(accessed) = &mut self.accessed {
            accessed.push(status.accessed.unwrap());
        }
    }
}

struct IndexFlags {
    size: bool,
    mode: bool,
    created: bool,
    modified: bool,
    accessed: bool,
    ignore_hidden: bool,
}

#[derive(Serialize, Deserialize)]
struct EntryStatus {
    size: Option<u64>,
    mode: Option<Mode>,
    created: Option<SystemTime>,
    modified: Option<SystemTime>,
    accessed: Option<SystemTime>,
}

impl EntryStatus {
    fn from_metadata(metadata: &Metadata, index_flags: &IndexFlags) -> Result<Self> {
        let size = if index_flags.size && !metadata.is_dir() {
            Some(metadata.len())
        } else {
            None
        };
        let mode = if index_flags.mode {
            Some(metadata.into())
        } else {
            None
        };

        let created = if index_flags.created {
            Some(sanitize_system_time(&metadata.created()?)?)
        } else {
            None
        };
        let modified = if index_flags.modified {
            Some(sanitize_system_time(&metadata.modified()?)?)
        } else {
            None
        };
        let accessed = if index_flags.accessed {
            Some(sanitize_system_time(&metadata.accessed()?)?)
        } else {
            None
        };

        let status = Self {
            size,
            mode,
            created,
            modified,
            accessed,
        };
        Ok(status)
    }
}

fn sanitize_system_time(time: &SystemTime) -> Result<SystemTime> {
    // metadata may contain invalid SystemTime
    // it will catch them as Err instead of panic
    Ok(SystemTime::UNIX_EPOCH + time.duration_since(SystemTime::UNIX_EPOCH)?)
}

struct EntryPrecursor {
    name: String,
    ftype: FileType,
    status: EntryStatus,
}

impl EntryPrecursor {
    fn from_path(path: &Path, index_flags: &IndexFlags) -> Result<Self> {
        let name = if path.parent().is_some() {
            path.file_name().ok_or(Error::Filename)?.to_str()
        } else {
            path.to_str()
        };
        let name = name.ok_or(Error::Utf8)?.to_string();

        let metadata = path.metadata()?;
        let ftype = metadata.file_type();

        Ok(Self {
            name,
            ftype,
            status: EntryStatus::from_metadata(&metadata, index_flags)?,
        })
    }

    fn from_dir_entry(dent: &DirEntry, index_flags: &IndexFlags) -> Result<Self> {
        let name = dent.file_name().to_str().ok_or(Error::Utf8)?.to_string();

        Ok(Self {
            name,
            ftype: dent.file_type()?,
            status: EntryStatus::from_metadata(&dent.metadata()?, index_flags)?,
        })
    }
}

fn walk_file_system(
    database: Arc<Mutex<Database>>,
    index_flags: &IndexFlags,
    path: &Path,
    parent: usize,
) {
    if let Ok(rd) = path.read_dir() {
        let children = rd
            .filter_map(|dent| {
                dent.ok().as_ref().and_then(|dent| {
                    if index_flags.ignore_hidden && is_hidden(dent) {
                        return None;
                    }
                    EntryPrecursor::from_dir_entry(&dent, index_flags)
                        .ok()
                        .map(|precursor| (dent.path(), precursor))
                })
            })
            .collect::<Vec<_>>();

        let mut sub_directories = Vec::new();
        {
            let mut db = database.lock().unwrap();
            for (path, precursor) in children {
                if precursor.ftype.is_dir() {
                    sub_directories.push((path, db.dirs.len()));
                    db.push_dir(precursor, parent);
                } else {
                    db.push_file(precursor, parent);
                }
            }
        }

        sub_directories
            .par_iter()
            .for_each_with(database, |database, (path, index)| {
                walk_file_system(database.clone(), index_flags, path, *index);
            });
    }
}

// taken from https://github.com/BurntSushi/ripgrep/blob/1b2c1dc67583d70d1d16fc93c90db80bead4fb09/crates/ignore/src/pathutil.rs#L6-L46

#[cfg(unix)]
fn is_hidden(dent: &DirEntry) -> bool {
    use std::os::unix::ffi::OsStrExt;

    if let Some(name) = dent.path().file_name() {
        name.as_bytes().get(0) == Some(&b'.')
    } else {
        false
    }
}

#[cfg(windows)]
fn is_hidden(dent: &DirEntry) -> bool {
    if let Ok(metadata) = dent.metadata() {
        if Mode::from(&metadata).is_hidden() {
            return true;
        }
    }
    if let Some(name) = dent.path().file_name() {
        name.to_str().map(|s| s.starts_with('.')).unwrap_or(false)
    } else {
        false
    }
}
