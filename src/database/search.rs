use super::{util, Database, EntryId, EntryNode};
use crate::{
    query::{Query, SortOrder},
    Error, Result,
};

use rayon::prelude::*;
use regex::Regex;
use std::{
    path::Path,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
};
use thread_local::ThreadLocal;

impl Database {
    pub fn search(&self, query: &Query, abort_signal: &Arc<AtomicBool>) -> Result<Vec<EntryId>> {
        if query.is_empty() {
            return self.filter_and_sort::<AllFilter>(query, abort_signal);
        }
        if !query.match_path() {
            return self.filter_and_sort::<BasenameFilter>(query, abort_signal);
        }
        if query.is_regex_enabled() {
            return self.filter_and_sort::<RegexPathFilter>(query, abort_signal);
        }
        self.filter_and_sort::<PathFilter>(query, abort_signal)
    }

    fn filter_and_sort<F: Filter>(
        &self,
        query: &Query,
        abort_signal: &Arc<AtomicBool>,
    ) -> Result<Vec<EntryId>> {
        let ctx = FilterContext {
            database: self,
            abort_signal,
            regex: query.regex(),
        };

        let mut hits = if let Some(ids) = self.sorted_ids[query.sort_by()].as_ref() {
            match query.sort_order() {
                SortOrder::Ascending => F::ordered(&ctx, ids.into_par_iter().copied())?,
                SortOrder::Descending => F::ordered(&ctx, ids.into_par_iter().rev().copied())?,
            }
        } else {
            let mut hits = F::unordered(&ctx)?;

            let compare_func = util::get_compare_func(query.sort_by());
            let slice = hits.as_parallel_slice_mut();
            match query.sort_order() {
                SortOrder::Ascending => slice.par_sort_unstable_by(|a, b| {
                    compare_func(&self.entry(EntryId(*a)), &self.entry(EntryId(*b)))
                }),
                SortOrder::Descending => slice.par_sort_unstable_by(|a, b| {
                    compare_func(&self.entry(EntryId(*b)), &self.entry(EntryId(*a)))
                }),
            };

            hits
        };

        if query.sort_dirs_before_files() {
            hits.as_parallel_slice_mut().par_sort_by(|a, b| {
                self.nodes[*b as usize]
                    .is_dir
                    .cmp(&self.nodes[*a as usize].is_dir)
            });
        }

        Ok(hits.into_iter().map(EntryId).collect())
    }
}

struct FilterContext<'d, 'a, 'r> {
    database: &'d Database,
    abort_signal: &'a Arc<AtomicBool>,
    regex: &'r Regex,
}

trait Filter {
    /// Returns filtered ids without changing an order.
    fn ordered(ctx: &FilterContext, ids: impl ParallelIterator<Item = u32>) -> Result<Vec<u32>>;

    /// Returns filtered ids in an arbitrary order.
    fn unordered(ctx: &FilterContext) -> Result<Vec<u32>>;
}

struct AllFilter;

impl Filter for AllFilter {
    fn ordered(_: &FilterContext, ids: impl ParallelIterator<Item = u32>) -> Result<Vec<u32>> {
        Ok(ids.collect())
    }

    fn unordered(ctx: &FilterContext) -> Result<Vec<u32>> {
        let hits = (0..ctx.database.num_entries() as u32).collect();
        Ok(hits)
    }
}

struct BasenameFilter;

impl Filter for BasenameFilter {
    fn ordered(ctx: &FilterContext, ids: impl ParallelIterator<Item = u32>) -> Result<Vec<u32>> {
        // Since rust-lang/regex@e040c1b, regex library stopped using thread_local,
        // which had a performance impact on indexa.
        // We mitigate it by putting Regex in thread local storage.
        let regex_tls = ThreadLocal::new();

        ids.filter_map(|id| {
            if ctx.abort_signal.load(Ordering::Relaxed) {
                return Some(Err(Error::SearchAbort));
            }

            let regex = regex_tls.get_or(|| ctx.regex.clone());
            let node = &ctx.database.nodes[id as usize];
            regex
                .is_match(ctx.database.basename_from_node(node))
                .then(|| Ok(id))
        })
        .collect()
    }

