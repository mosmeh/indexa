use crate::matcher::Matcher;
use crate::mode::Mode;
use crate::{Error, Result};
use itertools::Itertools;
use rayon::prelude::*;
use serde::{Deserialize, Serialize};
use std::borrow::Cow;
use std::cmp;
use std::fmt;
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
    root_ids: Vec<u32>,
    basename_sort_key: Option<Vec<u32>>,
    path_sort_key: Option<Vec<u32>>,
    extension_sort_key: Option<Vec<u32>>,
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

    pub fn is_indexed(&self, kind: StatusKind) -> bool {
        match kind {
            StatusKind::Basename => self.basename_sort_key.is_some(),
            StatusKind::FullPath => self.path_sort_key.is_some(),
            StatusKind::Extension => self.extension_sort_key.is_some(),
            StatusKind::Size => self.size.is_some(),
            StatusKind::Mode => self.mode.is_some(),
            StatusKind::Created => self.created.is_some(),
            StatusKind::Modified => self.modified.is_some(),
            StatusKind::Accessed => self.accessed.is_some(),
        }
    }

    pub fn search(&self, matcher: &Matcher, aborted: Arc<AtomicBool>) -> Result<Vec<EntryId>> {
        if matcher.match_path {
            self.match_path(matcher, aborted)
        } else {
            self.match_basename(matcher, aborted)
        }
    }

    #[inline]
    pub fn entry(&self, id: EntryId) -> Entry<'_> {
        Entry { database: self, id }
    }

    /// Compares two entries by specified status using indexed information.
    ///
    /// Returns `None` if it cannot perform the fast comparison by specified status.
    #[inline]
    pub fn fast_compare(
        &self,
        kind: StatusKind,
        a: &Entry<'_>,
        b: &Entry<'_>,
    ) -> Option<cmp::Ordering> {
        match kind {
            StatusKind::Basename => Database::compare_entries(&self.basename_sort_key, a, b),
            StatusKind::FullPath => Database::compare_entries(&self.path_sort_key, a, b),
            StatusKind::Extension => Database::compare_entries(&self.extension_sort_key, a, b),
            StatusKind::Size => Database::compare_entries(&self.size, a, b),
            StatusKind::Mode => Database::compare_entries(&self.mode, a, b),
            StatusKind::Created => Database::compare_entries(&self.created, a, b),
            StatusKind::Modified => Database::compare_entries(&self.modified, a, b),
            StatusKind::Accessed => Database::compare_entries(&self.accessed, a, b),
        }
    }

    #[inline]
    fn compare_entries<T>(key: &Option<Vec<T>>, a: &Entry, b: &Entry) -> Option<cmp::Ordering>
    where
        T: Ord,
    {
        key.as_ref()
            .map(|x| x[a.id.0 as usize].cmp(&x[b.id.0 as usize]))
    }

    fn match_basename(&self, matcher: &Matcher, aborted: Arc<AtomicBool>) -> Result<Vec<EntryId>> {
        (0..self.entries.len() as u32)
            .into_par_iter()
            .zip(self.entries.par_iter())
            .filter_map(|(i, node)| {
                if aborted.load(Ordering::Relaxed) {
                    Some(Err(Error::SearchAbort))
                } else if matcher.query.is_match(&node.name) {
                    Some(Ok(EntryId(i)))
                } else {
                    None
                }
            })
            .collect()
    }

    fn match_path(&self, matcher: &Matcher, aborted: Arc<AtomicBool>) -> Result<Vec<EntryId>> {
        let mut hits = Vec::with_capacity(self.entries.len());
        for _ in 0..self.entries.len() {
            hits.push(AtomicBool::new(false));
        }

        for (root_id, next_root_id) in self
            .root_ids
            .iter()
            .copied()
            .chain(std::iter::once(self.entries.len() as u32))
            .tuple_windows()
        {
            let root_node = &self.entries[root_id as usize];
            if matcher.query.is_match(&root_node.name) {
                (root_id..next_root_id).into_par_iter().try_for_each(|id| {
                    if aborted.load(Ordering::Relaxed) {
                        return Err(Error::SearchAbort);
                    }
                    hits[id as usize].store(true, Ordering::Relaxed);
                    Ok(())
                })?;
            } else {
                self.match_path_impl(
                    root_node,
                    Path::new(&root_node.name),
                    matcher,
                    &hits,
                    aborted.clone(),
                )?;
            }
        }

        Ok((0..self.entries.len() as u32)
            .into_par_iter()
            .filter(|id| hits[*id as usize].load(Ordering::Relaxed))
            .map(EntryId)
            .collect())
    }

    fn match_path_impl(
        &self,
        node: &EntryNode,
        path: &Path,
        matcher: &Matcher,
        hits: &[AtomicBool],
        aborted: Arc<AtomicBool>,
    ) -> Result<()> {
        (node.children_start..node.children_end)
            .into_par_iter()
            .try_for_each(|id| {
                if aborted.load(Ordering::Relaxed) {
                    return Err(Error::SearchAbort);
                }

                let child = &self.entries[id as usize];
                let child_path = path.join(&child.name);
                if let Some(s) = child_path.to_str() {
                    if matcher.query.is_match(s) {
                        hits[id as usize].store(true, Ordering::Relaxed);

                        if child.is_dir {
                            self.match_all_descendants(child, hits, aborted.clone())?;
                        }
                    } else if child.is_dir {
                        self.match_path_impl(child, &child_path, matcher, hits, aborted.clone())?;
                    }
                }

                Ok(())
            })
    }

    fn match_all_descendants(
        &self,
        node: &EntryNode,
        hits: &[AtomicBool],
        aborted: Arc<AtomicBool>,
    ) -> Result<()> {
        (node.children_start..node.children_end)
            .into_par_iter()
            .try_for_each(|id| {
                if aborted.load(Ordering::Relaxed) {
                    return Err(Error::SearchAbort);
                }

                hits[id as usize].store(true, Ordering::Relaxed);

                let child = &self.entries[id as usize];
                if child.is_dir && child.children_start < child.children_end {
                    self.match_all_descendants(child, hits, aborted.clone())?;
                }

                Ok(())
            })
    }

    fn push_entry(&mut self, precursor: EntryPrecursor, parent: u32) {
        self.entries.push(EntryNode {
            name: precursor.name,
            parent,
            children_start: u32::MAX,
            children_end: u32::MAX,
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

    fn path_vec_from_node<'a>(&'a self, node: &'a EntryNode) -> Vec<&'a str> {
        let mut buf = Vec::new();
        self.path_vec_from_node_impl(node.parent, &mut buf);
        buf.push(&node.name);
        buf
    }

    fn path_vec_from_node_impl<'a>(&'a self, index: u32, mut buf: &mut Vec<&'a str>) {
        let dir = &self.entries[index as usize];
        if dir.parent == index {
            // root node
            buf.push(&self.entries[dir.parent as usize].name);
        } else {
            self.path_vec_from_node_impl(dir.parent, &mut buf);
        }
        buf.push(&dir.name);
    }
}

pub struct DatabaseBuilder {
    dirs: Vec<PathBuf>,
    index_flags: IndexFlags,
    fast_sort_flags: FastSortFlags,
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
            fast_sort_flags: FastSortFlags {
                basename: true,
                path: false,
                extension: false,
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

    pub fn add_status(&mut self, kind: StatusKind) -> &mut Self {
        match kind {
            StatusKind::Basename => self.fast_sort_flags.basename = true,
            StatusKind::FullPath => self.fast_sort_flags.path = true,
            StatusKind::Extension => self.fast_sort_flags.extension = true,
            StatusKind::Size => self.index_flags.size = true,
            StatusKind::Mode => self.index_flags.mode = true,
            StatusKind::Created => self.index_flags.created = true,
            StatusKind::Modified => self.index_flags.modified = true,
            StatusKind::Accessed => self.index_flags.accessed = true,
        };
        self
    }

    pub fn ignore_hidden(&mut self, yes: bool) -> &mut Self {
        self.index_flags.ignore_hidden = yes;
        self
    }

    pub fn build(&self) -> Result<Database> {
        let database = Database {
            entries: Vec::new(),
            root_ids: Vec::new(),
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
            basename_sort_key: None,
            path_sort_key: None,
            extension_sort_key: None,
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
                db.root_ids.push(root_node_id);

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
        let mut database = Arc::try_unwrap(database).unwrap().into_inner().unwrap();

        if self.fast_sort_flags.basename {
            database.basename_sort_key = Some(generate_sort_keys(&database, |a, b| {
                a.basename().cmp(b.basename())
            }));
        }
        if self.fast_sort_flags.path {
            database.path_sort_key = Some(generate_sort_keys(&database, |a, b| {
                a.path_vec().cmp(&b.path_vec())
            }));
        }
        if self.fast_sort_flags.extension {
            database.extension_sort_key = Some(generate_sort_keys(&database, |a, b| {
                a.extension().cmp(&b.extension())
            }));
        }

        Ok(database)
    }
}

#[derive(Copy, Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum StatusKind {
    Basename,
    #[serde(rename = "path")]
    FullPath,
    Extension,
    Size,
    Mode,
    Created,
    Modified,
    Accessed,
}

impl fmt::Display for StatusKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            StatusKind::FullPath => f.write_str("Path"),
            StatusKind::Basename => f.write_str("Basename"),
            StatusKind::Size => f.write_str("Size"),
            StatusKind::Mode => f.write_str("Mode"),
            StatusKind::Extension => f.write_str("Extension"),
            StatusKind::Created => f.write_str("Created"),
            StatusKind::Modified => f.write_str("Modified"),
            StatusKind::Accessed => f.write_str("Accessed"),
        }
    }
}

