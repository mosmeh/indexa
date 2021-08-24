// idea from https://github.com/sharkdp/fd/blob/6f2c8cdf914aca3ec19809d5b661f124d2935900/src/regex_helper.rs

use regex_syntax::hir::{Class, Group, Hir, HirKind, Literal, Repetition};

pub fn hir_has_path_separator(hir: &Hir) -> bool {
    use std::path::MAIN_SEPARATOR;

    match hir.kind() {
        HirKind::Literal(Literal::Unicode(c)) => *c == MAIN_SEPARATOR,
        HirKind::Literal(Literal::Byte(b)) => char::from(*b) == MAIN_SEPARATOR,
        HirKind::Class(Class::Unicode(ranges)) => ranges
            .iter()
            .any(|r| r.start() <= MAIN_SEPARATOR && MAIN_SEPARATOR <= r.end()),
        HirKind::Class(Class::Bytes(ranges)) => ranges.iter().any(|r| {
            char::from(r.start()) <= MAIN_SEPARATOR && MAIN_SEPARATOR <= char::from(r.end())
        }),
        HirKind::Group(Group { hir, .. }) | HirKind::Repetition(Repetition { hir, .. }) => {
            hir_has_path_separator(hir)
        }
        HirKind::Concat(hirs) | HirKind::Alternation(hirs) => {
            hirs.iter().any(hir_has_path_separator)
        }
        _ => false,
    }
}

pub fn hir_has_uppercase_char(hir: &Hir) -> bool {
    match hir.kind() {
        HirKind::Literal(Literal::Unicode(c)) => c.is_uppercase(),
        HirKind::Literal(Literal::Byte(b)) => char::from(*b).is_uppercase(),
        HirKind::Class(Class::Unicode(ranges)) => ranges
            .iter()
            .any(|r| r.start().is_uppercase() || r.end().is_uppercase()),
        HirKind::Class(Class::Bytes(ranges)) => ranges
            .iter()
            .any(|r| char::from(r.start()).is_uppercase() || char::from(r.end()).is_uppercase()),
        HirKind::Group(Group { hir, .. }) | HirKind::Repetition(Repetition { hir, .. }) => {
            hir_has_uppercase_char(hir)
        }
        HirKind::Concat(hirs) | HirKind::Alternation(hirs) => {
            hirs.iter().any(hir_has_uppercase_char)
        }
        _ => false,
    }
}
