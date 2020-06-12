use crate::database::Entry;
use crate::{Error, Result};
use regex::{Regex, RegexBuilder};
use std::borrow::Cow;
use std::ops::Range;

#[derive(Clone)]
pub struct Matcher {
    pub(crate) query: Regex,
    pub(crate) match_path: bool,
}

impl Matcher {
    pub fn query_is_empty(&self) -> bool {
        self.query.as_str().is_empty()
    }

    pub fn is_match(&self, entry: &Entry) -> bool {
        if self.match_path {
            entry
                .path()
                .to_str()
                .map(|s| self.query.is_match(s))
                .unwrap_or(false)
        } else {
            self.query.is_match(entry.basename())
        }
    }

    pub fn match_detail<'a, 'b>(&'a self, entry: &'b Entry) -> Result<MatchDetail<'a, 'b>> {
        Ok(MatchDetail {
            query: &self.query,
            match_path: self.match_path,
            basename: entry.basename(),
            path_str: entry.path().to_str().ok_or(Error::Utf8)?.to_string(),
        })
    }
}

pub struct MatcherBuilder<'a> {
    query_str: Cow<'a, str>,
    match_path: bool,
    auto_match_path: bool,
    case_insensitive: bool,
    regex: bool,
}

impl<'a> MatcherBuilder<'a> {
    pub fn new<P>(query_str: P) -> Self
    where
        P: Into<Cow<'a, str>>,
    {
        Self {
            query_str: query_str.into(),
            match_path: false,
            auto_match_path: false,
            case_insensitive: false,
            regex: false,
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

    pub fn build(&self) -> Result<Matcher> {
        let regex = if self.regex {
            RegexBuilder::new(&self.query_str)
        } else {
            RegexBuilder::new(&regex::escape(&self.query_str))
        }
        .case_insensitive(self.case_insensitive)
        .build()?;

        Ok(Matcher {
            query: regex,
            match_path: should_search_match_path(
                self.match_path,
                self.auto_match_path,
                self.regex,
                &self.query_str,
            ),
        })
    }
}

pub struct MatchDetail<'a, 'b> {
    query: &'a Regex,
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
            self.query
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
                .find_iter(self.basename)
                .map(|m| m.range())
                .collect()
        }
    }

    pub fn path_matches(&self) -> Vec<Range<usize>> {
        if self.match_path {
            self.query
                .find_iter(&self.path_str)
                .map(|m| m.range())
                .collect()
        } else {
            self.query
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
    query_str: &str,
) -> bool {
    if match_path {
        return true;
    }
    if !auto_inpath {
        return false;
    }

    if regex && std::path::MAIN_SEPARATOR == '\\' {
        return query_str.contains("\\\\");
    }

    query_str.contains(std::path::MAIN_SEPARATOR)
}