    fn unordered(ctx: &FilterContext) -> Result<Vec<u32>> {
        let regex_tls = ThreadLocal::new();

        let nodes = &ctx.database.nodes;
        (0..nodes.len() as u32)
            .into_par_iter()
            .zip(nodes.par_iter())
            .filter_map(|(id, node)| {
                if ctx.abort_signal.load(Ordering::Relaxed) {
                    return Some(Err(Error::SearchAbort));
                }

                let regex = regex_tls.get_or(|| ctx.regex.clone());
                regex
                    .is_match(ctx.database.basename_from_node(node))
                    .then(|| Ok(id))
            })
            .collect()
    }
}

struct PathFilter;

impl Filter for PathFilter {
    fn ordered(ctx: &FilterContext, ids: impl ParallelIterator<Item = u32>) -> Result<Vec<u32>> {
        let matched = Self::match_path(ctx)?;

        ids.filter_map(|id| {
            if ctx.abort_signal.load(Ordering::Relaxed) {
                return Some(Err(Error::SearchAbort));
            }

            matched[id as usize].load(Ordering::Relaxed).then(|| Ok(id))
        })
        .collect()
    }

    fn unordered(ctx: &FilterContext) -> Result<Vec<u32>> {
        let matched = Self::match_path(ctx)?;

        (0..ctx.database.num_entries() as u32)
            .into_par_iter()
            .zip(matched.into_par_iter())
            .filter_map(|(id, hit)| {
                if ctx.abort_signal.load(Ordering::Relaxed) {
                    return Some(Err(Error::SearchAbort));
                }

                hit.load(Ordering::Relaxed).then(|| Ok(id))
            })
            .collect()
    }
}

impl PathFilter {
    fn match_path(ctx: &FilterContext) -> Result<Vec<AtomicBool>> {
        let nodes = &ctx.database.nodes;
        let mut matched = Vec::with_capacity(nodes.len());
        for _ in 0..nodes.len() {
            matched.push(AtomicBool::new(false));
        }

        let regex_tls = ThreadLocal::new();
        let root_paths = &ctx.database.root_paths;

        for ((root_id, root_path), next_root_id) in root_paths.iter().zip(
            root_paths
                .keys()
                .skip(1)
                .copied()
                .chain(std::iter::once(nodes.len() as u32)),
        ) {
            if ctx.regex.is_match(root_path.to_str().unwrap()) {
                matched[*root_id as usize..next_root_id as usize]
                    .into_par_iter()
                    .try_for_each(|hit| {
                        if ctx.abort_signal.load(Ordering::Relaxed) {
                            return Err(Error::SearchAbort);
                        }
                        hit.store(true, Ordering::Relaxed);
                        Ok(())
                    })?;
            } else {
                let root_node = &nodes[*root_id as usize];
                Self::match_path_impl(ctx, root_node, root_path, &regex_tls, &matched)?;
            }
        }

        Ok(matched)
    }

    fn match_path_impl(
        ctx: &FilterContext,
        node: &EntryNode,
        path: &Path,
        regex_tls: &ThreadLocal<Regex>,
        matched: &[AtomicBool],
    ) -> Result<()> {
        let regex = regex_tls.get_or(|| ctx.regex.clone());

        let children_range = node.child_start as usize..node.child_end as usize;
        (
            &ctx.database.nodes[children_range.clone()],
            &matched[children_range],
        )
            .into_par_iter()
            .try_for_each(|(node, hit)| {
                if ctx.abort_signal.load(Ordering::Relaxed) {
                    return Err(Error::SearchAbort);
                }

                let child_path = path.join(&ctx.database.basename_from_node(node));
                if let Some(s) = child_path.to_str() {
                    if regex.is_match(s) {
                        hit.store(true, Ordering::Relaxed);
                        if node.has_any_child() {
                            Self::match_all_descendants(ctx, node, matched)?;
                        }
                        return Ok(());
                    }

                    if node.has_any_child() {
                        Self::match_path_impl(ctx, node, &child_path, regex_tls, matched)?;
                        return Ok(());
                    }
                }

                Ok(())
            })
    }

