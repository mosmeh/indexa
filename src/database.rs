mod builder;
mod indexer;
mod search;
mod util;

pub use builder::DatabaseBuilder;

use crate::{mode::Mode, Result};

use enum_map::{Enum, EnumMap};
use fxhash::FxHashMap;
use serde::{Deserialize, Serialize};
use std::{cmp::Ordering, path::PathBuf, time::SystemTime};
use strum_macros::{Display, EnumIter};

// Database can have multiple "root" entries, which correspond to directories
// specified in "dirs" in config.

#[derive(Debug, Serialize, Deserialize)]
pub struct Database {
    /// names of all entries concatenated
    name_arena: String,
    nodes: Vec<EntryNode>,
    root_paths: FxHashMap<u32, PathBuf>,
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
        self.nodes.len()
    }

    #[inline]
    pub fn root_entries(&self) -> impl ExactSizeIterator<Item = Entry<'_>> {
        self.root_paths
            .keys()
            .map(move |id| self.entry(EntryId(*id)))
    }

    #[inline]
    pub fn is_indexed(&self, kind: StatusKind) -> bool {
        match kind {
            StatusKind::Basename | StatusKind::Path | StatusKind::Extension => true,
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

    #[inline]
    pub fn entry(&self, id: EntryId) -> Entry<'_> {
        Entry { database: self, id }
    }

    #[inline]
    fn basename_from_node(&self, node: &EntryNode) -> &str {
        &self.name_arena[node.name_start..node.name_start + node.name_len as usize]
    }

    #[inline]
    fn path_from_id(&self, id: u32) -> PathBuf {
        let node = &self.nodes[id as usize];
        if node.parent == id {
            // root node
            self.root_paths[&id].clone()
        } else {
            let mut buf = self.path_from_id(node.parent);
            buf.push(&self.basename_from_node(node));
            buf
        }
    }

    fn cmp_by_path(&self, id_a: u32, id_b: u32) -> Ordering {
        // -- Fast path --

        if id_a == id_b {
            return Ordering::Equal;
        }

        let node_a = &self.nodes[id_a as usize];
        let node_b = &self.nodes[id_b as usize];

        let a_is_root = node_a.parent == id_a;
        let b_is_root = node_b.parent == id_b;

        if a_is_root && b_is_root {
            // e.g. C:\ vs. D:\
            return Ord::cmp(&self.root_paths[&id_a], &self.root_paths[&id_b]);
        }

        if !a_is_root && !b_is_root && node_a.parent == node_b.parent {
            // e.g. /foo/bar vs. /foo/baz
            return Ord::cmp(
                self.basename_from_node(node_a),
                self.basename_from_node(node_b),
            );
        }

        if !b_is_root && id_a == node_b.parent {
            // e.g. /foo vs. /foo/bar
            return Ordering::Less;
        }
        if !a_is_root && id_b == node_a.parent {
            // e.g. /foo/bar vs. /foo
            return Ordering::Greater;
        }

        // -- Slow path --

        // "path" in the sense of graph
        fn path_from_root(db: &Database, mut id: u32) -> impl Iterator<Item = u32> {
            let mut path = Vec::new();
            loop {
                let node = &db.nodes[id as usize];
                path.push(id);
                if node.parent == id {
                    // root node
                    return path.into_iter().rev();
                } else {
                    id = node.parent;
                }
            }
        }

        let mut path_a = path_from_root(self, id_a);
        let mut path_b = path_from_root(self, id_b);
        loop {
            match (path_a.next(), path_b.next()) {
                (Some(a), Some(b)) if a == b => continue,
                (None, None) => return Ordering::Equal,
                (None, Some(_)) => return Ordering::Less, // /foo vs. /foo/bar
                (Some(_), None) => return Ordering::Greater, // /foo/bar vs. /foo
                (Some(a), Some(b)) => {
                    // /foo/bar vs. /foo/baz
                    return Ord::cmp(
                        self.basename_from_node(&self.nodes[a as usize]),
                        self.basename_from_node(&self.nodes[b as usize]),
                    );
                }
            }
        }
    }
}

