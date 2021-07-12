use super::{
    indexer::{IndexOptions, Indexer},
    util, Database, EntryId, StatusKind,
};
use crate::{Error, Result};

use enum_map::{enum_map, EnumMap};
use rayon::prelude::*;
use std::path::{Path, PathBuf};

pub type StatusFlags = EnumMap<StatusKind, bool>;

#[derive(Default)]
pub struct DatabaseBuilder {
    dirs: Vec<PathBuf>,
    index_options: IndexOptions,
    fast_sort_flags: StatusFlags,
}

impl DatabaseBuilder {
    pub fn new() -> Self {
        Self {
            dirs: Vec::new(),
            index_options: Default::default(),
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
        }
    }

    pub fn add_dir<P: AsRef<Path>>(&mut self, path: P) -> &mut Self {
        self.dirs.push(path.as_ref().to_path_buf());
        self
    }

    pub fn index(&mut self, kind: StatusKind) -> &mut Self {
        self.index_options.index_flags[kind] = true;
        self
    }

    pub fn fast_sort(&mut self, kind: StatusKind) -> &mut Self {
        self.fast_sort_flags[kind] = true;
        self
    }

    pub fn ignore_hidden(&mut self, yes: bool) -> &mut Self {
        self.index_options.ignore_hidden = yes;
        self
    }

    pub fn build(&self) -> Result<Database> {
        for (kind, enabled) in self.fast_sort_flags {
            if enabled && !self.index_options.index_flags[kind] {
                return Err(Error::InvalidOption(
                    "Fast sorting cannot be enabled for a non-indexed status.".to_string(),
                ));
            }
        }

        let dirs = util::canonicalize_dirs(&self.dirs)?;
        let mut indexer = Indexer::new(&self.index_options);

        for path in dirs {
            indexer = indexer.index(path)?;
        }

        let mut database = indexer.finish();

        let mut sorted_ids = EnumMap::default();
        for (kind, ids) in sorted_ids.iter_mut() {
            if self.fast_sort_flags[kind] {
                *ids = Some(sort_ids(&database, kind));
            }
        }
        database.sorted_ids = sorted_ids;

        Ok(database)
    }
}

fn sort_ids(database: &Database, sort_by: StatusKind) -> Vec<u32> {
    let compare_func = util::get_compare_func(sort_by);

    let mut ids = (0..database.nodes.len() as u32).collect::<Vec<_>>();
    ids.as_parallel_slice_mut().par_sort_unstable_by(|a, b| {
        compare_func(&database.entry(EntryId(*a)), &database.entry(EntryId(*b)))
    });

    ids
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
