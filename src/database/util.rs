use super::{Entry, StatusKind};
use crate::{Error, Result};

use std::{
    cmp::Ordering,
    path::{Path, PathBuf},
    time::SystemTime,
};

/// Canonicalize all paths and remove all redundant subdirectories
pub fn canonicalize_dirs<P>(dirs: &[P]) -> Result<Vec<PathBuf>>
where
    P: AsRef<Path>,
{
    let mut dirs = dirs
        .iter()
        .map(|path| {
            let canonicalized = dunce::canonicalize(path)?;
            let path_str = canonicalized
                .to_str()
                .ok_or(Error::NonUtf8Path)?
                .to_string();
            Ok((canonicalized, path_str))
        })
        .collect::<Result<Vec<_>>>()?;

    // we use str::starts_with, because Path::starts_with doesn't work well for Windows paths
    dirs.sort_unstable_by(|(_, a), (_, b)| a.cmp(b));
    dirs.dedup_by(|(_, a), (_, b)| a.starts_with(b as &str));

    Ok(dirs.into_iter().map(|(path, _)| path).collect())
}

pub fn get_basename(path: &Path) -> &std::ffi::OsStr {
    path.file_name().unwrap_or_else(|| path.as_os_str())
}

pub fn get_compare_func(kind: StatusKind) -> fn(&Entry, &Entry) -> Ordering {
    #[inline]
    fn cmp_by_basename(a: &Entry, b: &Entry) -> Ordering {
        Ord::cmp(a.basename(), b.basename())
    }
    fn cmp_by_path(a: &Entry, b: &Entry) -> Ordering {
        Ord::cmp(&a.path_vec(), &b.path_vec())
    }
    fn cmp_by_extension(a: &Entry, b: &Entry) -> Ordering {
        Ord::cmp(&a.extension(), &b.extension())
    }
    fn cmp_by_size(a: &Entry, b: &Entry) -> Ordering {
        Ord::cmp(&a.size().ok(), &b.size().ok()).then_with(|| cmp_by_basename(a, b))
    }
    fn cmp_by_mode(a: &Entry, b: &Entry) -> Ordering {
        Ord::cmp(&a.mode().ok(), &b.mode().ok()).then_with(|| cmp_by_basename(a, b))
    }
    fn cmp_by_created(a: &Entry, b: &Entry) -> Ordering {
        Ord::cmp(&a.created().ok(), &b.created().ok()).then_with(|| cmp_by_basename(a, b))
    }
    fn cmp_by_modified(a: &Entry, b: &Entry) -> Ordering {
        Ord::cmp(&a.modified().ok(), &b.modified().ok()).then_with(|| cmp_by_basename(a, b))
    }
    fn cmp_by_accessed(a: &Entry, b: &Entry) -> Ordering {
        Ord::cmp(&a.accessed().ok(), &b.accessed().ok()).then_with(|| cmp_by_basename(a, b))
    }

    match kind {
        StatusKind::Basename => cmp_by_basename,
        StatusKind::Path => cmp_by_path,
        StatusKind::Extension => cmp_by_extension,
        StatusKind::Size => cmp_by_size,
        StatusKind::Mode => cmp_by_mode,
        StatusKind::Created => cmp_by_created,
        StatusKind::Modified => cmp_by_modified,
        StatusKind::Accessed => cmp_by_accessed,
    }
}

/// check for invalid SystemTime (e.g. older than unix epoch) and fix them
pub fn sanitize_system_time(time: &SystemTime) -> SystemTime {
    if let Ok(duration) = time.duration_since(SystemTime::UNIX_EPOCH) {
        SystemTime::UNIX_EPOCH + duration
    } else {
        // defaults to unix epoch
        SystemTime::UNIX_EPOCH
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::{Path, PathBuf};

    #[test]
    fn test_canonicalize_dirs() {
        let tmpdir = tempfile::tempdir().unwrap();
        let path = tmpdir.path();

        let dirs = vec![
            path.join("a"),
            path.join("a/b/.."),
            path.join("e"),
            path.join("b/c"),
            path.join("a/b"),
            path.join("b/c/d"),
            path.join("e/a/b"),
            path.join("e/."),
        ];
        for dir in &dirs {
            std::fs::create_dir_all(dir).unwrap();
        }

        assert_eq!(
            canonicalize_dirs(&dirs).unwrap(),
            vec![path.join("a"), path.join("b/c"), path.join("e")]
                .iter()
                .map(|p| dunce::canonicalize(p).unwrap())
                .collect::<Vec<_>>()
        );

        assert!(canonicalize_dirs::<PathBuf>(&[]).unwrap().is_empty());

        let tmpdir = tempfile::tempdir().unwrap();
        let path = tmpdir.path();
        std::env::set_current_dir(path).unwrap();
        assert_eq!(
            canonicalize_dirs(&[Path::new(".")]).unwrap(),
            vec![dunce::canonicalize(path).unwrap()]
        );
    }

    #[test]
    #[should_panic]
    fn canonicalize_non_existent_dir() {
        let tmpdir = tempfile::tempdir().unwrap();
        let dir = tmpdir.path().join("xxxx");
        canonicalize_dirs(&[dir]).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn test_get_basename() {
        assert_eq!("/", get_basename(Path::new("/")));
        assert_eq!("foo", get_basename(Path::new("/foo")));
        assert_eq!("bar", get_basename(Path::new("/foo/bar")));
    }

    #[cfg(windows)]
    #[test]
    fn test_get_basename() {
        assert_eq!(r"C:\", get_basename(Path::new(r"C:\")));
        assert_eq!("foo", get_basename(Path::new(r"C:\foo")));
        assert_eq!("bar", get_basename(Path::new(r"C:\foo\bar")));
        assert_eq!(
            r"\\server\share\",
            get_basename(Path::new(r"\\server\share\"))
        );
        assert_eq!("foo", get_basename(Path::new(r"\\server\share\foo")));
    }
}
