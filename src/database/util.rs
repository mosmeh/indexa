use super::{Entry, StatusKind};

use std::cmp::Ordering;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

/// Canonicalize all paths.
/// Removes non-UTF-8 paths and redundant subdirectories.
pub fn canonicalize_dirs<P>(dirs: &[P]) -> Vec<PathBuf>
where
    P: AsRef<Path>,
{
    let mut dirs = dirs
        .iter()
        .filter_map(|path| {
            if let Ok(canonicalized) = dunce::canonicalize(path) {
                if let Some(path_str) = canonicalized.to_str() {
                    let path_str = path_str.to_string();
                    return Some((canonicalized, path_str));
                }
            }
            None
        })
        .collect::<Vec<_>>();

    // we use str::starts_with, because Path::starts_with doesn't work well for Windows paths
    dirs.sort_unstable_by(|(_, a), (_, b)| a.cmp(b));
    dirs.dedup_by(|(_, a), (_, b)| a.starts_with(&b as &str));

    dirs.iter().map(|(path, _)| path).cloned().collect()
}

pub fn get_basename(path: &Path) -> &std::ffi::OsStr {
    path.file_name().unwrap_or_else(|| path.as_os_str())
}

pub fn build_compare_func(
    kind: StatusKind,
) -> Box<dyn Fn(&Entry, &Entry) -> Ordering + Send + Sync> {
    match kind {
        StatusKind::Basename => Box::new(|a, b| a.basename().cmp(b.basename())),
        StatusKind::FullPath => Box::new(|a, b| a.path_vec().cmp(&b.path_vec())),
        StatusKind::Extension => Box::new(|a, b| {
            a.extension()
                .cmp(&b.extension())
                .then_with(|| a.basename().cmp(b.basename()))
        }),
        StatusKind::Size => Box::new(|a, b| {
            a.size()
                .cmp(&b.size())
                .then_with(|| a.basename().cmp(b.basename()))
        }),
        StatusKind::Mode => Box::new(|a, b| {
            a.mode()
                .cmp(&b.mode())
                .then_with(|| a.basename().cmp(b.basename()))
        }),
        StatusKind::Created => Box::new(|a, b| {
            a.created()
                .cmp(&b.created())
                .then_with(|| a.basename().cmp(b.basename()))
        }),
        StatusKind::Modified => Box::new(|a, b| {
            a.modified()
                .cmp(&b.modified())
                .then_with(|| a.basename().cmp(b.basename()))
        }),
        StatusKind::Accessed => Box::new(|a, b| {
            a.accessed()
                .cmp(&b.accessed())
                .then_with(|| a.basename().cmp(b.basename()))
        }),
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

    #[test]
    fn test_canonicalize_dirs() {
        use std::fs;

        let tmpdir = tempfile::tempdir().unwrap();
        let path = tmpdir.path();

        let dirs = vec![
            path.join("a"),
            path.join("a/b/.."),
            path.join("e"),
            path.join("b/c"),
            path.join("a/b"),
            PathBuf::new(),
            path.join("b/c/d"),
            path.join("e/a/b"),
            path.join("e/."),
        ];
        for dir in &dirs {
            fs::create_dir_all(dir).unwrap();
        }

        assert_eq!(
            canonicalize_dirs(&dirs),
            vec![path.join("a"), path.join("b/c"), path.join("e")]
                .iter()
                .map(|p| dunce::canonicalize(p).unwrap())
                .collect::<Vec<_>>()
        );

        assert!(canonicalize_dirs::<PathBuf>(&[]).is_empty());
        assert!(canonicalize_dirs(&[PathBuf::new()]).is_empty());

        let tmpdir = tempfile::tempdir().unwrap();
        let path = tmpdir.path();
        std::env::set_current_dir(path).unwrap();
        assert_eq!(
            canonicalize_dirs(&[Path::new(".")]),
            vec![dunce::canonicalize(path).unwrap()]
        );
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