#[derive(Copy, Clone, Debug, PartialEq, Serialize, Deserialize, Enum, Display, EnumIter)]
#[serde(rename_all = "lowercase")]
pub enum StatusKind {
    #[serde(alias = "name")]
    Basename,
    Path,
    #[serde(alias = "ext")]
    Extension,
    Size,
    #[serde(
        alias = "attribute",
        alias = "attributes",
        alias = "attr",
        alias = "attrs"
    )]
    Mode,
    #[serde(alias = "ctime")]
    Created,
    #[serde(alias = "mtime")]
    Modified,
    #[serde(alias = "atime")]
    Accessed,
}

type StatusFlags = EnumMap<StatusKind, bool>;

#[derive(Debug, Copy, Clone)]
pub struct EntryId(u32);

/// A convenience struct which acts as if it holds data of the entry.
///
/// If a requested status is indexed, Entry grabs it from database.
/// Otherwise the status is fetched from file systems.
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
    pub fn children(&self) -> impl ExactSizeIterator<Item = Entry<'_>> {
        let node = &self.node();
        (node.child_start..node.child_end).map(move |id| self.database.entry(EntryId(id)))
    }

    #[inline]
    pub fn basename(&self) -> &str {
        self.database.basename_from_node(self.node())
    }

    #[inline]
    pub fn path(&self) -> PathBuf {
        self.database.path_from_id(self.id.0)
    }

    #[inline]
    pub fn extension(&self) -> Option<&str> {
        let node = self.node();
        if node.is_dir {
            return None;
        }

        self.database
            .basename_from_node(node)
            .rsplit_once('.')
            .map(|(_, ext)| ext)
    }

    #[inline]
    pub fn size(&self) -> Result<u64> {
        if let Some(size) = &self.database.size {
            return Ok(size[self.id.0 as usize]);
        }

        let size = if self.is_dir() {
            self.path().read_dir().map(|rd| rd.count() as u64)?
        } else {
            self.path()
                .symlink_metadata()
                .map(|metadata| metadata.len())?
        };

        Ok(size)
    }

    #[inline]
    pub fn mode(&self) -> Result<Mode> {
        if let Some(mode) = &self.database.mode {
            return Ok(mode[self.id.0 as usize]);
        }

        self.path()
            .symlink_metadata()
            .map(|metadata| Mode::from(&metadata))
            .map_err(Into::into)
    }

    #[inline]
    pub fn created(&self) -> Result<SystemTime> {
        if let Some(created) = &self.database.created {
            return Ok(created[self.id.0 as usize]);
        }

        self.path()
            .symlink_metadata()
            .and_then(|metadata| metadata.created())
            .map(|created| util::sanitize_system_time(&created))
            .map_err(Into::into)
    }

    #[inline]
    pub fn modified(&self) -> Result<SystemTime> {
        if let Some(modified) = &self.database.modified {
            return Ok(modified[self.id.0 as usize]);
        }

        self.path()
            .symlink_metadata()
            .and_then(|metadata| metadata.modified())
            .map(|modified| util::sanitize_system_time(&modified))
            .map_err(Into::into)
    }

    #[inline]
    pub fn accessed(&self) -> Result<SystemTime> {
        if let Some(accessed) = &self.database.accessed {
            return Ok(accessed[self.id.0 as usize]);
        }

        self.path()
            .symlink_metadata()
            .and_then(|metadata| metadata.accessed())
            .map(|accessed| util::sanitize_system_time(&accessed))
            .map_err(Into::into)
    }

    #[inline]
    fn node(&self) -> &EntryNode {
        &self.database.nodes[self.id.0 as usize]
    }

    #[inline]
    fn cmp_by_path(&self, other: &Self) -> Ordering {
        self.database.cmp_by_path(self.id.0, other.id.0)
    }

    #[inline]
    fn cmp_by_extension(&self, other: &Self) -> Ordering {
        if self.node().is_dir && other.node().is_dir {
            return Ordering::Equal;
        }
        self.extension().cmp(&other.extension())
    }
}

#[derive(Debug, Serialize, Deserialize)]
struct EntryNode {
    name_start: usize,
    parent: u32,
    child_start: u32,
    child_end: u32,
    name_len: u16,
    is_dir: bool,
}

impl EntryNode {
    #[inline]
    fn has_any_child(&self) -> bool {
        self.child_start < self.child_end
    }
}