#[derive(Debug, Copy, Clone)]
pub struct EntryId(u32);

pub struct Entry<'a> {
    database: &'a Database,
    id: EntryId,
}

impl<'a> Entry<'a> {
    #[inline]
    pub fn is_dir(&self) -> bool {
        self.node().is_dir
    }

    #[inline]
    pub fn basename(&self) -> &str {
        &self.node().name
    }

    #[inline]
    pub fn path(&self) -> PathBuf {
        self.database.path_from_node(&self.node())
    }

    #[inline]
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

    #[inline]
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

    #[inline]
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

    #[inline]
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

    #[inline]
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

    #[inline]
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

    #[inline]
    fn path_vec(&'a self) -> Vec<&'a str> {
        self.database.path_vec_from_node(&self.node())
    }

    #[inline]
    fn node(&self) -> &EntryNode {
        &self.database.entries[self.id.0 as usize]
    }
}

#[derive(Debug, Serialize, Deserialize)]
struct EntryNode {
    name: String,
    parent: u32,
    children_start: u32,
    children_end: u32,
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

struct FastSortFlags {
    basename: bool,
    path: bool,
    extension: bool,
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

fn generate_sort_keys<F>(database: &Database, compare_func: F) -> Vec<u32>
where
    F: Fn(&Entry, &Entry) -> std::cmp::Ordering + Send + Sync,
{
    let mut indices = (0..database.entries.len() as u32).collect::<Vec<_>>();
    indices
        .as_parallel_slice_mut()
        .par_sort_unstable_by(|a, b| {
            compare_func(&database.entry(EntryId(*a)), &database.entry(EntryId(*b)))
        });

    let mut sort_keys = vec![0; indices.len()];
    for (i, x) in indices.iter().enumerate() {
        sort_keys[*x as usize] = i as u32;
    }

    sort_keys
}

fn sanitize_system_time(time: &SystemTime) -> Result<SystemTime> {
    // metadata may contain invalid SystemTime
    // it will catch them as Err instead of panic
    Ok(SystemTime::UNIX_EPOCH + time.duration_since(SystemTime::UNIX_EPOCH)?)
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

        db.entries[parent as usize].children_start = db.entries.len() as u32;

        for mut precursor in children {
            if precursor.ftype.is_dir() {
                let dir_entries = mem::replace(&mut precursor.dir_entries, None).unwrap();
                sub_dirs.push((db.entries.len() as u32, dir_entries));
            }
            db.push_entry(precursor, parent);
        }

        db.entries[parent as usize].children_end = db.entries.len() as u32;
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
