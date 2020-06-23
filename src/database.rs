use crate::mode::Mode;
use crate::query::{Query, SortOrder};
use crate::{Error, Result};
use enum_map::{enum_map, Enum, EnumMap};
use itertools::Itertools;
use rayon::prelude::*;
use regex::Regex;
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
    size: Option<Vec<u64>>,
    mode: Option<Vec<Mode>>,
    created: Option<Vec<SystemTime>>,
    modified: Option<Vec<SystemTime>>,
    accessed: Option<Vec<SystemTime>>,
    sorted_ids: EnumMap<StatusKind, Option<Vec<u32>>>,
}

impl Database {
    #[inline]
    pub fn num_entries(&self) -> usize {
        self.entries.len()
    }

    #[inline]
    pub fn is_indexed(&self, kind: StatusKind) -> bool {
        match kind {
            StatusKind::Basename | StatusKind::FullPath | StatusKind::Extension => true,
            StatusKind::Size => self.size.is_some(),
            StatusKind::Mode => self.mode.is_some(),
            StatusKind::Created => self.created.is_some(),
            StatusKind::Modified => self.modified.is_some(),
            StatusKind::Accessed => self.accessed.is_some(),
        }
    }

    #[inline]
    pub fn is_fast_sortable(&self, kind: StatusKind) -> bool {
        self.sorted_ids[kind].is_some()
    }

    pub fn search(&self, query: &Query, aborted: Arc<AtomicBool>) -> Result<Vec<EntryId>> {
        if query.match_path() {
            self.match_path(query, aborted)
        } else {
            self.match_basename(query, aborted)
        }
    }

