mod regex_helper;

use crate::{
    database::{Entry, StatusKind},
    Error, Result,
};
use regex::{Regex, RegexBuilder};
use serde::Deserialize;
use std::{borrow::Cow, ops::Range};

#[derive(Clone)]
pub struct Query {
    regex: Regex,
    match_path: bool,
    is_regex_enabled: bool,
    sort_by: StatusKind,
    sort_order: SortOrder,
    sort_dirs_before_files: bool,
    has_path_separator: bool,
}

impl Query {
    #[inline]
    pub fn regex(&self) -> &Regex {
        &self.regex
    }

    #[inline]
    pub fn match_path(&self) -> bool {
        self.match_path
    }

    #[inline]
    pub fn is_regex_enabled(&self) -> bool {
        self.is_regex_enabled
    }

    #[inline]
    pub fn sort_by(&self) -> StatusKind {
        self.sort_by
    }

    #[inline]
    pub fn sort_order(&self) -> SortOrder {
        self.sort_order
    }

    #[inline]
    pub fn sort_dirs_before_files(&self) -> bool {
        self.sort_dirs_before_files
    }

    #[inline]
    pub fn is_empty(&self) -> bool {
        self.regex.as_str().is_empty()
    }

    #[inline]
    pub fn has_path_separator(&self) -> bool {
        self.has_path_separator
    }

    #[inline]
    pub fn is_match(&self, entry: &Entry) -> bool {
        if self.match_path {
            entry
                .path()
                .to_str()
                .map(|s| self.regex.is_match(s))
                .unwrap_or(false)
        } else {
            self.regex.is_match(entry.basename())
        }
    }

    pub fn basename_matches(&self, entry: &Entry) -> Result<Vec<Range<usize>>> {
        if self.is_empty() {
            return Ok(Vec::new());
        }

        let basename = entry.basename();

        if self.match_path {
            let path = entry.path();
            let path_str = path.to_str().ok_or(Error::NonUtf8Path)?;

            Ok(self
                .regex
                .find_iter(path_str)
                .filter(|m| path_str.len() - m.end() < basename.len())
                .map(|m| Range {
                    start: basename.len().saturating_sub(path_str.len() - m.start()),
                    end: basename.len() - (path_str.len() - m.end()),
                })
                .collect())
        } else {
            Ok(self.regex.find_iter(basename).map(|m| m.range()).collect())
        }
    }

    pub fn path_matches(&self, entry: &Entry) -> Result<Vec<Range<usize>>> {
        if self.is_empty() {
            return Ok(Vec::new());
        }

        let path = entry.path();
        let path_str = path.to_str().ok_or(Error::NonUtf8Path)?;

        if self.match_path {
            Ok(self.regex.find_iter(path_str).map(|m| m.range()).collect())
        } else {
            let basename = entry.basename();

            Ok(self
                .regex
                .find_iter(basename)
                .map(|m| Range {
                    start: path_str.len() - basename.len() + m.start(),
                    end: path_str.len() - basename.len() + m.end(),
                })
                .collect())
        }
    }
}

#[derive(Copy, Clone, Debug, PartialEq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum MatchPathMode {
    #[serde(alias = "yes")]
    Always,
    #[serde(alias = "no")]
    Never,
    Auto,
}

#[derive(Copy, Clone, Debug)]
pub enum CaseSensitivity {
    Sensitive,
    Insensitive,
    Smart,
}

#[derive(Copy, Clone, Debug, PartialEq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SortOrder {
    #[serde(alias = "asc")]
    Ascending,
    #[serde(alias = "desc")]
    Descending,
}

pub struct QueryBuilder<'a> {
    pattern: Cow<'a, str>,
    match_path_mode: MatchPathMode,
    case_sensitivity: CaseSensitivity,
    is_regex_enabled: bool,
    sort_by: StatusKind,
    sort_order: SortOrder,
    sort_dirs_before_files: bool,
}

impl<'a> QueryBuilder<'a> {
    pub fn new<P>(query_str: P) -> Self
    where
        P: Into<Cow<'a, str>>,
    {
        Self {
            pattern: query_str.into(),
            match_path_mode: MatchPathMode::Never,
            case_sensitivity: CaseSensitivity::Smart,
            is_regex_enabled: false,
            sort_by: StatusKind::Basename,
            sort_order: SortOrder::Ascending,
            sort_dirs_before_files: false,
        }
    }

