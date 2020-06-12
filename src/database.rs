use crate::matcher::Matcher;
use crate::mode::Mode;
use crate::{Error, Result};
use rayon::prelude::*;
use serde::{Deserialize, Serialize};
use std::borrow::Cow;
use std::fs::{DirEntry, FileType, Metadata};
use std::io;
use std::mem;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::SystemTime;

#[derive(Debug, Serialize, Deserialize)]
pub struct Database {
    entries: Vec<EntryNode>,
    size: Option<Vec<u64>>,
    mode: Option<Vec<Mode>>,
    created: Option<Vec<SystemTime>>,
    modified: Option<Vec<SystemTime>>,
    accessed: Option<Vec<SystemTime>>,
}

impl Database {
    pub fn num_entries(&self) -> usize {
        self.entries.len()
    }

    pub fn search(&self, matcher: &Matcher) -> Vec<EntryId> {
        (0..self.entries.len() as u32)
            .into_par_iter()
            .zip(self.entries.par_iter())
            .filter_map(|(i, node): (u32, &EntryNode)| {
                if self.node_matches(node, matcher) {
                    Some(EntryId(i))
                } else {
                    None
                }
            })
            .collect()
    }

    pub fn abortable_search(
        &self,
        matcher: &Matcher,
        aborted: Arc<AtomicBool>,
    ) -> Result<Vec<EntryId>> {
        (0..self.entries.len() as u32)
            .into_par_iter()
            .zip(self.entries.par_iter())
            .filter_map(|(i, node): (u32, &EntryNode)| {
                if aborted.load(Ordering::Relaxed) {
                    Some(Err(Error::SearchAbort))
                } else if self.node_matches(node, matcher) {
                    Some(Ok(EntryId(i)))
                } else {
                    None
                }
            })
            .collect()
    }

    pub fn entry<'a, 'b>(&'a self, id: &'b EntryId) -> Entry<'a, 'b> {
        Entry { database: self, id }
    }

    fn node_matches(&self, node: &EntryNode, matcher: &Matcher) -> bool {
        if matcher.match_path {
            if let Some(path) = self.path_from_node(node).to_str() {
                if matcher.query.is_match(path) {
                    return true;
                }
            }
        } else if matcher.query.is_match(&node.name) {
            return true;
        }
        false
    }

    fn push_entry(&mut self, precursor: EntryPrecursor, parent: u32) {
        self.entries.push(EntryNode {
            name: precursor.name,
            parent,
            is_dir: precursor.ftype.is_dir(),
        });

        if let Some(size) = &mut self.size {
            size.push(precursor.status.size.unwrap());
        }
        if let Some(mode) = &mut self.mode {
            mode.push(precursor.status.mode.unwrap());
        }
        if let Some(created) = &mut self.created {
            created.push(precursor.status.created.unwrap());
        }
        if let Some(modified) = &mut self.modified {
            modified.push(precursor.status.modified.unwrap());
        }
        if let Some(accessed) = &mut self.accessed {
            accessed.push(precursor.status.accessed.unwrap());
        }
    }

    fn path_from_node(&self, node: &EntryNode) -> PathBuf {
        let mut buf = PathBuf::new();
        self.path_from_node_impl(node.parent, &mut buf);
        buf.push(&node.name);
        buf
    }

    fn path_from_node_impl(&self, index: u32, mut buf: &mut PathBuf) {
        let dir = &self.entries[index as usize];
        if dir.parent == index {
            // root node
            buf.push(&self.entries[dir.parent as usize].name);
        } else {
            self.path_from_node_impl(dir.parent, &mut buf);
        }
        buf.push(&dir.name);
    }
}

pub struct DatabaseBuilder {
    dirs: Vec<PathBuf>,
    index_flags: IndexFlags,
}