    fn match_all_descendants(
        ctx: &FilterContext,
        node: &EntryNode,
        matched: &[AtomicBool],
    ) -> Result<()> {
        let children_range = node.child_start as usize..node.child_end as usize;
        (
            &ctx.database.nodes[children_range.clone()],
            &matched[children_range],
        )
            .into_par_iter()
            .try_for_each(|(node, hit)| {
                if ctx.abort_signal.load(Ordering::Relaxed) {
                    return Err(Error::SearchAbort);
                }

                hit.store(true, Ordering::Relaxed);
                if node.has_any_child() {
                    Self::match_all_descendants(ctx, node, matched)?;
                }

                Ok(())
            })
    }
}

struct RegexPathFilter;

impl Filter for RegexPathFilter {
    fn ordered(ctx: &FilterContext, ids: impl ParallelIterator<Item = u32>) -> Result<Vec<u32>> {
        let matched = Self::match_path(ctx)?;

        ids.filter_map(|id| {
            if ctx.abort_signal.load(Ordering::Relaxed) {
                return Some(Err(Error::SearchAbort));
            }

            matched[id as usize].load(Ordering::Relaxed).then(|| Ok(id))
        })
        .collect()
    }

    fn unordered(ctx: &FilterContext) -> Result<Vec<u32>> {
        let matched = Self::match_path(ctx)?;

        (0..ctx.database.num_entries() as u32)
            .into_par_iter()
            .zip(matched.into_par_iter())
            .filter_map(|(id, hit)| {
                if ctx.abort_signal.load(Ordering::Relaxed) {
                    return Some(Err(Error::SearchAbort));
                }

                hit.load(Ordering::Relaxed).then(|| Ok(id))
            })
            .collect()
    }
}

impl RegexPathFilter {
    fn match_path(ctx: &FilterContext) -> Result<Vec<AtomicBool>> {
        let nodes = &ctx.database.nodes;
        let regex_tls = ThreadLocal::new();

        let mut matched = Vec::with_capacity(nodes.len());
        for _ in 0..nodes.len() {
            matched.push(AtomicBool::new(false));
        }

        for (root_id, root_path) in &ctx.database.root_paths {
            if ctx.regex.is_match(root_path.to_str().unwrap()) {
                matched[*root_id as usize].store(true, Ordering::Relaxed);
            }

            let root_node = &nodes[*root_id as usize];
            Self::match_path_impl(ctx, root_node, root_path, &regex_tls, &matched)?;
        }

        Ok(matched)
    }

    fn match_path_impl(
        ctx: &FilterContext,
        node: &EntryNode,
        path: &Path,
        regex_tls: &ThreadLocal<Regex>,
        matched: &[AtomicBool],
    ) -> Result<()> {
        let regex = regex_tls.get_or(|| ctx.regex.clone());

        let children_range = node.child_start as usize..node.child_end as usize;
        (
            &ctx.database.nodes[children_range.clone()],
            &matched[children_range],
        )
            .into_par_iter()
            .try_for_each(|(node, hit)| {
                if ctx.abort_signal.load(Ordering::Relaxed) {
                    return Err(Error::SearchAbort);
                }

                let child_path = path.join(&ctx.database.basename_from_node(node));
                if let Some(s) = child_path.to_str() {
                    if regex.is_match(s) {
                        hit.store(true, Ordering::Relaxed);
                    }
                    if node.has_any_child() {
                        Self::match_path_impl(ctx, node, &child_path, regex_tls, matched)?;
                    }
                }

                Ok(())
            })
    }
}
