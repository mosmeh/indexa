mod basename;
mod component_wise_path;
mod full_path;
mod passthrough;
mod regex_path;

pub use basename::BasenameFilter;
pub use component_wise_path::ComponentWisePathFilter;
pub use full_path::FullPathFilter;
pub use passthrough::PassthroughFilter;
pub use regex_path::RegexPathFilter;

use crate::{
    database::{Database, EntryNode},
    Error, Result,
};

use rayon::prelude::*;
use regex::Regex;
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc,
};
use thread_local::ThreadLocal;

pub(crate) struct FilterContext<'d, 'a, 'r> {
    database: &'d Database,
    abort_signal: &'a Arc<AtomicBool>,
    regex: &'r Regex,

    // Since rust-lang/regex@e040c1b, regex library stopped using thread_local,
    // which had a performance impact on indexa.
    // We mitigate it by putting Regex in thread local storage.
    regex_tls: ThreadLocal<Regex>,
}

impl<'d, 'a, 'r> FilterContext<'d, 'a, 'r> {
    pub fn new(
        database: &'d Database,
        abort_signal: &'a Arc<AtomicBool>,
        regex: &'r Regex,
    ) -> Self {
        Self {
            database,
            abort_signal,
            regex,
            regex_tls: ThreadLocal::with_capacity(rayon::current_num_threads() + 1),
        }
    }

    fn thread_local_regex(&self) -> &Regex {
        self.regex_tls.get_or(|| self.regex.clone())
    }
}

// Filters can choose to directly implement `Filter` or
// implement `MatchEntries` instead.

pub(crate) trait Filter {
    /// Returns filtered ids without changing an order.
    fn ordered(ctx: &FilterContext, ids: impl ParallelIterator<Item = u32>) -> Result<Vec<u32>>;

    /// Returns filtered ids in an arbitrary order.
    fn unordered(ctx: &FilterContext) -> Result<Vec<u32>>;
}

pub(crate) trait MatchEntries: Filter {
    fn match_entries(ctx: &FilterContext, matched: &[AtomicBool]) -> Result<()>;
}

impl<T: MatchEntries> Filter for T {
    fn ordered(ctx: &FilterContext, ids: impl ParallelIterator<Item = u32>) -> Result<Vec<u32>> {
        let nodes = &ctx.database.nodes;
        let mut matched = Vec::with_capacity(nodes.len());
        for _ in 0..nodes.len() {
            matched.push(AtomicBool::new(false));
        }

        Self::match_entries(ctx, &matched)?;

        ids.filter_map(|id| {
            if ctx.abort_signal.load(Ordering::Relaxed) {
                return Some(Err(Error::SearchAbort));
            }

            matched[id as usize].load(Ordering::Relaxed).then(|| Ok(id))
        })
        .collect()
    }

    fn unordered(ctx: &FilterContext) -> Result<Vec<u32>> {
        let nodes = &ctx.database.nodes;
        let mut matched = Vec::with_capacity(nodes.len());
        for _ in 0..nodes.len() {
            matched.push(AtomicBool::new(false));
        }

        Self::match_entries(ctx, &matched)?;

        (0..ctx.database.num_entries() as u32)
            .into_par_iter()
            .zip(matched.into_par_iter())
            .filter_map(|(id, m)| {
                if ctx.abort_signal.load(Ordering::Relaxed) {
                    return Some(Err(Error::SearchAbort));
                }

                m.load(Ordering::Relaxed).then(|| Ok(id))
            })
            .collect()
    }
}

fn match_all_descendants(
    ctx: &FilterContext,
    matched: &[AtomicBool],
    node: &EntryNode,
) -> Result<()> {
    let children_range = node.child_start as usize..node.child_end as usize;
    (
        &ctx.database.nodes[children_range.clone()],
        &matched[children_range],
    )
        .into_par_iter()
        .try_for_each(|(node, m)| {
            if ctx.abort_signal.load(Ordering::Relaxed) {
                return Err(Error::SearchAbort);
            }

            m.store(true, Ordering::Relaxed);
            if node.has_any_child() {
                match_all_descendants(ctx, matched, node)?;
            }

            Ok(())
        })
}
