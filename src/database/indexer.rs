use super::{util, Database, EntryNode, StatusFlags, StatusKind};
use crate::{mode::Mode, Error, Result};

use camino::{Utf8Path, Utf8PathBuf};
use enum_map::{enum_map, EnumMap};
use fxhash::FxHashMap;
use hashbrown::{hash_map::RawEntryMut, HashMap};
use parking_lot::Mutex;
use rayon::prelude::*;
use std::{
    fs::{self, Metadata},
    mem,
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
    ctx: WalkContext,
}

impl<'a> Indexer<'a> {
    pub fn new(options: &'a IndexOptions) -> Indexer<'a> {
        let database = Database {
            name_arena: String::new(),
            nodes: Vec::new(),
            root_paths: FxHashMap::default(),
            size: options.index_flags[StatusKind::Size].then(Vec::new),
            mode: options.index_flags[StatusKind::Mode].then(Vec::new),
            created: options.index_flags[StatusKind::Created].then(Vec::new),
            modified: options.index_flags[StatusKind::Modified].then(Vec::new),
            accessed: options.index_flags[StatusKind::Accessed].then(Vec::new),
            sorted_ids: EnumMap::default(),
        };

        Self {
            options,
            ctx: WalkContext::new(database),
        }
    }

    pub fn index<P: Into<PathBuf>>(mut self, path: P) -> Result<Self> {
        let path = Utf8PathBuf::from_path_buf(path.into()).map_err(|_| Error::NonUtf8Path)?;

        let mut root_info = EntryInfo::from_path(&path, self.options)?;
        let dir_entries = mem::take(&mut root_info.dir_entries);

        let root_node_id = self.ctx.database.nodes.len() as u32;
        self.ctx.push_entry(root_info, root_node_id);
        self.ctx.database.root_paths.insert(root_node_id, path);

        if dir_entries.is_empty() {
            return Ok(self);
        }

        let ctx = Mutex::new(self.ctx);
        walk_file_system(&ctx, self.options, root_node_id, dir_entries.into());
        self.ctx = ctx.into_inner();

        Ok(self)
    }

    pub fn finish(self) -> Database {
        self.ctx.into_inner()
    }
}

/// Span in name_arena
struct NameSpan {
    start: usize,
    len: u16,
}

struct WalkContext {
    database: Database,

    // Set of spans which represent interned strings.
    // HashMap (instead of HashSet) is used here to make use of raw_entry_mut().
    // Also, () is specified as HashBuilder since we don't use the default hasher.
    // Each hash value is caluculated from a string NameSpan represents.
    name_spans: HashMap<NameSpan, (), ()>,
}

impl WalkContext {
    fn new(database: Database) -> WalkContext {
        Self {
            database,
            name_spans: HashMap::with_hasher(()),
        }
    }

    fn into_inner(self) -> Database {
        self.database
    }

