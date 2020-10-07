use super::util;
use super::{Database, EntryId, EntryNode, StatusKind};
use crate::mode::Mode;
use crate::{Error, Result};

use enum_map::{enum_map, EnumMap};
use parking_lot::Mutex;
use rayon::prelude::*;
use std::collections::HashMap;
use std::fs::{self, FileType, Metadata};
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
            if let Ok(mut root_info) =
                EntryInfo::from_path(&path, &self.index_flags, self.ignore_hidden)
            {
                if !root_info.ftype.is_dir() {
                    continue;
                }

                let dir_entries = mem::replace(&mut root_info.dir_entries, None);

                let root_node_id = {
                    let mut db = database.lock();

                    let root_node_id = db.entries.len() as u32;
                    db.push_entry(root_info, root_node_id);
                    db.root_paths.insert(root_node_id, path);

                    root_node_id
                };

                if let Some(dir_entries) = dir_entries {
                    walk_file_system(
                        database.clone(),
                        &self.index_flags,
                        self.ignore_hidden,
                        dir_entries,
                        root_node_id,
                    );
                }
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

struct DirEntry {
    name: String,
    metadata: Metadata,
    path: PathBuf,
    ftype: FileType,
}

impl DirEntry {
    fn from_fs_dir_entry(dent: fs::DirEntry) -> Result<Self> {
        let name = dent
            .file_name()
            .to_str()
            .ok_or(Error::NonUtf8Path)?
            .to_string();
        Ok(Self {
            name,
            metadata: dent.metadata()?,
            path: dent.path(),
            ftype: dent.file_type()?,
        })
    }
}

struct EntryInfo {
    name: String,
    status: EntryStatus,
    dir_entries: Option<Vec<DirEntry>>,
    ftype: FileType,
}

impl EntryInfo {
    fn from_path(path: &Path, index_flags: &StatusFlags, ignore_hidden: bool) -> Result<Self> {
        let name = util::get_basename(path)
            .to_str()
            .ok_or(Error::NonUtf8Path)?
            .to_string();
        let metadata = path.symlink_metadata()?;
        let ftype = metadata.file_type();

        if ftype.is_dir() {
            let (dir_entries, num_children) = get_dir_entries(path, ignore_hidden);

            Ok(Self {
                name,
                status: EntryStatus::from_metadata_with_size(
                    Some(num_children),
                    &metadata,
                    index_flags,
                )?,
                dir_entries,
                ftype,
            })
        } else {
            Ok(Self {
                name,
                status: EntryStatus::from_metadata(&metadata, index_flags)?,
                dir_entries: None,
                ftype,
            })
        }
    }

    fn from_dir_entry(
        dent: DirEntry,
        index_flags: &StatusFlags,
        ignore_hidden: bool,
    ) -> Result<Self> {
        if dent.ftype.is_dir() {
            let (dir_entries, num_children) = get_dir_entries(&dent.path, ignore_hidden);

            Ok(Self {
                name: dent.name,
                ftype: dent.ftype,
                status: EntryStatus::from_metadata_with_size(
                    Some(num_children),
                    &dent.metadata,
                    index_flags,
                )?,
                dir_entries,
            })
        } else {
            Ok(Self {
                name: dent.name,
                ftype: dent.ftype,
                status: EntryStatus::from_metadata(&dent.metadata, index_flags)?,
                dir_entries: None,
            })
        }
    }
}

fn get_dir_entries(path: &Path, ignore_hidden: bool) -> (Option<Vec<DirEntry>>, u64) {
    if let Ok(rd) = path.read_dir() {
        let fs_dir_entries: Vec<_> = rd.collect();
        let num_children = fs_dir_entries.len();

        let dir_entries = fs_dir_entries
            .into_iter()
            .filter_map(|dent| {
                dent.ok().and_then(|dent| {
                    if ignore_hidden && is_hidden(&dent) {
                        return None;
                    }
                    DirEntry::from_fs_dir_entry(dent).ok()
                })
            })
            .collect();

        (Some(dir_entries), num_children as u64)
    } else {
        (None, 0)
    }
}

fn walk_file_system(
    database: Arc<Mutex<Database>>,
    index_flags: &StatusFlags,
    ignore_hidden: bool,
    dir_entries: Vec<DirEntry>,
    parent: u32,
) {
    let (mut child_dirs, child_files) = dir_entries
        .into_iter()
        .filter_map(|dent| EntryInfo::from_dir_entry(dent, index_flags, ignore_hidden).ok())
        .partition::<Vec<_>, _>(|info| info.ftype.is_dir());

    let sub_dir_entries: Vec<_> = child_dirs
        .iter_mut()
        .map(|info| mem::replace(&mut info.dir_entries, None))
        .collect();

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
        .zip(sub_dir_entries.into_par_iter())
        .filter_map(|(index, dir_entries)| dir_entries.map(|dir_entries| (index, dir_entries)))
        .for_each_with(database, |database, (index, dir_entries)| {
            walk_file_system(
                database.clone(),
                index_flags,
                ignore_hidden,
                dir_entries,
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

#[cfg(unix)]
#[inline]
pub fn is_hidden(dent: &fs::DirEntry) -> bool {
    use std::os::unix::ffi::OsStrExt;

    dent.path()
        .file_name()
        .map(|filename| filename.as_bytes().get(0) == Some(&b'.'))
        .unwrap_or(false)
}

#[cfg(windows)]
#[inline]
pub fn is_hidden(dent: &fs::DirEntry) -> bool {
    if let Ok(metadata) = dent.metadata() {
        if Mode::from(&metadata).is_hidden() {
            return true;
        }
    }

    dent.path()
        .file_name()
        .and_then(|filename| filename.to_str())
        .map(|s| s.starts_with('.'))
        .unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use crate::database::*;
    use itertools::Itertools;
    use std::fs;
    use std::path::Path;
    use strum::IntoEnumIterator;
    use tempfile::TempDir;

    fn tmpdir() -> TempDir {
        tempfile::tempdir().unwrap()
    }

    fn create_dir_structure<P>(dirs: &[P]) -> TempDir
    where
        P: AsRef<Path>,
    {
        let tmpdir = tmpdir();
        let path = tmpdir.path();

        for dir in dirs {
            fs::create_dir_all(path.join(dir)).unwrap();
        }

        tmpdir
    }

    fn collect_paths<'a>(entries: impl Iterator<Item = Entry<'a>>) -> Vec<PathBuf> {
        let mut paths = Vec::new();
        for entry in entries {
            assert_eq!(entry.path().file_name().unwrap(), entry.basename());
            paths.push(entry.path());
            paths.append(&mut collect_paths(entry.children()));
        }
        paths
    }

    #[test]
    fn build() {
        let tmpdir =
            create_dir_structure(&[Path::new("a/b"), Path::new("e/a/b"), Path::new("b/c/d")]);
        let tmpdir2 =
            create_dir_structure(&[Path::new("a/b"), Path::new("f/b"), Path::new("𠮷/😥")]);
        let path = tmpdir.path();
        let path2 = tmpdir2.path();

        let mut builder = DatabaseBuilder::new();
        for kind in StatusKind::iter() {
            builder.index(kind);
            builder.fast_sort(kind);
        }

        let database = builder.add_dir(path).add_dir(path2).build().unwrap();

        let mut paths = collect_paths(database.root_entries());
        paths.sort_unstable();

        assert_eq!(
            paths,
            vec![
                path.to_path_buf(),
                path.join("a"),
                path.join("a/b"),
                path.join("b"),
                path.join("b/c"),
                path.join("b/c/d"),
                path.join("e"),
                path.join("e/a"),
                path.join("e/a/b"),
                path2.to_path_buf(),
                path2.join("a"),
                path2.join("a/b"),
                path2.join("f"),
                path2.join("f/b"),
                path2.join("𠮷"),
                path2.join("𠮷/😥")
            ]
            .iter()
            .map(|p| dunce::canonicalize(p).unwrap())
            .collect::<Vec<_>>()
            .iter()
            .sorted()
            .cloned()
            .collect::<Vec<_>>()
        );
    }

    #[test]
    fn empty_database() {
        let database = DatabaseBuilder::new().build().unwrap();
        assert_eq!(database.num_entries(), 0);

        let database = DatabaseBuilder::new().add_dir("xxxx").build().unwrap();
        assert_eq!(database.num_entries(), 0);
    }

    #[test]
    #[should_panic(expected = "Fast sorting cannot be enabled for a non-indexed status")]
    fn fast_sort_for_non_indexed_status() {
        let tmpdir = tmpdir();
        DatabaseBuilder::new()
            .fast_sort(StatusKind::Size)
            .add_dir(tmpdir.path())
            .build()
            .unwrap();
    }
}
