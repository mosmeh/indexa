use crate::database::{Entry, StatusKind};
use crate::{Error, Result};
use regex::{Regex, RegexBuilder};
use serde::{Deserialize, Serialize};
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

#[derive(Copy, Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SortOrder {
    Ascending,
    Descending,
}

pub struct QueryBuilder<'a> {
    string: Cow<'a, str>,
    match_path: bool,
    auto_match_path: bool,
    case_insensitive: bool,
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
            string: query_str.into(),
            match_path: false,
            auto_match_path: false,
            case_insensitive: false,
            regex: false,
            sort_by: StatusKind::Basename,
            sort_order: SortOrder::Ascending,
            sort_dirs_before_files: false,
        }
    }

    pub fn match_path(&mut self, yes: bool) -> &mut Self {
        self.match_path = yes;
        self
    }

    pub fn auto_match_path(&mut self, yes: bool) -> &mut Self {
        self.auto_match_path = yes;
        self
    }

    pub fn case_insensitive(&mut self, yes: bool) -> &mut Self {
        self.case_insensitive = yes;
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
        let regex = if self.regex {
            RegexBuilder::new(&self.string)
        } else {
            RegexBuilder::new(&regex::escape(&self.string))
        }
        .case_insensitive(self.case_insensitive)
        .build()?;

        Ok(Query {
            regex,
            match_path: should_match_path(
                self.match_path,
                self.auto_match_path,
                self.regex,
                &self.string,
            ),
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

fn should_match_path(match_path: bool, auto_inpath: bool, regex: bool, string: &str) -> bool {
    if match_path {
        return true;
    }
    if !auto_inpath {
        return false;
    }

    if regex && std::path::MAIN_SEPARATOR == '\\' {
        return string.contains(r"\\");
    }

    string.contains(std::path::MAIN_SEPARATOR)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn auto_match_path() {
        use std::path::MAIN_SEPARATOR as SEP;

        assert!(should_match_path(true, false, false, "foo"));
        assert!(should_match_path(
            false,
            true,
            false,
            &format!("foo{}bar", SEP)
        ));
        assert!(!should_match_path(false, true, false, "foo"));

        if SEP == '\\' {
            assert!(should_match_path(false, true, true, r"foo\\"));
            assert!(should_match_path(false, true, true, r"foo\\bar"));
            assert!(!should_match_path(false, true, true, r"foo\bar"));
        } else {
            assert!(should_match_path(false, true, true, &format!("foo{}", SEP)));
            assert!(should_match_path(
                false,
                true,
                true,
                &format!("foo{}bar", SEP)
            ));
        }
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

        let query = QueryBuilder::new("bar").match_path(true).build().unwrap();
        let match_detail = MatchDetail {
            query: &query,
            basename: "barbaz",
            path_str: "aaa/foobarbaz/barbaz".to_string(),
        };
        assert_eq!(match_detail.basename_matches(), vec![0..3]);
        assert_eq!(match_detail.path_matches(), vec![7..10, 14..17]);

        let query = QueryBuilder::new("[0-9]+")
            .match_path(true)
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
