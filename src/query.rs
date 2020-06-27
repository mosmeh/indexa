mod regex_helper;

use crate::database::{Entry, StatusKind};
use crate::{Error, Result};
use regex::{Regex, RegexBuilder};
use serde::Deserialize;
use std::borrow::Cow;
use std::ops::Range;

#[derive(Clone)]
pub struct Query {
    regex: Regex,
    match_path: bool,
    sort_by: StatusKind,
    sort_order: SortOrder,
    sort_dirs_before_files: bool,
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

    #[inline]
    pub fn match_detail<'a, 'b>(&'a self, entry: &'b Entry) -> Result<MatchDetail<'a, 'b>> {
        Ok(MatchDetail {
            query: &self,
            basename: entry.basename(),
            path_str: entry.path().to_str().ok_or(Error::NonUtf8Path)?.to_string(),
        })
    }
}

#[derive(Copy, Clone, Debug)]
pub enum MatchPathMode {
    Always,
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
    Ascending,
    Descending,
}

pub struct QueryBuilder<'a> {
    pattern: Cow<'a, str>,
    match_path_mode: MatchPathMode,
    case_sensitivity: CaseSensitivity,
    regex: bool,
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
            regex: false,
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
        self.regex = yes;
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
            should_be_case_sensitive(self.case_sensitivity, self.regex, &self.pattern);

        let regex = if self.regex {
            RegexBuilder::new(&self.pattern)
        } else {
            RegexBuilder::new(&regex::escape(&self.pattern))
        }
        .case_insensitive(!case_sensitive)
        .build()?;

        Ok(Query {
            regex,
            match_path: should_match_path(self.match_path_mode, self.regex, &self.pattern),
            sort_by: self.sort_by,
            sort_order: self.sort_order,
            sort_dirs_before_files: self.sort_dirs_before_files,
        })
    }
}

pub struct MatchDetail<'a, 'b> {
    query: &'a Query,
    basename: &'b str,
    path_str: String,
}

impl MatchDetail<'_, '_> {
    pub fn path_str(&self) -> &str {
        &self.path_str
    }

    pub fn basename_matches(&self) -> Vec<Range<usize>> {
        if self.query.is_empty() {
            Vec::new()
        } else if self.query.match_path() {
            self.query
                .regex()
                .find_iter(&self.path_str)
                .filter_map(|m| {
                    if self.path_str.len() - m.end() < self.basename.len() {
                        Some(Range {
                            start: self
                                .basename
                                .len()
                                .saturating_sub(self.path_str.len() - m.start()),
                            end: self.basename.len() - (self.path_str.len() - m.end()),
                        })
                    } else {
                        None
                    }
                })
                .collect()
        } else {
            self.query
                .regex()
                .find_iter(self.basename)
                .map(|m| m.range())
                .collect()
        }
    }

    pub fn path_matches(&self) -> Vec<Range<usize>> {
        if self.query.is_empty() {
            Vec::new()
        } else if self.query.match_path() {
            self.query
                .regex()
                .find_iter(&self.path_str)
                .map(|m| m.range())
                .collect()
        } else {
            self.query
                .regex()
                .find_iter(self.basename)
                .map(|m| Range {
                    start: self.path_str.len() - self.basename.len() + m.start(),
                    end: self.path_str.len() - self.basename.len() + m.end(),
                })
                .collect()
        }
    }
}

fn should_match_path(match_path_mode: MatchPathMode, regex: bool, pattern: &str) -> bool {
    match match_path_mode {
        MatchPathMode::Always => true,
        MatchPathMode::Never => false,
        MatchPathMode::Auto => {
            if regex && std::path::MAIN_SEPARATOR == '\\' {
                pattern.contains(r"\\")
            } else {
                pattern.contains(std::path::MAIN_SEPARATOR)
            }
        }
    }
}

fn should_be_case_sensitive(case_sensitivity: CaseSensitivity, regex: bool, pattern: &str) -> bool {
    match case_sensitivity {
        CaseSensitivity::Sensitive => true,
        CaseSensitivity::Insensitive => false,
        CaseSensitivity::Smart => {
            if regex {
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

    #[test]
    fn match_path() {
        use std::path::MAIN_SEPARATOR;

        assert!(should_match_path(MatchPathMode::Always, false, "foo"));
        assert!(should_match_path(
            MatchPathMode::Auto,
            false,
            &format!("foo{}bar", MAIN_SEPARATOR)
        ));
        assert!(!should_match_path(MatchPathMode::Auto, false, "foo"));

        if MAIN_SEPARATOR == '\\' {
            assert!(should_match_path(MatchPathMode::Auto, true, r"foo\\"));
            assert!(should_match_path(MatchPathMode::Auto, true, r"foo\\bar"));
            assert!(!should_match_path(MatchPathMode::Auto, true, r"foo\bar"));
        } else {
            assert!(should_match_path(
                MatchPathMode::Auto,
                true,
                &format!("foo{}", MAIN_SEPARATOR)
            ));
            assert!(should_match_path(
                MatchPathMode::Auto,
                true,
                &format!("foo{}bar", MAIN_SEPARATOR)
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

    #[test]
    fn match_detail() {
        let query = QueryBuilder::new("bar").build().unwrap();
        let match_detail = MatchDetail {
            query: &query,
            basename: "barbaz",
            path_str: "aaa/foobarbaz/barbaz".to_string(),
        };
        assert_eq!(match_detail.basename_matches(), vec![0..3]);
        assert_eq!(match_detail.path_matches(), vec![14..17]);

        let query = QueryBuilder::new("bar")
            .match_path_mode(MatchPathMode::Always)
            .build()
            .unwrap();
        let match_detail = MatchDetail {
            query: &query,
            basename: "barbaz",
            path_str: "aaa/foobarbaz/barbaz".to_string(),
        };
        assert_eq!(match_detail.basename_matches(), vec![0..3]);
        assert_eq!(match_detail.path_matches(), vec![7..10, 14..17]);

        let query = QueryBuilder::new("[0-9]+")
            .match_path_mode(MatchPathMode::Always)
            .regex(true)
            .build()
            .unwrap();
        let match_detail = MatchDetail {
            query: &query,
            basename: "foo123bar",
            path_str: "0042bar/a/foo123bar".to_string(),
        };
        assert_eq!(match_detail.basename_matches(), vec![3..6]);
        assert_eq!(match_detail.path_matches(), vec![0..4, 13..16]);
    }
}
