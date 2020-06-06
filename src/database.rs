use crate::{Error, Result};
use rayon::prelude::*;
use regex::Regex;
use serde::{Deserialize, Serialize};
use std::fs::{DirEntry, FileType, Metadata};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::SystemTime;

#[derive(Debug, Serialize, Deserialize)]
pub struct Database {
    files: Vec<Entry>,
    dirs: Vec<Entry>,
    file_statuses: StatusVec,
    dir_statuses: StatusVec,
}

impl<'a> Database {
    pub fn new<P: AsRef<Path>>(path: P) -> Result<Self> {
        let entry_info = EntryInfo::from_path(path.as_ref())?;
        let root = Entry {
            name: entry_info.name,
            parent: 0, // points to itself
        };
        let database = Self {
            files: Vec::new(),
            dirs: vec![root],
            file_statuses: Default::default(),
            dir_statuses: StatusVec {
                ctime: vec![entry_info.status.ctime],
                mtime: vec![entry_info.status.mtime],
                size: vec![entry_info.status.size],
            },
        };

        let database = Arc::new(Mutex::new(database));
        walk_file_system(database.clone(), path.as_ref(), 0);

        // safe to unwrap since create_node, which is the only user of database, has returned
        Ok(Arc::try_unwrap(database).unwrap().into_inner().unwrap())
    }

    pub fn search(&self, pattern: &Regex, in_path: bool) -> Vec<Hit> {
        search(&self, pattern, in_path)
    }

    pub fn path_from_hit(&self, hit: &Hit) -> PathBuf {
        let mut buf = PathBuf::new();
        let entry = match hit {
            Hit::File(i) => &self.files[*i],
            Hit::Directory(i) => &self.dirs[*i],
        };
        self.path_from_entry_impl(entry.parent, &mut buf);
        buf.push(&entry.name);
        buf
    }

    pub fn status_from_hit(&'a self, hit: &Hit) -> RefEntryStatus<'a> {
        let (&i, status_vec) = match hit {
            Hit::File(i) => (i, &self.file_statuses),
            Hit::Directory(i) => (i, &self.dir_statuses),
        };
        RefEntryStatus {
            ctime: &status_vec.ctime[i],
            mtime: &status_vec.mtime[i],
            size: status_vec.size[i],
        }
    }

    fn push_file(&mut self, entry_info: EntryInfo, parent: usize) {
        self.files.push(Entry {
            name: entry_info.name,
            parent,
        });
        self.file_statuses.ctime.push(entry_info.status.ctime);
        self.file_statuses.mtime.push(entry_info.status.mtime);
        self.file_statuses.size.push(entry_info.status.size);
    }

    fn push_dir(&mut self, entry_info: EntryInfo, parent: usize) {
        self.dirs.push(Entry {
            name: entry_info.name,
            parent,
        });
        self.dir_statuses.ctime.push(entry_info.status.ctime);
        self.dir_statuses.mtime.push(entry_info.status.mtime);
        self.dir_statuses.size.push(entry_info.status.size);
    }

    fn path_from_entry(&self, entry: &Entry) -> PathBuf {
        let mut buf = PathBuf::new();
        self.path_from_entry_impl(entry.parent, &mut buf);
        buf.push(&entry.name);
        buf
    }

    fn path_from_entry_impl(&self, index: usize, mut buf: &mut PathBuf) {
        let dir = &self.dirs[index];
        if dir.parent == 0 {
            buf.push(&self.dirs[dir.parent].name);
        } else {
            self.path_from_entry_impl(dir.parent, &mut buf);
        }
        buf.push(&dir.name);
    }
}

pub enum Hit {
    File(usize),
    Directory(usize),
}

pub struct RefEntryStatus<'a> {
    pub ctime: &'a SystemTime,
    pub mtime: &'a SystemTime,
    pub size: u64,
}

#[derive(Debug, Serialize, Deserialize)]
struct Entry {
    name: String,
    parent: usize,
}

#[derive(Debug, Serialize, Deserialize)]
struct StatusVec {
    ctime: Vec<SystemTime>,
    mtime: Vec<SystemTime>,
    size: Vec<u64>,
}

impl Default for StatusVec {
    fn default() -> Self {
        Self {
            ctime: Vec::new(),
            mtime: Vec::new(),
            size: Vec::new(),
        }
    }
}

#[derive(Serialize, Deserialize)]
struct EntryStatus {
    ctime: SystemTime,
    mtime: SystemTime,
    size: u64,
}

impl EntryStatus {
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

struct EntryInfo {
    name: String,
    ftype: FileType,
    status: EntryStatus,
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
            status: EntryStatus::from_metadata(&metadata)?,
        })
    }

    fn from_dir_entry(dent: &DirEntry) -> Result<Self> {
        let name = dent.file_name().to_str().ok_or(Error::Utf8)?.to_string();

        Ok(Self {
            name,
            ftype: dent.file_type()?,
            status: EntryStatus::from_metadata(&dent.metadata()?)?,
        })
    }
}

fn walk_file_system(database: Arc<Mutex<Database>>, path: &Path, parent: usize) {
    if let Ok(rd) = path.read_dir() {
        let children = rd
            .filter_map(|dent| {
                dent.ok().and_then(|dent| {
                    EntryInfo::from_dir_entry(&dent)
                        .ok()
                        .map(|entry_info| (dent.path(), entry_info))
                })
            })
            .collect::<Vec<_>>();

        let mut sub_directories = Vec::new();
        {
            let mut db = database.lock().unwrap();
            for (path, entry_info) in children {
                if entry_info.ftype.is_dir() {
                    sub_directories.push((path, db.dirs.len()));
                    db.push_dir(entry_info, parent);
                } else {
                    db.push_file(entry_info, parent);
                }
            }
        }

        sub_directories
            .par_iter()
            .for_each_with(database, |database, (path, index)| {
                walk_file_system(database.clone(), path, *index);
            });
    }
}

fn search(database: &Database, pattern: &Regex, in_path: bool) -> Vec<Hit> {
    let match_file = |(i, entry)| {
        if in_path {
            if let Some(path) = database.path_from_entry(entry).to_str() {
                if pattern.is_match(path) {
                    return Some(Hit::File(i));
                }
            }
        } else if pattern.is_match(&entry.name) {
            return Some(Hit::File(i));
        }
        None
    };

    let match_dir = |(i, entry)| {
        if in_path {
            if let Some(path) = database.path_from_entry(entry).to_str() {
                if pattern.is_match(path) {
                    return Some(Hit::Directory(i));
                }
            }
        } else if pattern.is_match(&entry.name) {
            return Some(Hit::Directory(i));
        }
        None
    };

    let files = (0..database.files.len())
        .into_par_iter()
        .zip(database.files.par_iter())
        .filter_map(match_file);
    let dirs = (0..database.dirs.len())
        .into_par_iter()
        .zip(database.dirs.par_iter())
        .filter_map(match_dir);

    files.chain(dirs).collect()
}
