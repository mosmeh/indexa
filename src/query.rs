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
    dirs_before_files: bool,
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
    pub fn dirs_before_files(&self) -> bool {
        self.dirs_before_files
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
            regex: &self.regex,
            match_path: self.match_path,
            basename: entry.basename(),
            path_str: entry.path().to_str().ok_or(Error::Utf8)?.to_string(),
        })
    }
}

#[derive(Copy, Clone, Debug, Serialize, Deserialize)]
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
    dirs_before_files: bool,
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
            dirs_before_files: false,
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

    pub fn dirs_before_files(&mut self, yes: bool) -> &mut Self {
        self.dirs_before_files = yes;
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
            match_path: should_search_match_path(
                self.match_path,
                self.auto_match_path,
                self.regex,
                &self.string,
            ),
            sort_by: self.sort_by,
            sort_order: self.sort_order,
            dirs_before_files: self.dirs_before_files,
        })
    }
}

pub struct MatchDetail<'a, 'b> {
    regex: &'a Regex,
    match_path: bool,
    basename: &'b str,
    path_str: String,
}

impl MatchDetail<'_, '_> {
    pub fn path_str(&self) -> &str {
        &self.path_str
    }

    pub fn basename_matches(&self) -> Vec<Range<usize>> {
        if self.match_path {
            self.regex
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
            self.regex
                .find_iter(self.basename)
                .map(|m| m.range())
                .collect()
        }
    }

    pub fn path_matches(&self) -> Vec<Range<usize>> {
        if self.match_path {
            self.regex
                .find_iter(&self.path_str)
                .map(|m| m.range())
                .collect()
        } else {
            self.regex
                .find_iter(self.basename)
                .map(|m| Range {
                    start: self.path_str.len() - self.basename.len() + m.start(),
                    end: self.path_str.len() - self.basename.len() + m.end(),
                })
                .collect()
        }
    }
}

fn should_search_match_path(
    match_path: bool,
    auto_inpath: bool,
    regex: bool,
    string: &str,
) -> bool {
    if match_path {
        return true;
    }
    if !auto_inpath {
        return false;
    }

    if regex && std::path::MAIN_SEPARATOR == '\\' {
        return string.contains("\\\\");
    }

    string.contains(std::path::MAIN_SEPARATOR)
}