    #[inline]
    pub fn entry(&self, id: EntryId) -> Entry<'_> {
        Entry { database: self, id }
    }

    fn match_basename(&self, query: &Query, aborted: Arc<AtomicBool>) -> Result<Vec<EntryId>> {
        self.collect_hits(query, |(id, node)| {
            if aborted.load(Ordering::Relaxed) {
                return Some(Err(Error::SearchAbort));
            }

            if query.regex().is_match(&node.name) {
                Some(Ok(EntryId(id)))
            } else {
                None
            }
        })
    }

    fn match_path(&self, query: &Query, aborted: Arc<AtomicBool>) -> Result<Vec<EntryId>> {
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
            if query.regex().is_match(&root_node.name) {
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
                    &query.regex(),
                    &hits,
                    aborted.clone(),
                )?;
            }
        }

        self.collect_hits(query, |(id, _)| {
            if aborted.load(Ordering::Relaxed) {
                return Some(Err(Error::SearchAbort));
            }

            if hits[id as usize].load(Ordering::Relaxed) {
                Some(Ok(EntryId(id)))
            } else {
                None
            }
        })
    }

    fn match_path_impl(
        &self,
        node: &EntryNode,
        path: &Path,
        regex: &Regex,
        hits: &[AtomicBool],
        aborted: Arc<AtomicBool>,
    ) -> Result<()> {
        (node.child_start..node.child_end)
            .into_par_iter()
            .try_for_each(|id| {
                if aborted.load(Ordering::Relaxed) {
                    return Err(Error::SearchAbort);
                }

                let child = &self.entries[id as usize];
                let child_path = path.join(&child.name);
                if let Some(s) = child_path.to_str() {
                    if regex.is_match(s) {
                        hits[id as usize].store(true, Ordering::Relaxed);

                        if child.has_any_child() {
                            self.match_all_descendants(child, hits, aborted.clone())?;
                        }
                    } else if child.has_any_child() {
                        self.match_path_impl(child, &child_path, regex, hits, aborted.clone())?;
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
        (node.child_start..node.child_end)
            .into_par_iter()
            .try_for_each(|id| {
                if aborted.load(Ordering::Relaxed) {
                    return Err(Error::SearchAbort);
                }

                hits[id as usize].store(true, Ordering::Relaxed);

                let child = &self.entries[id as usize];
                if child.has_any_child() {
                    self.match_all_descendants(child, hits, aborted.clone())?;
                }

                Ok(())
            })
    }

    fn collect_hits<F>(&self, query: &Query, func: F) -> Result<Vec<EntryId>>
    where
        F: Fn((u32, &EntryNode)) -> Option<Result<EntryId>> + Send + Sync,
    {
        let hits: Result<Vec<_>> = if self.is_fast_sortable(query.sort_by()) {
            let iter = self.sorted_ids[query.sort_by()]
                .as_ref()
                .unwrap()
                .par_iter()
                .map(|id| (*id, &self.entries[*id as usize]));
            match query.sort_order() {
                SortOrder::Ascending => iter.filter_map(func).collect(),
                SortOrder::Descending => iter.rev().filter_map(func).collect(),
            }
        } else {
            let mut v = (0..self.entries.len() as u32)
                .into_par_iter()
                .zip(self.entries.par_iter())
                .filter_map(func)
                .collect::<Result<Vec<_>>>()?;

            let compare_func = build_compare_func(query.sort_by());
            match query.sort_order() {
                SortOrder::Ascending => v
                    .as_parallel_slice_mut()
                    .par_sort_unstable_by(|a, b| compare_func(&self.entry(*a), &self.entry(*b))),
                SortOrder::Descending => v
                    .as_parallel_slice_mut()
                    .par_sort_unstable_by(|a, b| compare_func(&self.entry(*b), &self.entry(*a))),
            };

            Ok(v)
        };

        if query.dirs_before_files() {
            hits.map(|mut hits| {
                hits.as_parallel_slice_mut().par_sort_by(|a, b| {
                    self.entries[b.0 as usize]
                        .is_dir
                        .cmp(&self.entries[a.0 as usize].is_dir)
                });
                hits
            })
        } else {
            hits
        }
    }

    fn push_entry(&mut self, info: EntryInfo, parent: u32) {
        self.entries.push(EntryNode {
            name: info.name,
            parent,
            child_start: u32::MAX,
            child_end: u32::MAX,
            is_dir: info.ftype.is_dir(),
        });

        if let Some(size) = &mut self.size {
            size.push(info.status.size.unwrap());
        }
        if let Some(mode) = &mut self.mode {
            mode.push(info.status.mode.unwrap());
        }
        if let Some(created) = &mut self.created {
            created.push(info.status.created.unwrap());
        }
        if let Some(modified) = &mut self.modified {
            modified.push(info.status.modified.unwrap());
        }
        if let Some(accessed) = &mut self.accessed {
            accessed.push(info.status.accessed.unwrap());
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
    index_flags: StatusFlags,
    fast_sort_flags: StatusFlags,
    ignore_hidden: bool,
}

impl Default for DatabaseBuilder {
    fn default() -> Self {
        Self {
            dirs: Vec::new(),
            index_flags: enum_map! {
                StatusKind::Basename => true,
                StatusKind::FullPath => true,
                StatusKind::Extension => true,
                StatusKind::Size => false,
                StatusKind::Mode => false,
                StatusKind::Created => false,
                StatusKind::Modified => false,
                StatusKind::Accessed => false,
            },
            fast_sort_flags: enum_map! {
                StatusKind::Basename => true,
                StatusKind::FullPath => false,
                StatusKind::Extension => false,
                StatusKind::Size => false,
                StatusKind::Mode => false,
                StatusKind::Created => false,
                StatusKind::Modified => false,
                StatusKind::Accessed => false,
            },
            ignore_hidden: false,
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

    pub fn index(&mut self, kind: StatusKind) -> &mut Self {
        self.index_flags[kind] = true;
        self
    }

    pub fn fast_sort(&mut self, kind: StatusKind) -> &mut Self {
        self.fast_sort_flags[kind] = true;
        self
    }

    pub fn ignore_hidden(&mut self, yes: bool) -> &mut Self {
        self.ignore_hidden = yes;
        self
    }

    pub fn build(&self) -> Result<Database> {
        for (kind, enabled) in self.fast_sort_flags {
            if enabled && !self.index_flags[kind] {
                return Err(Error::InvalidOption(
                    "Fast sorting cannot be enabled for a non-indexed status.".to_string(),
                ));
            }
        }

        let database = Database {
            entries: Vec::new(),
            root_ids: Vec::new(),
            size: if self.index_flags[StatusKind::Size] {
                Some(Vec::new())
            } else {
                None
            },
            mode: if self.index_flags[StatusKind::Mode] {
                Some(Vec::new())
            } else {
                None
            },
            created: if self.index_flags[StatusKind::Created] {
                Some(Vec::new())
            } else {
                None
            },
            modified: if self.index_flags[StatusKind::Modified] {
                Some(Vec::new())
            } else {
                None
            },
            accessed: if self.index_flags[StatusKind::Accessed] {
                Some(Vec::new())
            } else {
                None
            },
            sorted_ids: EnumMap::new(),
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
            let mut root_info = EntryInfo::from_path(&path, &self.index_flags)?;
            if !root_info.ftype.is_dir() {
                continue;
            }

            let dir_entries = mem::replace(&mut root_info.dir_entries, None).unwrap();

            let root_node_id = {
                let mut db = database.lock().unwrap();

                let root_node_id = db.entries.len() as u32;
                db.push_entry(
                    EntryInfo {
                        name: path_str.to_string(),
                        ..root_info
                    },
                    root_node_id,
                );
                db.root_ids.push(root_node_id);

                root_node_id
            };

            walk_file_system(
                database.clone(),
                &self.index_flags,
                self.ignore_hidden,
                &dir_entries,
                root_node_id,
            );
        }

        // safe to unwrap since above codes are the only users of database at the moment
        let mut database = Arc::try_unwrap(database).unwrap().into_inner().unwrap();

        database.sorted_ids =
            generate_sorted_ids(&database, &self.index_flags, &self.fast_sort_flags);

        Ok(database)
    }
}

#[derive(Copy, Clone, Debug, PartialEq, Serialize, Deserialize, Enum)]
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
                    .map(|created| Cow::Owned(sanitize_system_time(&created)))
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
                    .map(|modified| Cow::Owned(sanitize_system_time(&modified)))
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
                    .map(|accessed| Cow::Owned(sanitize_system_time(&accessed)))
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
    child_start: u32,
    child_end: u32,
    is_dir: bool,
}

impl EntryNode {
    #[inline]
    fn has_any_child(&self) -> bool {
        self.child_start < self.child_end
    }
}

type StatusFlags = EnumMap<StatusKind, bool>;

struct EntryStatus {
    size: Option<u64>,
    mode: Option<Mode>,
    created: Option<SystemTime>,
    modified: Option<SystemTime>,
    accessed: Option<SystemTime>,
}

impl EntryStatus {
    fn from_metadata(metadata: &Metadata, index_flags: &StatusFlags) -> Result<Self> {
        let size = if index_flags[StatusKind::Size] {
            Some(metadata.len())
        } else {
            None
        };

        Self::from_metadata_with_size(size, metadata, index_flags)
    }

    fn from_metadata_with_size(
        size: Option<u64>,
        metadata: &Metadata,
        index_flags: &StatusFlags,
    ) -> Result<Self> {
        let mode = if index_flags[StatusKind::Mode] {
            Some(metadata.into())
        } else {
            None
        };

        let created = if index_flags[StatusKind::Created] {
            Some(sanitize_system_time(&metadata.created()?))
        } else {
            None
        };
        let modified = if index_flags[StatusKind::Modified] {
            Some(sanitize_system_time(&metadata.modified()?))
        } else {
            None
        };
        let accessed = if index_flags[StatusKind::Accessed] {
            Some(sanitize_system_time(&metadata.accessed()?))
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

struct EntryInfo {
    name: String,
    ftype: FileType,
    status: EntryStatus,
    dir_entries: Option<Vec<io::Result<DirEntry>>>,
}

impl EntryInfo {
    fn from_path(path: &Path, index_flags: &StatusFlags) -> Result<Self> {
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

    fn from_dir_entry(dent: &DirEntry, index_flags: &StatusFlags) -> Result<Self> {
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

fn generate_sorted_ids(
    database: &Database,
    index_flags: &StatusFlags,
    fast_sort_flags: &StatusFlags,
) -> EnumMap<StatusKind, Option<Vec<u32>>> {
    let mut sorted_ids = EnumMap::new();
    for (kind, key) in sorted_ids.iter_mut() {
        if index_flags[kind] && fast_sort_flags[kind] {
            let compare_func = build_compare_func(kind);

            let mut indices = (0..database.entries.len() as u32).collect::<Vec<_>>();
            indices
                .as_parallel_slice_mut()
                .par_sort_unstable_by(|a, b| {
                    compare_func(&database.entry(EntryId(*a)), &database.entry(EntryId(*b)))
                });

            *key = Some(indices);
        }
    }
    sorted_ids
}

fn build_compare_func(
    kind: StatusKind,
) -> Box<dyn Fn(&Entry, &Entry) -> cmp::Ordering + Send + Sync> {
    match kind {
        StatusKind::Basename => Box::new(|a, b| a.basename().cmp(b.basename())),
        StatusKind::FullPath => Box::new(|a, b| a.path_vec().cmp(&b.path_vec())),
        StatusKind::Extension => Box::new(|a, b| {
            a.extension()
                .cmp(&b.extension())
                .then_with(|| a.basename().cmp(b.basename()))
        }),
        StatusKind::Size => Box::new(|a, b| {
            a.size()
                .cmp(&b.size())
                .then_with(|| a.basename().cmp(b.basename()))
        }),
        StatusKind::Mode => Box::new(|a, b| {
            a.mode()
                .cmp(&b.mode())
                .then_with(|| a.basename().cmp(b.basename()))
        }),
        StatusKind::Created => Box::new(|a, b| {
            a.created()
                .cmp(&b.created())
                .then_with(|| a.basename().cmp(b.basename()))
        }),
        StatusKind::Modified => Box::new(|a, b| {
            a.modified()
                .cmp(&b.modified())
                .then_with(|| a.basename().cmp(b.basename()))
        }),
        StatusKind::Accessed => Box::new(|a, b| {
            a.accessed()
                .cmp(&b.accessed())
                .then_with(|| a.basename().cmp(b.basename()))
        }),
    }
}

fn sanitize_system_time(time: &SystemTime) -> SystemTime {
    // check for invalid SystemTime (e.g. older than unix epoch)
    if let Ok(duration) = time.duration_since(SystemTime::UNIX_EPOCH) {
        SystemTime::UNIX_EPOCH + duration
    } else {
        // defaults to unix epoch
        SystemTime::UNIX_EPOCH
    }
}

fn walk_file_system(
    database: Arc<Mutex<Database>>,
    index_flags: &StatusFlags,
    ignore_hidden: bool,
    dir_entries: &[io::Result<DirEntry>],
    parent: u32,
) {
    let (mut child_dirs, child_files) = dir_entries
        .iter()
        .filter_map(|dent| {
            dent.as_ref().ok().and_then(|dent| {
                if ignore_hidden && is_hidden(dent) {
                    return None;
                }
                EntryInfo::from_dir_entry(&dent, index_flags).ok()
            })
        })
        .partition::<Vec<_>, _>(|info| info.ftype.is_dir());

    let sub_dir_entries = child_dirs
        .iter_mut()
        .map(|info| mem::replace(&mut info.dir_entries, None).unwrap())
        .collect::<Vec<_>>();

    let (dir_start, dir_end) = {
        let mut db = database.lock().unwrap();

        let child_start = db.entries.len() as u32;
        let dir_end = child_start + child_dirs.len() as u32;
        let child_end = dir_end + child_files.len() as u32;

        db.entries[parent as usize].child_start = child_start;
        db.entries[parent as usize].child_end = child_end;

        for info in child_dirs {
            db.push_entry(info, parent);
        }
        for info in child_files {
            db.push_entry(info, parent);
        }

        (child_start, dir_end)
    };

    (dir_start..dir_end)
        .into_par_iter()
        .zip(sub_dir_entries.par_iter())
        .for_each_with(database, |database, (index, dir_entries)| {
            walk_file_system(
                database.clone(),
                index_flags,
                ignore_hidden,
                &dir_entries,
                index,
            );
        });
}

// taken from https://github.com/BurntSushi/ripgrep/blob/1b2c1dc67583d70d1d16fc93c90db80bead4fb09/crates/ignore/src/pathutil.rs#L6-L46
#[cfg(unix)]
#[inline]
fn is_hidden(dent: &DirEntry) -> bool {
    use std::os::unix::ffi::OsStrExt;

    if let Some(name) = dent.path().file_name() {
        name.as_bytes().get(0) == Some(&b'.')
    } else {
        false
    }
}

#[cfg(windows)]
#[inline]
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
