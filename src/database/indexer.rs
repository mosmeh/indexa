use super::{util, Database, EntryNode, StatusFlags, StatusKind};
use crate::{mode::Mode, Error, Result};

use camino::{Utf8Path, Utf8PathBuf};
use enum_map::{enum_map, EnumMap};
use fxhash::FxHashMap;
use hashbrown::{hash_map::RawEntryMut, HashMap};
use parking_lot::Mutex;
use rayon::prelude::*;
use std::{
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
    fn needs_metadata(&self, is_dir: bool) -> bool {
        let flags = &self.index_flags;
        (!is_dir && flags[StatusKind::Size]) // "size" of a directory is overwritten with a number of its children
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

        let root_entry = LeafOrInternalEntry::from_path(&path, self.options)?;
        let root_node_id = self.ctx.database.nodes.len() as u32;
        self.ctx.database.root_paths.insert(root_node_id, path);

        match root_entry {
            LeafOrInternalEntry::Leaf(entry) => {
                self.ctx.push_leaf_entry(&entry, root_node_id);
            }
            LeafOrInternalEntry::Internal(entry) => {
                self.ctx.push_internal_entry(&entry, root_node_id);
                let ctx = Mutex::new(self.ctx);
                walk_file_system(
                    &ctx,
                    self.options,
                    root_node_id,
                    entry.child_dir_entries.into(),
                );
                self.ctx = ctx.into_inner();
            }
        }

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

    fn push_leaf_entry(&mut self, entry: &LeafEntry, parent_id: u32) {
        self.push_entry(&entry.name, &entry.metadata, entry.is_dir, parent_id);
    }

    fn push_internal_entry(&mut self, entry: &InternalEntry, parent_id: u32) {
        self.push_entry(&entry.name, &entry.metadata, true, parent_id);
    }

    fn push_entry(&mut self, name: &str, metadata: &Metadata, is_dir: bool, parent_id: u32) {
        let hash = fxhash::hash64(name);
        let hash_entry = {
            let name_arena = &self.database.name_arena;
            self.name_spans.raw_entry_mut().from_hash(hash, |span| {
                &name_arena[span.start..][..span.len as usize] == name
            })
        };

        let name_len = name.len() as u16;
        let name_start = match hash_entry {
            RawEntryMut::Occupied(entry) => {
                let NameSpan { start, len } = *entry.key();
                debug_assert_eq!(len, name_len);
                start
            }
            RawEntryMut::Vacant(entry) => {
                let name_arena = &mut self.database.name_arena;
                let start = name_arena.len();
                name_arena.push_str(name);
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
        debug_assert_eq!(&self.database.name_arena[name_start..][..name.len()], name);

        self.database.nodes.push(EntryNode {
            name_start,
            name_len,
            parent: parent_id,
            child_start: u32::MAX,
            child_end: u32::MAX,
            is_dir,
        });

        if let Some(size) = &mut self.database.size {
            size.push(metadata.size);
        }
        if let Some(mode) = &mut self.database.mode {
            mode.push(metadata.mode);
        }
        if let Some(created) = &mut self.database.created {
            created.push(metadata.created);
        }
        if let Some(modified) = &mut self.database.modified {
            modified.push(metadata.modified);
        }
        if let Some(accessed) = &mut self.database.accessed {
            accessed.push(metadata.accessed);
        }
    }
}

fn walk_file_system(
    ctx: &Mutex<WalkContext>,
    options: &IndexOptions,
    parent_id: u32,
    dir_entries: Vec<DirEntry>,
) {
    let mut child_leaf_entries = Vec::new();
    let mut child_internal_entries = Vec::new();
    for dent in dir_entries {
        match LeafOrInternalEntry::from_dir_entry(dent, options) {
            LeafOrInternalEntry::Leaf(entry) => {
                child_leaf_entries.push(entry);
            }
            LeafOrInternalEntry::Internal(entry) => {
                child_internal_entries.push(entry);
            }
        }
    }

    let (internal_start, internal_end) = {
        let mut ctx = ctx.lock();

        let child_start = ctx.database.nodes.len() as u32;
        let internal_end = child_start + child_internal_entries.len() as u32;
        let child_end = internal_end + child_leaf_entries.len() as u32;

        let mut parent_node = &mut ctx.database.nodes[parent_id as usize];
        parent_node.child_start = child_start;
        parent_node.child_end = child_end;

        for entry in &child_internal_entries {
            ctx.push_internal_entry(entry, parent_id);
        }
        for entry in child_leaf_entries {
            ctx.push_leaf_entry(&entry, parent_id);
        }

        (child_start, internal_end)
    };

    (internal_start..internal_end)
        .into_par_iter()
        .zip(child_internal_entries.into_par_iter())
        .for_each(|(id, entry)| walk_file_system(ctx, options, id, entry.child_dir_entries.into()));
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
    metadata: Metadata,
}

impl DirEntry {
    fn from_std_dir_entry(dent: std::fs::DirEntry, options: &IndexOptions) -> Result<Self> {
        let is_dir = dent.file_type()?.is_dir();
        Ok(Self {
            name: dent.file_name().to_str().ok_or(Error::NonUtf8Path)?.into(),
            path: dent.path().into(),
            is_dir,
            metadata: if options.needs_metadata(is_dir) {
                Metadata::from_std_metadata(&dent.metadata()?, options)?
            } else {
                Metadata::default()
            },
        })
    }
}

/// Our version of Metadata.
///
/// Fields corresponding to non-indexed statuses are never referenced, so they
/// are filled with dummy values.
struct Metadata {
    size: u64,
    mode: Mode,
    created: SystemTime,
    modified: SystemTime,
    accessed: SystemTime,
}

impl Default for Metadata {
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

impl Metadata {
    fn from_std_metadata(metadata: &std::fs::Metadata, options: &IndexOptions) -> Result<Self> {
        Ok(Self {
            size: if options.index_flags[StatusKind::Size] {
                metadata.len()
            } else {
                0
            },
            mode: if options.index_flags[StatusKind::Mode] {
                metadata.into()
            } else {
                Mode::default()
            },
            created: if options.index_flags[StatusKind::Created] {
                util::sanitize_system_time(&metadata.created()?)
            } else {
                SystemTime::UNIX_EPOCH
            },
            modified: if options.index_flags[StatusKind::Modified] {
                util::sanitize_system_time(&metadata.modified()?)
            } else {
                SystemTime::UNIX_EPOCH
            },
            accessed: if options.index_flags[StatusKind::Accessed] {
                util::sanitize_system_time(&metadata.accessed()?)
            } else {
                SystemTime::UNIX_EPOCH
            },
        })
    }
}

/// An entry that has no children.
///
/// This can be a file or a directory.
struct LeafEntry {
    name: Box<str>,
    is_dir: bool,
    metadata: Metadata,
}

/// An entry that has at least one children.
///
/// All internal entries are, by definition, directories.
struct InternalEntry {
    name: Box<str>,
    metadata: Metadata,
    child_dir_entries: Box<[DirEntry]>,
}

enum LeafOrInternalEntry {
    Leaf(LeafEntry),
    Internal(InternalEntry),
}

impl LeafOrInternalEntry {
    fn from_dir_entry(dent: DirEntry, options: &IndexOptions) -> Self {
        if !dent.is_dir {
            return Self::Leaf(LeafEntry {
                name: dent.name,
                is_dir: false,
                metadata: dent.metadata,
            });
        }

        let (dir_entries, num_children) = list_dir(&dent.path, options).unwrap_or_default();
        let metadata = Metadata {
            size: num_children,
            ..dent.metadata
        };
        if dir_entries.is_empty() {
            Self::Leaf(LeafEntry {
                name: dent.name,
                is_dir: true,
                metadata,
            })
        } else {
            Self::Internal(InternalEntry {
                name: dent.name,
                metadata,
                child_dir_entries: dir_entries.into(),
            })
        }
    }

    fn from_path<P: AsRef<Utf8Path>>(path: P, options: &IndexOptions) -> Result<Self> {
        let path = path.as_ref();
        let metadata = path.symlink_metadata()?;
        let is_dir = metadata.is_dir();

        let dent = DirEntry {
            name: util::get_basename(path).into(),
            path: path.into(),
            is_dir,
            metadata: options
                .needs_metadata(is_dir)
                .then(|| Metadata::from_std_metadata(&metadata, options))
                .transpose()?
                .unwrap_or_default(),
        };

        Ok(Self::from_dir_entry(dent, options))
    }
}
