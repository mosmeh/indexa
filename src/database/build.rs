use super::util;
use super::{Database, EntryId, EntryNode, StatusKind};
use crate::mode::Mode;
use crate::{Error, Result};

use enum_map::{enum_map, EnumMap};
use parking_lot::Mutex;
use rayon::prelude::*;
use std::collections::HashMap;
use std::fs::{DirEntry, FileType, Metadata};
use std::io;
use std::mem;
use std::ops::Range;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::SystemTime;

type StatusFlags = EnumMap<StatusKind, bool>;

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

        let dirs = util::canonicalize_dirs(&self.dirs);

        let database = Database {
            name_arena: String::new(),
            entries: Vec::new(),
            root_paths: HashMap::with_capacity(dirs.len()),
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

        for path in dirs {
            let mut root_info = EntryInfo::from_path(&path, &self.index_flags)?;
            if !root_info.ftype.is_dir() {
                continue;
            }

            let dir_entries = mem::replace(&mut root_info.dir_entries, None);

            let root_node_id = {
                let mut db = database.lock();

                let root_node_id = db.entries.len() as u32;
                db.push_entry(
                    EntryInfo {
                        // safe to unwrap because of canonicalize_dirs
                        name: path.to_str().unwrap().to_string(),
                        ..root_info
                    },
                    root_node_id,
                );
                db.root_paths.insert(root_node_id, path);

                root_node_id
            };

            if let Some(dir_entries) = dir_entries {
                walk_file_system(
                    database.clone(),
                    &self.index_flags,
                    self.ignore_hidden,
                    &dir_entries,
                    root_node_id,
                );
            }
        }

        // safe to unwrap since above codes are the only users of database at the moment
        let mut database = Arc::try_unwrap(database).unwrap().into_inner();

        database.sorted_ids =
            generate_sorted_ids(&database, &self.index_flags, &self.fast_sort_flags);

        Ok(database)
    }
}

impl Database {
    #[inline]
    fn push_entry(&mut self, info: EntryInfo, parent: u32) {
        self.entries.push(EntryNode {
            name_start: self.name_arena.len(),
            name_len: info.name.len() as u16,
            parent,
            child_start: u32::MAX,
            child_end: u32::MAX,
            is_dir: info.ftype.is_dir(),
        });
        self.name_arena.push_str(&info.name);

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

    #[inline]
    fn set_children_range(&mut self, id: u32, range: Range<u32>) {
        let mut entry = &mut self.entries[id as usize];
        entry.child_start = range.start;
        entry.child_end = range.end;
    }
}

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
            Some(util::sanitize_system_time(&metadata.created()?))
        } else {
            None
        };
        let modified = if index_flags[StatusKind::Modified] {
            Some(util::sanitize_system_time(&metadata.modified()?))
        } else {
            None
        };
        let accessed = if index_flags[StatusKind::Accessed] {
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

struct EntryInfo {
    name: String,
    ftype: FileType,
    status: EntryStatus,
    dir_entries: Option<Vec<io::Result<DirEntry>>>,
}

impl EntryInfo {
    fn from_path(path: &Path, index_flags: &StatusFlags) -> Result<Self> {
        let name = if path.parent().is_some() {
            path.file_name().ok_or(Error::NoFilename)?.to_str()
        } else {
            path.to_str()
        };
        let name = name.ok_or(Error::NonUtf8Path)?.to_string();

        let metadata = path.symlink_metadata()?;
        let ftype = metadata.file_type();

        if ftype.is_dir() {
            let (dir_entries, num_children) = if let Ok(rd) = path.read_dir() {
                let dir_entries = rd.collect::<Vec<_>>();
                let num_children = dir_entries.len();
                (Some(dir_entries), num_children)
            } else {
                (None, 0)
            };

            Ok(Self {
                name,
                ftype,
                status: EntryStatus::from_metadata_with_size(
                    Some(num_children as u64),
                    &metadata,
                    index_flags,
                )?,
                dir_entries,
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
        let name = dent
            .file_name()
            .to_str()
            .ok_or(Error::NonUtf8Path)?
            .to_string();
        let ftype = dent.file_type()?;

        if ftype.is_dir() {
            let path = dent.path();

            let (dir_entries, num_children) = if let Ok(rd) = path.read_dir() {
                let dir_entries = rd.collect::<Vec<_>>();
                let num_children = dir_entries.len();
                (Some(dir_entries), num_children)
            } else {
                (None, 0)
            };

            Ok(Self {
                name,
                ftype,
                status: EntryStatus::from_metadata_with_size(
                    Some(num_children as u64),
                    &dent.metadata()?,
                    index_flags,
                )?,
                dir_entries,
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
        .map(|info| mem::replace(&mut info.dir_entries, None))
        .collect::<Vec<_>>();

    let (dir_start, dir_end) = {
        let mut db = database.lock();

        let child_start = db.entries.len() as u32;
        let dir_end = child_start + child_dirs.len() as u32;
        let child_end = dir_end + child_files.len() as u32;

        db.set_children_range(parent, child_start..child_end);
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
        .filter_map(|(index, dir_entries)| {
            dir_entries.as_ref().map(|dir_entries| (index, dir_entries))
        })
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

fn generate_sorted_ids(
    database: &Database,
    index_flags: &StatusFlags,
    fast_sort_flags: &StatusFlags,
) -> EnumMap<StatusKind, Option<Vec<u32>>> {
    let mut sorted_ids = EnumMap::new();
    for (kind, key) in sorted_ids.iter_mut() {
        if index_flags[kind] && fast_sort_flags[kind] {
            let compare_func = util::build_compare_func(kind);

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
