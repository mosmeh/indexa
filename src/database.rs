use crate::{Error, Result};
use crossbeam::channel::Sender;
use rayon::prelude::*;
use regex::Regex;
use serde::{Deserialize, Serialize};
use std::fs::{DirEntry, FileType, Metadata};
use std::path::{Path, PathBuf};
use std::time::SystemTime;

#[derive(Serialize, Deserialize)]
pub struct Database {
    root_path: PathBuf,
    root_node: Node,
}

impl<'a> Database {
    pub fn new<P: AsRef<Path>>(path: P) -> Result<Self> {
        let root_node = create_node(path.as_ref(), EntryInfo::from_path(path.as_ref())?);
        Ok(Self {
            root_node,
            root_path: path.as_ref().to_path_buf(),
        })
    }

    pub fn search(&'a self, pattern: &Regex, tx: Sender<Hit<'a>>) -> Result<()> {
        search(&self.root_node, &self.root_path, pattern, tx)
    }
}

pub struct Hit<'a> {
    pub path: PathBuf,
    pub status: &'a FileStatus,
}

#[derive(Serialize, Deserialize)]
pub struct FileStatus {
    pub ctime: SystemTime,
    pub mtime: SystemTime,
    pub size: u64,
}

impl FileStatus {
    fn from_metadata(metadata: &Metadata) -> Result<Self> {
        // metadata may contain invalid SystemTime
        // it will catch them as Err instead of panic
        let ctime =
            SystemTime::UNIX_EPOCH + metadata.created()?.duration_since(SystemTime::UNIX_EPOCH)?;
        let mtime = SystemTime::UNIX_EPOCH
            + metadata
                .modified()?
                .duration_since(SystemTime::UNIX_EPOCH)?;

        let status = Self {
            ctime,
            mtime,
            size: metadata.len(),
        };
        Ok(status)
    }
}

#[derive(Serialize, Deserialize)]
enum Node {
    File {
        name: String,
        status: FileStatus,
    },
    Directory {
        name: String,
        status: FileStatus,
        children: Vec<Node>,
    },
    Symlink {
        name: String,
        status: FileStatus,
    },
}

impl Node {
    fn new(entry_info: EntryInfo) -> Node {
        let EntryInfo {
            name,
            ftype,
            status,
            ..
        } = entry_info;

        if ftype.is_file() {
            Node::File { name, status }
        } else if ftype.is_dir() {
            Node::Directory {
                name,
                status,
                children: Vec::new(),
            }
        } else if ftype.is_symlink() {
            Node::Symlink { name, status }
        } else {
            unimplemented!()
        }
    }

    fn name(&self) -> &str {
        use Node::*;
        match &self {
            File { name, .. } | Directory { name, .. } | Symlink { name, .. } => name,
        }
    }

    fn status(&self) -> &FileStatus {
        use Node::*;
        match &self {
            File { status, .. } | Directory { status, .. } | Symlink { status, .. } => status,
        }
    }
}

fn create_node(path: &Path, entry_info: EntryInfo) -> Node {
    let mut node = Node::new(entry_info);
    if let Node::Directory { children, .. } = &mut node {
        // ignore all the errors that occur during traversal
        if let Ok(rd) = path.read_dir() {
            children.par_extend(rd.collect::<Vec<_>>().par_iter().filter_map(|dent| {
                dent.as_ref()
                    .ok()
                    .and_then(|dent| {
                        EntryInfo::from_dir_entry(&dent)
                            .ok()
                            .map(|entry_info| (dent.path(), entry_info))
                    })
                    .map(|(path, entry_info)| create_node(&path, entry_info))
            }))
        }
    }
    node
}

fn search<'a>(node: &'a Node, path: &Path, pattern: &Regex, tx: Sender<Hit<'a>>) -> Result<()> {
    if let Node::Directory { children, .. } = node {
        children.par_iter().try_for_each_with(tx, |tx, child| {
            let child_path = path.join(child.name());
            if let Some(s) = child_path.to_str() {
                if pattern.is_match(s) {
                    tx.send(Hit {
                        path: child_path.clone(),
                        status: child.status(),
                    })
                    .map_err(|_| Error::ChannelSend)?;
                }
                if let Node::Directory { .. } = child {
                    search(child, &child_path, pattern, tx.clone())?;
                }
            }
            Ok(())
        })
    } else {
        unreachable!()
    }
}

struct EntryInfo {
    name: String,
    ftype: FileType,
    status: FileStatus,
}

impl EntryInfo {
    fn from_path(path: &Path) -> Result<Self> {
        let name = if path.parent().is_some() {
            path.file_name().ok_or(Error::Filename)?.to_str()
        } else {
            path.to_str()
        };
        let name = name.ok_or(Error::Utf8)?.to_string();

        let metadata = path.metadata()?;
        let ftype = metadata.file_type();

        Ok(Self {
            name,
            ftype,
            status: FileStatus::from_metadata(&metadata)?,
        })
    }

    fn from_dir_entry(dent: &DirEntry) -> Result<Self> {
        let name = dent.file_name().to_str().ok_or(Error::Utf8)?.to_string();

        Ok(Self {
            name,
            ftype: dent.file_type()?,
            status: FileStatus::from_metadata(&dent.metadata()?)?,
        })
    }
}
