mod build;
mod search;
mod util;

pub use build::DatabaseBuilder;

use crate::mode::Mode;

use enum_map::{Enum, EnumMap};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::PathBuf;
use std::time::SystemTime;
use strum_macros::{Display, EnumIter};

#[derive(Debug, Serialize, Deserialize)]
pub struct Database {
    name_arena: String,
    entries: Vec<EntryNode>,
    root_paths: HashMap<u32, PathBuf>,
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
    pub fn root_entries(&self) -> impl ExactSizeIterator<Item = Entry<'_>> {
        self.root_paths
            .keys()
            .map(move |id| self.entry(EntryId(*id)))
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
        let node = &self.entries[id as usize];
        if node.parent == id {
            // root node
            self.root_paths[&id].clone()
        } else {
            let mut buf = self.path_from_id(node.parent);
            buf.push(&self.basename_from_node(node));
            buf
        }
    }

    #[inline]
    fn path_vec_from_id<'a>(&'a self, id: u32) -> Vec<&'a str> {
        let node = &self.entries[id as usize];
        if node.parent == id {
            // root node
            vec![self.root_paths[&id].to_str().unwrap()]
        } else {
            let mut buf = self.path_vec_from_id(node.parent);
            buf.push(&self.basename_from_node(node));
            buf
        }
    }
}

#[derive(Copy, Clone, Debug, PartialEq, Serialize, Deserialize, Enum, Display, EnumIter)]
#[serde(rename_all = "lowercase")]
pub enum StatusKind {
    Basename,
    #[serde(rename = "path")]
    #[strum(serialize = "Path")]
    FullPath,
    Extension,
    Size,
    Mode,
    Created,
    Modified,
    Accessed,
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
        let node = &self.node();
        if node.is_dir {
            return None;
        }

        let name = self.database.basename_from_node(node);
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
                    self.path()
                        .symlink_metadata()
                        .map(|metadata| metadata.len())
                        .ok()
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
                    .symlink_metadata()
                    .map(|metadata| Mode::from(&metadata))
                    .ok()
            })
    }

    #[inline]
    pub fn created(&self) -> Option<SystemTime> {
        self.database
            .created
            .as_ref()
            .map(|v| v[self.id.0 as usize])
            .or_else(|| {
                self.path()
                    .symlink_metadata()
                    .ok()
                    .and_then(|metadata| metadata.created().ok())
                    .map(|created| util::sanitize_system_time(&created))
            })
    }

    #[inline]
    pub fn modified(&self) -> Option<SystemTime> {
        self.database
            .modified
            .as_ref()
            .map(|v| v[self.id.0 as usize])
            .or_else(|| {
                self.path()
                    .symlink_metadata()
                    .ok()
                    .and_then(|metadata| metadata.modified().ok())
                    .map(|modified| util::sanitize_system_time(&modified))
            })
    }

    #[inline]
    pub fn accessed(&self) -> Option<SystemTime> {
        self.database
            .accessed
            .as_ref()
            .map(|v| v[self.id.0 as usize])
            .or_else(|| {
                self.path()
                    .symlink_metadata()
                    .ok()
                    .and_then(|metadata| metadata.accessed().ok())
                    .map(|accessed| util::sanitize_system_time(&accessed))
            })
    }

    #[inline]
    fn path_vec(&'a self) -> Vec<&'a str> {
        self.database.path_vec_from_id(self.id.0)
    }

    #[inline]
    fn node(&self) -> &EntryNode {
        &self.database.entries[self.id.0 as usize]
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
