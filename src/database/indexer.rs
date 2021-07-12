use super::{builder::StatusFlags, util, Database, EntryNode, StatusKind};
use crate::{mode::Mode, Error, Result};

use enum_map::{enum_map, EnumMap};
use parking_lot::Mutex;
use rayon::prelude::*;
use std::{
    collections::HashMap,
    fs::{self, FileType, Metadata},
    mem,
    ops::Range,
    path::{Path, PathBuf},
    time::SystemTime,
};

pub struct IndexOptions {
    pub index_flags: StatusFlags,
    pub ignore_hidden: bool,
}

impl Default for IndexOptions {
    fn default() -> Self {
        Self {
            index_flags: enum_map! {
                StatusKind::Basename => true,
                StatusKind::Path => true,
                StatusKind::Extension => true,
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

impl IndexOptions {
    #[inline]
    fn needs_metadata(&self) -> bool {
        let flags = &self.index_flags;
        flags[StatusKind::Size]
            || flags[StatusKind::Mode]
            || flags[StatusKind::Created]
            || flags[StatusKind::Modified]
            || flags[StatusKind::Accessed]
    }
}

pub struct Indexer<'a> {
    options: &'a IndexOptions,
    database: Database,
}

impl<'a> Indexer<'a> {
    pub fn new(options: &'a IndexOptions) -> Indexer<'a> {
        let database = Database {
            name_arena: String::new(),
            nodes: Vec::new(),
            root_paths: HashMap::new(),
            size: options.index_flags[StatusKind::Size].then(Vec::new),
            mode: options.index_flags[StatusKind::Mode].then(Vec::new),
            created: options.index_flags[StatusKind::Created].then(Vec::new),
            modified: options.index_flags[StatusKind::Modified].then(Vec::new),
            accessed: options.index_flags[StatusKind::Accessed].then(Vec::new),
            sorted_ids: EnumMap::default(),
        };

        Self { options, database }
    }

    pub fn index<P: AsRef<Path>>(mut self, path: P) -> Result<Self> {
        if let Ok(mut root_info) = EntryInfo::from_path(path.as_ref(), self.options) {
            if !root_info.ftype.is_dir() {
                return Ok(self);
            }

            let dir_entries = mem::take(&mut root_info.dir_entries);

            let root_node_id = self.database.nodes.len() as u32;
            self.database.push_entry(root_info, root_node_id);
            self.database
                .root_paths
                .insert(root_node_id, path.as_ref().to_path_buf());

            if !dir_entries.is_empty() {
                let database = Mutex::new(self.database);
                walk_file_system(&database, self.options, root_node_id, dir_entries);
                self.database = database.into_inner();
            }
        }

        Ok(self)
    }

    pub fn finish(self) -> Database {
        self.database
    }
}

impl Database {
    fn push_entry(&mut self, info: EntryInfo, parent_id: u32) {
        self.nodes.push(EntryNode {
            name_start: self.name_arena.len(),
            name_len: info.name.len() as u16,
            parent: parent_id,
            child_start: u32::MAX,
            child_end: u32::MAX,
            is_dir: info.ftype.is_dir(),
        });
        self.name_arena.push_str(&info.name);

        if let Some(status) = info.status {
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

    #[inline]
    fn set_children(&mut self, id: u32, range: Range<u32>) {
        let mut node = &mut self.nodes[id as usize];
        node.child_start = range.start;
        node.child_end = range.end;
    }
}

fn walk_file_system(
    database: &Mutex<Database>,
    options: &IndexOptions,
    parent_id: u32,
    dir_entries: Vec<DirEntry>,
) {
    let (mut child_dirs, child_files) = dir_entries
        .into_iter()
        .filter_map(|dent| EntryInfo::from_dir_entry(dent, options).ok())
        .partition::<Vec<_>, _>(|info| info.ftype.is_dir());

    if child_dirs.is_empty() && child_files.is_empty() {
        return;
    }

    let child_dir_entries: Vec<_> = child_dirs
        .iter_mut()
        .map(|info| mem::take(&mut info.dir_entries))
        .collect();

    let (dir_start, dir_end) = {
        let mut database = database.lock();

        let child_start = database.nodes.len() as u32;
        let dir_end = child_start + child_dirs.len() as u32;
        let child_end = dir_end + child_files.len() as u32;

        database.set_children(parent_id, child_start..child_end);
        for info in child_dirs {
            database.push_entry(info, parent_id);
        }
        for info in child_files {
            database.push_entry(info, parent_id);
        }

        (child_start, dir_end)
    };

    (dir_start..dir_end)
        .into_par_iter()
        .zip(child_dir_entries.into_par_iter())
        .filter(|(_, dir_entries)| !dir_entries.is_empty())
        .for_each(|(id, dir_entries)| walk_file_system(database, options, id, dir_entries));
}

fn list_dir<P: AsRef<Path>>(path: P, options: &IndexOptions) -> Result<(Vec<DirEntry>, u64)> {
    let rd = path.as_ref().read_dir()?;

    let mut dir_entries = Vec::new();
    let mut num_children = 0;

    for dent in rd {
        num_children += 1;

        if let Ok(dent) = dent {
            if options.ignore_hidden && util::is_hidden(&dent) {
                continue;
            }
            if let Ok(dir_entry) = DirEntry::from_std_dir_entry(dent, options) {
                dir_entries.push(dir_entry);
            }
        }
    }

    Ok((dir_entries, num_children))
}

/// Our version of DirEntry.
// std::fs::DirEntry keeps a file descriptor open, which leads to
// "too many open files" error when we are holding lots of std::fs::DirEntry.
// We avoid the problem by extracting information into our DirEntry and
// discarding std::fs::DirEntry.
struct DirEntry {
    name: String,
    path: PathBuf,
    ftype: FileType,
    metadata: Option<Metadata>,
}

impl DirEntry {
    fn from_std_dir_entry(dent: fs::DirEntry, options: &IndexOptions) -> Result<Self> {
        let name = dent
            .file_name()
            .to_str()
            .ok_or(Error::NonUtf8Path)?
            .to_string();

        let metadata = if options.needs_metadata() {
            Some(dent.metadata()?)
        } else {
            None
        };

        Ok(Self {
            name,
            path: dent.path(),
            ftype: dent.file_type()?,
            metadata,
        })
    }
}

/// Our representation of metadata.
struct EntryStatus {
    size: Option<u64>,
    mode: Option<Mode>,
    created: Option<SystemTime>,
    modified: Option<SystemTime>,
    accessed: Option<SystemTime>,
}

impl EntryStatus {
    fn from_metadata(metadata: &Metadata, options: &IndexOptions) -> Result<Self> {
        let size = options.index_flags[StatusKind::Size].then(|| metadata.len());
        Self::from_metadata_and_size(metadata, size, options)
    }

    fn from_metadata_and_size(
        metadata: &Metadata,
        size: Option<u64>,
        options: &IndexOptions,
    ) -> Result<Self> {
        let mode = options.index_flags[StatusKind::Mode].then(|| metadata.into());

        let created = if options.index_flags[StatusKind::Created] {
            Some(util::sanitize_system_time(&metadata.created()?))
        } else {
            None
        };
        let modified = if options.index_flags[StatusKind::Modified] {
            Some(util::sanitize_system_time(&metadata.modified()?))
        } else {
            None
        };
        let accessed = if options.index_flags[StatusKind::Accessed] {
            Some(util::sanitize_system_time(&metadata.accessed()?))
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

/// Struct holding information needed to create single entry and iterate over its children.
struct EntryInfo {
    name: String,
    ftype: FileType,
    status: Option<EntryStatus>,
    dir_entries: Vec<DirEntry>,
}

impl EntryInfo {
    fn from_path<P: AsRef<Path>>(path: P, options: &IndexOptions) -> Result<Self> {
        let name = util::get_basename(path.as_ref())
            .to_str()
            .ok_or(Error::NonUtf8Path)?
            .to_string();
        let metadata = path.as_ref().symlink_metadata()?;
        let ftype = metadata.file_type();

        let (status, dir_entries) = if ftype.is_dir() {
            let (dir_entries, num_children) = list_dir(path, options).unwrap_or_default();
            let status = if options.needs_metadata() {
                Some(EntryStatus::from_metadata_and_size(
                    &metadata,
                    Some(num_children),
                    options,
                )?)
            } else {
                None
            };

            (status, dir_entries)
        } else {
            let status = if options.needs_metadata() {
                Some(EntryStatus::from_metadata(&metadata, options)?)
            } else {
                None
            };

            (status, Vec::new())
        };

        Ok(Self {
            name,
            ftype,
            status,
            dir_entries,
        })
    }

    fn from_dir_entry(dent: DirEntry, options: &IndexOptions) -> Result<Self> {
        let (status, dir_entries) = if dent.ftype.is_dir() {
            let (dir_entries, num_children) = list_dir(&dent.path, options).unwrap_or_default();
            let status = if let Some(metadata) = dent.metadata {
                Some(EntryStatus::from_metadata_and_size(
                    &metadata,
                    Some(num_children),
                    options,
                )?)
            } else {
                None
            };

            (status, dir_entries)
        } else {
            let status = if let Some(metadata) = dent.metadata {
                Some(EntryStatus::from_metadata(&metadata, options)?)
            } else {
                None
            };

            (status, Vec::new())
        };

        Ok(Self {
            name: dent.name,
            ftype: dent.ftype,
            status,
            dir_entries,
        })
    }
}