    pub fn match_path_mode(&mut self, match_path_mode: MatchPathMode) -> &mut Self {
        self.match_path_mode = match_path_mode;
        self
    }

    pub fn case_sensitivity(&mut self, case_sensitivity: CaseSensitivity) -> &mut Self {
        self.case_sensitivity = case_sensitivity;
        self
    }

    pub fn regex(&mut self, yes: bool) -> &mut Self {
        self.is_regex_enabled = yes;
        self
    }

    pub fn sort_by(&mut self, kind: StatusKind) -> &mut Self {
        self.sort_by = kind;
        self
    }

    pub fn sort_order(&mut self, order: SortOrder) -> &mut Self {
        self.sort_order = order;
        self
    }

    pub fn sort_dirs_before_files(&mut self, yes: bool) -> &mut Self {
        self.sort_dirs_before_files = yes;
        self
    }

    pub fn build(&self) -> Result<Query> {
        let case_sensitive =
            should_be_case_sensitive(self.case_sensitivity, self.is_regex_enabled, &self.pattern);

        let regex = if self.is_regex_enabled {
            RegexBuilder::new(&self.pattern)
        } else {
            RegexBuilder::new(&regex::escape(&self.pattern))
        }
        .case_insensitive(!case_sensitive)
        .build()?;

        let has_path_separator = pattern_has_path_separator(&self.pattern, self.is_regex_enabled);

        Ok(Query {
            regex,
            match_path: should_match_path(self.match_path_mode, has_path_separator),
            is_regex_enabled: self.is_regex_enabled,
            sort_by: self.sort_by,
            sort_order: self.sort_order,
            sort_dirs_before_files: self.sort_dirs_before_files,
            has_path_separator,
        })
    }
}

fn pattern_has_path_separator(pattern: &str, is_regex_enabled: bool) -> bool {
    if is_regex_enabled {
        regex_helper::pattern_has_path_separator(pattern)
    } else {
        pattern.contains(std::path::MAIN_SEPARATOR)
    }
}

fn should_match_path(match_path_mode: MatchPathMode, has_path_separator: bool) -> bool {
    match match_path_mode {
        MatchPathMode::Always => true,
        MatchPathMode::Never => false,
        MatchPathMode::Auto => has_path_separator,
    }
}