    fn push_entry(&mut self, info: EntryInfo, parent_id: u32) {
        let hash = fxhash::hash64(&info.name);
        let hash_entry = {
            let name_arena = &self.database.name_arena;
            self.name_spans.raw_entry_mut().from_hash(hash, |span| {
                name_arena[span.start..][..span.len as usize] == *info.name
            })
        };

        let name_len = info.name.len() as u16;
        let name_start = match hash_entry {
            RawEntryMut::Occupied(entry) => {
                let NameSpan { start, len } = *entry.key();
                debug_assert_eq!(len, name_len);
                start
            }
            RawEntryMut::Vacant(entry) => {
                let name_arena = &mut self.database.name_arena;
                let start = name_arena.len();
                name_arena.push_str(&info.name);
                entry.insert_with_hasher(
                    hash,
                    NameSpan {
                        start,
                        len: name_len,
                    },
                    (),
                    |span| fxhash::hash64(&name_arena[span.start..][..span.len as usize]),
                );
                start
            }
        };
        debug_assert_eq!(
            self.database.name_arena[name_start..][..info.name.len()],
            *info.name
        );

        self.database.nodes.push(EntryNode {
            name_start,
            name_len,
            parent: parent_id,
            child_start: u32::MAX,
            child_end: u32::MAX,
            is_dir: info.is_dir,
        });

        let status = info.status;
        if let Some(size) = &mut self.database.size {
            size.push(status.size);
        }
        if let Some(mode) = &mut self.database.mode {
            mode.push(status.mode);
        }
        if let Some(created) = &mut self.database.created {
            created.push(status.created);
        }
        if let Some(modified) = &mut self.database.modified {
            modified.push(status.modified);
        }
        if let Some(accessed) = &mut self.database.accessed {
            accessed.push(status.accessed);
        }
    }
}

fn walk_file_system(
    ctx: &Mutex<WalkContext>,
    options: &IndexOptions,
    parent_id: u32,
    dir_entries: Vec<DirEntry>,
) {
    let (mut child_dirs, child_files) = dir_entries
        .into_iter()
        .filter_map(|dent| EntryInfo::from_dir_entry(dent, options).ok())
        .partition::<Vec<_>, _>(|info| info.is_dir);

    if child_dirs.is_empty() && child_files.is_empty() {
        return;
    }

    let child_dir_entries: Vec<_> = child_dirs
        .iter_mut()
        .map(|info| mem::take(&mut info.dir_entries))
        .collect();

    let (dir_start, dir_end) = {
        let mut ctx = ctx.lock();

        let child_start = ctx.database.nodes.len() as u32;
        let dir_end = child_start + child_dirs.len() as u32;
        let child_end = dir_end + child_files.len() as u32;

        let mut parent_node = &mut ctx.database.nodes[parent_id as usize];
        parent_node.child_start = child_start;
        parent_node.child_end = child_end;

        for info in child_dirs {
            ctx.push_entry(info, parent_id);
        }
        for info in child_files {
            ctx.push_entry(info, parent_id);
        }

        (child_start, dir_end)
    };

    (dir_start..dir_end)
        .into_par_iter()
        .zip(child_dir_entries.into_par_iter())
        .filter(|(_, dir_entries)| !dir_entries.is_empty())
        .for_each(|(id, dir_entries)| walk_file_system(ctx, options, id, dir_entries.into()));
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
    name: Box<str>,
    path: Box<Path>,
    is_dir: bool,
    metadata: Option<Metadata>,
}

impl DirEntry {
    fn from_std_dir_entry(dent: fs::DirEntry, options: &IndexOptions) -> Result<Self> {
        Ok(Self {
            name: dent.file_name().to_str().ok_or(Error::NonUtf8Path)?.into(),
            path: dent.path().into(),
            is_dir: dent.file_type()?.is_dir(),
            metadata: options
                .needs_metadata()
                .then(|| dent.metadata())
                .transpose()?,
        })
    }
}

/// Our representation of metadata.
///
/// Fields corresponding to non-indexed statuses are never referenced, so they
/// are filled with dummy values.
struct EntryStatus {
    size: u64,
    mode: Mode,
    created: SystemTime,
    modified: SystemTime,
    accessed: SystemTime,
}

impl Default for EntryStatus {
    fn default() -> Self {
        Self {
            size: 0,
            mode: Mode::default(),
            created: SystemTime::UNIX_EPOCH,
            modified: SystemTime::UNIX_EPOCH,
            accessed: SystemTime::UNIX_EPOCH,
        }
    }
}

impl EntryStatus {
    fn from_metadata(metadata: &Metadata, options: &IndexOptions) -> Result<Self> {
        let size = options.index_flags[StatusKind::Size].then(|| metadata.len());
        Self::from_metadata_and_size(metadata, size.unwrap_or_default(), options)
    }

    fn from_metadata_and_size(
        metadata: &Metadata,
        size: u64,
        options: &IndexOptions,
    ) -> Result<Self> {
        let mut status = Self {
            size,
            ..Self::default()
        };

        if options.index_flags[StatusKind::Mode] {
            status.mode = metadata.into();
        }

        if options.index_flags[StatusKind::Created] {
            status.created = util::sanitize_system_time(&metadata.created()?);
        }
        if options.index_flags[StatusKind::Modified] {
            status.modified = util::sanitize_system_time(&metadata.modified()?);
        }
        if options.index_flags[StatusKind::Accessed] {
            status.accessed = util::sanitize_system_time(&metadata.accessed()?);
        }

        Ok(status)
    }
}

/// Struct holding information needed to create single entry and iterate over its children.
struct EntryInfo {
    name: Box<str>,
    is_dir: bool,
    status: EntryStatus,
    dir_entries: Box<[DirEntry]>,
}

impl EntryInfo {
    fn from_path<P: AsRef<Utf8Path>>(path: P, options: &IndexOptions) -> Result<Self> {
        let path = path.as_ref();
        let metadata = path.symlink_metadata()?;

        let dent = DirEntry {
            name: util::get_basename(path).into(),
            path: path.into(),
            is_dir: metadata.is_dir(),
            metadata: options.needs_metadata().then(|| metadata),
        };

        Self::from_dir_entry(dent, options)
    }

    fn from_dir_entry(dent: DirEntry, options: &IndexOptions) -> Result<Self> {
        let (status, dir_entries) = if dent.is_dir {
            let (dir_entries, num_children) = list_dir(&dent.path, options).unwrap_or_default();
            let status = dent
                .metadata
                .map(|metadata| {
                    EntryStatus::from_metadata_and_size(&metadata, num_children, options)
                })
                .transpose()?
                .unwrap_or_default();

            (status, dir_entries.into())
        } else {
            let status = dent
                .metadata
                .map(|metadata| EntryStatus::from_metadata(&metadata, options))
                .transpose()?
                .unwrap_or_default();

            (status, Box::default())
        };

        Ok(Self {
            name: dent.name,
            is_dir: dent.is_dir,
            status,
            dir_entries,
        })
    }
}
