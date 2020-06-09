use crate::database::Entry;
use crate::{Error, Result};
use regex::{Regex, RegexBuilder};
use std::borrow::Cow;
use std::ops::Range;

#[derive(Clone)]
pub struct Matcher {
    pub(crate) pattern: Regex,
    pub(crate) in_path: bool,
}

impl Matcher {
    pub fn is_match(&self, entry: &Entry) -> bool {
        if self.in_path {
            entry
                .path()
                .to_str()
                .map(|s| self.pattern.is_match(s))
                .unwrap_or(false)
        } else {
            self.pattern.is_match(entry.basename())
        }
    }

    pub fn match_detail<'a, 'b>(&'a self, entry: &'b Entry) -> Result<MatchDetail<'a, 'b>> {
        Ok(MatchDetail {
            pattern: &self.pattern,
            in_path: self.in_path,
            basename: entry.basename(),
            path_str: entry.path().to_str().ok_or(Error::Utf8)?.to_string(),
        })
    }
}

pub struct MatcherBuilder<'a> {
    pattern_str: Cow<'a, str>,
    in_path: bool,
    case_insensitive: bool,
    regex: bool,
}

impl<'a> MatcherBuilder<'a> {
    pub fn new<P>(pattern_str: P) -> Self
    where
        P: Into<Cow<'a, str>>,
    {
        Self {
            pattern_str: pattern_str.into(),
            in_path: false,
            case_insensitive: false,
            regex: false,
        }
    }

    pub fn in_path(&mut self, yes: bool) -> &mut Self {
        self.in_path = yes;
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
            RegexBuilder::new(&self.pattern_str)
        } else {
            RegexBuilder::new(&regex::escape(&self.pattern_str))
        }
        .case_insensitive(self.case_insensitive)
        .build()?;

        Ok(Matcher {
            pattern: regex,
            in_path: self.in_path,
        })
    }
}

pub struct MatchDetail<'a, 'b> {
    pattern: &'a Regex,
    in_path: bool,
    basename: &'b str,
    path_str: String,
}

impl MatchDetail<'_, '_> {
    pub fn path_str(&self) -> &str {
        &self.path_str
    }

    pub fn basename_matches(&self) -> Vec<Range<usize>> {
        if self.in_path {
            self.pattern
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
            self.pattern
                .find_iter(self.basename)
                .map(|m| m.range())
                .collect()
        }
    }

    pub fn path_matches(&self) -> Vec<Range<usize>> {
        if self.in_path {
            self.pattern
                .find_iter(&self.path_str)
                .map(|m| m.range())
                .collect()
        } else {
            self.pattern
                .find_iter(self.basename)
                .map(|m| Range {
                    start: self.path_str.len() - self.basename.len() + m.start(),
                    end: self.path_str.len() - self.basename.len() + m.end(),
                })
                .collect()
        }
    }
}