fn should_be_case_sensitive(
    case_sensitivity: CaseSensitivity,
    is_regex_enabled: bool,
    pattern: &str,
) -> bool {
    match case_sensitivity {
        CaseSensitivity::Sensitive => true,
        CaseSensitivity::Insensitive => false,
        CaseSensitivity::Smart => {
            if is_regex_enabled {
                regex_helper::pattern_has_uppercase_char(pattern)
            } else {
                pattern.chars().any(|c| c.is_uppercase())
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::database::*;
    use std::{fs, path::Path};
    use tempfile::TempDir;

    #[test]
    fn match_path() {
        use std::path::MAIN_SEPARATOR;

        fn match_path(
            match_path_mode: MatchPathMode,
            is_regex_enabled: bool,
            pattern: &str,
        ) -> bool {
            let has_path_separator = pattern_has_path_separator(pattern, is_regex_enabled);
            should_match_path(match_path_mode, has_path_separator)
        }

        assert!(match_path(MatchPathMode::Always, false, "foo"));
        assert!(match_path(
            MatchPathMode::Auto,
            false,
            &format!("foo{}bar", MAIN_SEPARATOR)
        ));
        assert!(!match_path(MatchPathMode::Auto, false, "foo"));

        if regex_syntax::is_meta_character(MAIN_SEPARATOR) {
            // typically Windows, where MAIN_SEPARATOR is \

            assert!(match_path(
                MatchPathMode::Auto,
                true,
                &regex::escape(&format!(r"foo{}", MAIN_SEPARATOR))
            ));
            assert!(match_path(
                MatchPathMode::Auto,
                true,
                &regex::escape(&format!(r"foo{}bar", MAIN_SEPARATOR))
            ));
            assert!(match_path(MatchPathMode::Auto, true, r"."));
            assert!(!match_path(
                MatchPathMode::Auto,
                true,
                &format!(r"foo{}", MAIN_SEPARATOR)
            ));
            assert!(!match_path(
                MatchPathMode::Auto,
                true,
                &format!(r"[^{}]", regex::escape(&MAIN_SEPARATOR.to_string()))
            ));
        } else {
            assert!(match_path(
                MatchPathMode::Auto,
                true,
                &format!("foo{}", MAIN_SEPARATOR)
            ));
            assert!(match_path(
                MatchPathMode::Auto,
                true,
                &format!("foo{}bar", MAIN_SEPARATOR)
            ));
            assert!(match_path(MatchPathMode::Auto, true, r"."));
            assert!(!match_path(
                MatchPathMode::Auto,
                true,
                &format!(r"[^{}]", MAIN_SEPARATOR)
            ));
        }
    }

    #[test]
    fn case_sensitive() {
        assert!(should_be_case_sensitive(
            CaseSensitivity::Sensitive,
            false,
            "foo"
        ));

        assert!(!should_be_case_sensitive(
            CaseSensitivity::Insensitive,
            false,
            "foo"
        ));

        assert!(should_be_case_sensitive(
            CaseSensitivity::Smart,
            false,
            "Foo"
        ));

        assert!(!should_be_case_sensitive(
            CaseSensitivity::Smart,
            false,
            "foo"
        ));

        assert!(should_be_case_sensitive(
            CaseSensitivity::Smart,
            true,
            "[A-Z]x"
        ));

        assert!(!should_be_case_sensitive(
            CaseSensitivity::Smart,
            true,
            "[a-z]x"
        ));
    }

    fn create_dir_structure<P>(dirs: &[P]) -> TempDir
    where
        P: AsRef<Path>,
    {
        let tmpdir = tempfile::tempdir().unwrap();
        let path = tmpdir.path();

        for dir in dirs {
            fs::create_dir_all(path.join(dir)).unwrap();
        }

        tmpdir
    }

    #[test]
    fn match_ranges() {
        let tmpdir = create_dir_structure(&[
            Path::new("aaa/foobarbaz/barbaz"),
            Path::new("0042bar/a/foo123bar"),
        ]);
        let path = dunce::canonicalize(tmpdir.path()).unwrap();
        let prefix = path.to_str().unwrap();
        let prefix_len = if prefix.ends_with(std::path::MAIN_SEPARATOR) {
            prefix.len()
        } else {
            prefix.len() + 1
        };

        let database = DatabaseBuilder::new()
            .add_dir(tmpdir.path())
            .build()
            .unwrap();

        let search = |query| {
            database
                .search(query)
                .unwrap()
                .into_iter()
                .map(|id| database.entry(id))
                .collect::<Vec<_>>()
        };

        let query = QueryBuilder::new("bar").build().unwrap();
        let entries = search(&query);
        assert_eq!(
            entries
                .iter()
                .map(|entry| entry.basename())
                .collect::<Vec<_>>(),
            vec!["0042bar", "barbaz", "foo123bar", "foobarbaz"]
        );
        let entry = &entries[1];
        assert_eq!(query.basename_matches(&entry).unwrap(), vec![0..3]);
        assert_eq!(
            query.path_matches(&entry).unwrap(),
            vec![prefix_len + 14..prefix_len + 17]
        );

        let query = QueryBuilder::new("bar")
            .match_path_mode(MatchPathMode::Always)
            .build()
            .unwrap();
        let entry = search(&query)
            .into_iter()
            .find(|entry| entry.basename() == "barbaz")
            .unwrap();
        assert_eq!(query.basename_matches(&entry).unwrap(), vec![0..3]);
        assert_eq!(
            query
                .path_matches(&entry)
                .unwrap()
                .into_iter()
                .filter(|range| { prefix_len <= range.start })
                .collect::<Vec<_>>(),
            vec![
                prefix_len + 7..prefix_len + 10,
                prefix_len + 14..prefix_len + 17
            ]
        );

        let query = QueryBuilder::new("[0-9]+")
            .match_path_mode(MatchPathMode::Always)
            .regex(true)
            .build()
            .unwrap();
        let entry = search(&query)
            .into_iter()
            .find(|entry| entry.basename() == "foo123bar")
            .unwrap();
        assert_eq!(query.basename_matches(&entry).unwrap(), vec![3..6]);
        assert_eq!(
            query
                .path_matches(&entry)
                .unwrap()
                .into_iter()
                .filter(|range| { prefix_len <= range.start })
                .collect::<Vec<_>>(),
            vec![prefix_len..prefix_len + 4, prefix_len + 13..prefix_len + 16]
        );
    }
}