impl Default for DatabaseBuilder {
    fn default() -> Self {
        Self {
            dirs: Vec::new(),
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
}

impl DatabaseBuilder {
    pub fn new() -> Self {
        Default::default()
    }

    pub fn add_dir<P: AsRef<Path>>(&mut self, path: P) -> &mut Self {
        self.dirs.push(path.as_ref().to_path_buf());
        self
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
        let database = Database {
            entries: Vec::new(),
            size: if self.index_flags.size {
                Some(Vec::new())
            } else {
                None
            },
            mode: if self.index_flags.mode {
                Some(Vec::new())
            } else {
                None
            },
            created: if self.index_flags.created {
                Some(Vec::new())
            } else {
                None
            },
            modified: if self.index_flags.modified {
                Some(Vec::new())
            } else {
                None
            },
            accessed: if self.index_flags.accessed {
                Some(Vec::new())
            } else {
                None
            },
        };

        let database = Arc::new(Mutex::new(database));

        let mut dirs = self
            .dirs
            .iter()
            .filter_map(|path| {
                if let Ok(canonicalized) = dunce::canonicalize(path) {
                    if let Some(path_str) = canonicalized.to_str() {
                        let path_str = path_str.to_string();
                        return Some((canonicalized, path_str));
                    }
                }
                None
            })
            .collect::<Vec<_>>();

        // remove redundant subdirectories
        // we use str::starts_with, because Path::starts_with doesn't work well for Windows paths
        dirs.sort_unstable_by(|(_, a), (_, b)| a.cmp(b));
        dirs.dedup_by(|(_, a), (_, b)| a.starts_with(&b as &str));

        for (path, path_str) in &dirs {
            let mut root_precursor = EntryPrecursor::from_path(&path, &self.index_flags)?;
            if !root_precursor.ftype.is_dir() {
                continue;
            }

            let dir_entries = mem::replace(&mut root_precursor.dir_entries, None).unwrap();

            let root_node_id = {
                let mut db = database.lock().unwrap();

                let root_node_id = db.entries.len() as u32;
                db.push_entry(
                    EntryPrecursor {
                        name: path_str.to_string(),
                        ..root_precursor
                    },
                    root_node_id,
                );

                root_node_id
            };

            walk_file_system(
                database.clone(),
                &self.index_flags,
                &dir_entries,
                root_node_id,
            );
        }

        // safe to unwrap since above codes are the only users of database at the moment
        Ok(Arc::try_unwrap(database).unwrap().into_inner().unwrap())
    }
}

#[derive(Debug)]
pub struct EntryId(u32);

pub struct Entry<'a, 'b> {
    database: &'a Database,
    id: &'b EntryId,
}

impl<'a, 'b> Entry<'a, 'b> {
    pub fn is_dir(&self) -> bool {
        self.node().is_dir
    }

    pub fn basename(&self) -> &str {
        &self.node().name
    }

    pub fn path(&self) -> PathBuf {
        self.database.path_from_node(&self.node())
    }

    pub fn extension(&self) -> Option<&str> {
        let node = &self.node();
        if node.is_dir {
            return None;
        }

        let name = &node.name;
        if name.contains('.') {
            name.split('.').last()
        } else {
            None
        }
    }

    pub fn size(&self) -> Option<u64> {
        self.database
            .size
            .as_ref()
            .map(|v| v[self.id.0 as usize])
            .or_else(|| {
                if self.is_dir() {
                    self.path().read_dir().map(|rd| rd.count() as u64).ok()
                } else {
                    self.path().metadata().map(|metadata| metadata.len()).ok()
                }
            })
    }

    pub fn mode(&self) -> Option<Mode> {
        self.database
            .mode
            .as_ref()
            .map(|v| v[self.id.0 as usize])
            .or_else(|| {
                self.path()
                    .metadata()
                    .map(|metadata| Mode::from(&metadata))
                    .ok()
            })
    }

    pub fn created(&self) -> Option<Cow<'a, SystemTime>> {
        self.database
            .created
            .as_ref()
            .map(|v| Cow::Borrowed(&v[self.id.0 as usize]))
            .or_else(|| {
                self.path()
                    .metadata()
                    .ok()
                    .and_then(|metadata| metadata.created().ok())
                    .and_then(|created| sanitize_system_time(&created).ok())
                    .map(Cow::Owned)
            })
    }

    pub fn modified(&self) -> Option<Cow<'a, SystemTime>> {
        self.database
            .modified
            .as_ref()
            .map(|v| Cow::Borrowed(&v[self.id.0 as usize]))
            .or_else(|| {
                self.path()
                    .metadata()
                    .ok()
                    .and_then(|metadata| metadata.modified().ok())
                    .and_then(|modified| sanitize_system_time(&modified).ok())
                    .map(Cow::Owned)
            })
    }

