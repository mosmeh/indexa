// partly taken from https://github.com/sharkdp/fd/blob/6f2c8cdf914aca3ec19809d5b661f124d2935900/src/regex_helper.rs

use regex_syntax::hir::Hir;
use regex_syntax::ParserBuilder;

/// Determine if a regex pattern contains a literal uppercase character.
pub fn pattern_has_uppercase_char(pattern: &str) -> bool {
    let mut parser = ParserBuilder::new().allow_invalid_utf8(true).build();

    parser
        .parse(pattern)
        .map(|hir| hir_has_uppercase_char(&hir))
        .unwrap_or(false)
}

/// Determine if a regex expression contains a literal uppercase character.
fn hir_has_uppercase_char(hir: &Hir) -> bool {
    use regex_syntax::hir::*;

    match *hir.kind() {
        HirKind::Literal(Literal::Unicode(c)) => c.is_uppercase(),
        HirKind::Literal(Literal::Byte(b)) => char::from(b).is_uppercase(),
        HirKind::Class(Class::Unicode(ref ranges)) => ranges
            .iter()
            .any(|r| r.start().is_uppercase() || r.end().is_uppercase()),
        HirKind::Class(Class::Bytes(ref ranges)) => ranges
            .iter()
            .any(|r| char::from(r.start()).is_uppercase() || char::from(r.end()).is_uppercase()),
        HirKind::Group(Group { ref hir, .. }) | HirKind::Repetition(Repetition { ref hir, .. }) => {
            hir_has_uppercase_char(hir)
        }
        HirKind::Concat(ref hirs) | HirKind::Alternation(ref hirs) => {
            hirs.iter().any(hir_has_uppercase_char)
        }
        _ => false,
    }
}

pub fn pattern_has_path_separator(pattern: &str) -> bool {
    let mut parser = ParserBuilder::new().allow_invalid_utf8(true).build();

    parser
        .parse(pattern)
        .map(|hir| hir_has_path_separator(&hir))
        .unwrap_or(false)
}

fn hir_has_path_separator(hir: &Hir) -> bool {
    use regex_syntax::hir::*;
    use std::path::MAIN_SEPARATOR;

    match *hir.kind() {
        HirKind::Literal(Literal::Unicode(c)) => c == MAIN_SEPARATOR,
        HirKind::Literal(Literal::Byte(b)) => char::from(b) == MAIN_SEPARATOR,
        HirKind::Class(Class::Unicode(ref ranges)) => ranges
            .iter()
            .any(|r| r.start() <= MAIN_SEPARATOR && MAIN_SEPARATOR <= r.end()),
        HirKind::Class(Class::Bytes(ref ranges)) => ranges.iter().any(|r| {
            char::from(r.start()) <= MAIN_SEPARATOR || MAIN_SEPARATOR <= char::from(r.end())
        }),
        HirKind::Group(Group { ref hir, .. }) | HirKind::Repetition(Repetition { ref hir, .. }) => {
            hir_has_path_separator(hir)
        }
        HirKind::Concat(ref hirs) | HirKind::Alternation(ref hirs) => {
            hirs.iter().any(hir_has_path_separator)
        }
        _ => false,
    }
}
