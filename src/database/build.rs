use super::{util, Database, EntryId, EntryNode, StatusKind};
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

type StatusFlags = EnumMap<StatusKind, bool>;

struct IndexOptions {
    index_flags: StatusFlags,
    fast_sort_flags: StatusFlags,
    ignore_hidden: bool,
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
            fast_sort_flags: enum_map! {
                StatusKind::Basename => true,
                StatusKind::Path => false,
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

#[derive(Default)]
pub struct DatabaseBuilder {
    dirs: Vec<PathBuf>,
    options: IndexOptions,
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
        self.options.index_flags[kind] = true;
        self
    }

    pub fn fast_sort(&mut self, kind: StatusKind) -> &mut Self {
        self.options.fast_sort_flags[kind] = true;
        self
    }

    pub fn ignore_hidden(&mut self, yes: bool) -> &mut Self {
        self.options.ignore_hidden = yes;
        self
    }

    pub fn build(&self) -> Result<Database> {
        for (kind, enabled) in self.options.fast_sort_flags {
            if enabled && !self.options.index_flags[kind] {
                return Err(Error::InvalidOption(
                    "Fast sorting cannot be enabled for a non-indexed status.".to_string(),
                ));
            }
        }

        let dirs = util::canonicalize_dirs(&self.dirs)?;

        let database = Database {
            name_arena: String::new(),
            entries: Vec::new(),
            root_paths: HashMap::with_capacity(dirs.len()),
            size: self.options.index_flags[StatusKind::Size].then(Vec::new),
            mode: self.options.index_flags[StatusKind::Mode].then(Vec::new),
            created: self.options.index_flags[StatusKind::Created].then(Vec::new),
            modified: self.options.index_flags[StatusKind::Modified].then(Vec::new),
            accessed: self.options.index_flags[StatusKind::Accessed].then(Vec::new),
            sorted_ids: EnumMap::default(),
        };

        let database = Mutex::new(database);

        for path in dirs {
            if let Ok(mut root_info) = EntryInfo::from_path(&path, &self.options) {
                if !root_info.ftype.is_dir() {
                    continue;
                }

                let dir_entries = mem::take(&mut root_info.dir_entries);

                let root_node_id = {
                    let mut db = database.lock();

                    let root_node_id = db.entries.len() as u32;
                    db.push_entry(root_info, root_node_id);
                    db.root_paths.insert(root_node_id, path);

                    root_node_id
                };

                if !dir_entries.is_empty() {
                    walk_file_system(&database, &self.options, root_node_id, dir_entries);
                }
            }
        }

        let mut database = database.into_inner();

        database.sorted_ids = generate_sorted_ids(&database, &self.options);

        Ok(database)
    }
}

impl Database {
    #[inline]
    fn push_entry(&mut self, info: EntryInfo, parent_id: u32) {
        self.entries.push(EntryNode {
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
    fn set_children_range(&mut self, id: u32, range: Range<u32>) {
        let mut entry = &mut self.entries[id as usize];
        entry.child_start = range.start;
        entry.child_end = range.end;
    }
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
    fn from_path(path: &Path, options: &IndexOptions) -> Result<Self> {
        let name = util::get_basename(path)
            .to_str()
            .ok_or(Error::NonUtf8Path)?
            .to_string();
        let metadata = path.symlink_metadata()?;
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

fn list_dir(path: &Path, options: &IndexOptions) -> Result<(Vec<DirEntry>, u64)> {
    let rd = path.read_dir()?;

    let mut dir_entries = Vec::new();
    let mut num_children = 0;

    for dent in rd {
        num_children += 1;

        if let Ok(dent) = dent {
            if options.ignore_hidden && is_hidden(&dent) {
                continue;
            }
            if let Ok(dir_entry) = DirEntry::from_std_dir_entry(dent, options) {
                dir_entries.push(dir_entry);
            }
        }
    }

    Ok((dir_entries, num_children))
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
        let mut db = database.lock();

        let child_start = db.entries.len() as u32;
        let dir_end = child_start + child_dirs.len() as u32;
        let child_end = dir_end + child_files.len() as u32;

        db.set_children_range(parent_id, child_start..child_end);
        for info in child_dirs {
            db.push_entry(info, parent_id);
        }
        for info in child_files {
            db.push_entry(info, parent_id);
        }

        (child_start, dir_end)
    };

    (dir_start..dir_end)
        .into_par_iter()
        .zip(child_dir_entries.into_par_iter())
        .filter(|(_, dir_entries)| !dir_entries.is_empty())
        .for_each(|(id, dir_entries)| walk_file_system(database, options, id, dir_entries));
}

fn generate_sorted_ids(
    database: &Database,
    options: &IndexOptions,
) -> EnumMap<StatusKind, Option<Vec<u32>>> {
    let mut sorted_ids = EnumMap::default();
    for (kind, value) in sorted_ids.iter_mut() {
        if options.index_flags[kind] && options.fast_sort_flags[kind] {
            let compare_func = util::get_compare_func(kind);

            let mut ids = (0..database.entries.len() as u32).collect::<Vec<_>>();
            ids.as_parallel_slice_mut().par_sort_unstable_by(|a, b| {
                compare_func(&database.entry(EntryId(*a)), &database.entry(EntryId(*b)))
            });

            *value = Some(ids);
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
    use std::{fs, path::Path};
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

        let database1 = builder.add_dir(path).add_dir(path2).build().unwrap();
        let mut paths1 = collect_paths(database1.root_entries());
        paths1.sort_unstable();

        for kind in StatusKind::iter() {
            builder.index(kind);
            builder.fast_sort(kind);
        }

        let database2 = builder.add_dir(path).add_dir(path2).build().unwrap();
        let mut paths2 = collect_paths(database2.root_entries());
        paths2.sort_unstable();

        assert_eq!(paths1, paths2);
        assert_eq!(
            paths1,
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
    }

    #[test]
    #[should_panic]
    fn nonexistent_root_dir() {
        let tmpdir = tempfile::tempdir().unwrap();
        let dir = tmpdir.path().join("xxxx");
        DatabaseBuilder::new().add_dir(dir).build().unwrap();
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