    pub fn accessed(&self) -> Option<Cow<'a, SystemTime>> {
        self.database
            .accessed
            .as_ref()
            .map(|v| Cow::Borrowed(&v[self.id.0 as usize]))
            .or_else(|| {
                self.path()
                    .metadata()
                    .ok()
                    .and_then(|metadata| metadata.accessed().ok())
                    .and_then(|accessed| sanitize_system_time(&accessed).ok())
                    .map(Cow::Owned)
            })
    }

    fn node(&self) -> &EntryNode {
        &self.database.entries[self.id.0 as usize]
    }
}

#[derive(Debug, Serialize, Deserialize)]
struct EntryNode {
    name: String,
    parent: u32,
    is_dir: bool,
}

struct IndexFlags {
    size: bool,
    mode: bool,
    created: bool,
    modified: bool,
    accessed: bool,
    ignore_hidden: bool,
}

struct EntryStatus {
    size: Option<u64>,
    mode: Option<Mode>,
    created: Option<SystemTime>,
    modified: Option<SystemTime>,
    accessed: Option<SystemTime>,
}

impl EntryStatus {
    fn from_metadata(metadata: &Metadata, index_flags: &IndexFlags) -> Result<Self> {
        let size = if index_flags.size {
            Some(metadata.len())
        } else {
            None
        };

        Self::from_metadata_with_size(size, metadata, index_flags)
    }

    fn from_metadata_with_size(
        size: Option<u64>,
        metadata: &Metadata,
        index_flags: &IndexFlags,
    ) -> Result<Self> {
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
    dir_entries: Option<Vec<io::Result<DirEntry>>>,
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

        if ftype.is_dir() {
            let dir_entries = path.read_dir()?.collect::<Vec<_>>();
            let num_children = dir_entries.len();

            Ok(Self {
                name,
                ftype,
                status: EntryStatus::from_metadata_with_size(
                    Some(num_children as u64),
                    &metadata,
                    index_flags,
                )?,
                dir_entries: Some(dir_entries),
            })
        } else {
            Ok(Self {
                name,
                ftype,
                status: EntryStatus::from_metadata(&metadata, index_flags)?,
                dir_entries: None,
            })
        }
    }

    fn from_dir_entry(dent: &DirEntry, index_flags: &IndexFlags) -> Result<Self> {
        let name = dent.file_name().to_str().ok_or(Error::Utf8)?.to_string();
        let ftype = dent.file_type()?;

        if ftype.is_dir() {
            let path = dent.path();

            let dir_entries = path.read_dir()?.collect::<Vec<_>>();
            let num_children = dir_entries.len();

            Ok(Self {
                name,
                ftype,
                status: EntryStatus::from_metadata_with_size(
                    Some(num_children as u64),
                    &dent.metadata()?,
                    index_flags,
                )?,
                dir_entries: Some(dir_entries),
            })
        } else {
            Ok(Self {
                name,
                ftype,
                status: EntryStatus::from_metadata(&dent.metadata()?, index_flags)?,
                dir_entries: None,
            })
        }
    }
}

fn walk_file_system(
    database: Arc<Mutex<Database>>,
    index_flags: &IndexFlags,
    dir_entries: &[io::Result<DirEntry>],
    parent: u32,
) {
    let children = dir_entries
        .iter()
        .filter_map(|dent| {
            dent.as_ref().ok().and_then(|dent| {
                if index_flags.ignore_hidden && is_hidden(dent) {
                    return None;
                }
                EntryPrecursor::from_dir_entry(&dent, index_flags).ok()
            })
        })
        .collect::<Vec<_>>();

    let mut sub_dirs = Vec::new();
    {
        let mut db = database.lock().unwrap();
        for mut precursor in children {
            if precursor.ftype.is_dir() {
                let dir_entries = mem::replace(&mut precursor.dir_entries, None).unwrap();
                sub_dirs.push((db.entries.len() as u32, dir_entries));
            }
            db.push_entry(precursor, parent);
        }
    }

    sub_dirs
        .par_iter()
        .for_each_with(database, |database, (index, dir_entries)| {
            walk_file_system(database.clone(), index_flags, dir_entries, *index);
        });
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
