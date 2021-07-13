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
            self.match_all(query)
        } else if query.match_path() {
            self.match_path(query, abort_signal)
        } else {
            self.match_basename(query, abort_signal)
        }
    }

    fn match_all(&self, query: &Query) -> Result<Vec<EntryId>> {
        self.collect_hits(query, |(id, _)| Some(Ok(EntryId(id))))
    }

    fn match_basename(
        &self,
        query: &Query,
        abort_signal: &Arc<AtomicBool>,
    ) -> Result<Vec<EntryId>> {
        // Since rust-lang/regex@e040c1b, regex library stopped using thread_local,
        // which had a performance impact on indexa.
        // We mitigate it by putting Regex in thread local storage.
        let regex_tls = ThreadLocal::new();

        self.collect_hits(query, |(id, node)| {
            if abort_signal.load(Ordering::Relaxed) {
                return Some(Err(Error::SearchAbort));
            }

            let regex = regex_tls.get_or(|| query.regex().clone());
            regex
                .is_match(self.basename_from_node(node))
                .then(|| Ok(EntryId(id)))
        })
    }

    fn match_path(&self, query: &Query, abort_signal: &Arc<AtomicBool>) -> Result<Vec<EntryId>> {
        let mut hits = Vec::with_capacity(self.nodes.len());
        for _ in 0..self.nodes.len() {
            hits.push(AtomicBool::new(false));
        }

        let regex_tls = ThreadLocal::new();

        if query.is_regex_enabled() {
            for (root_id, root_path) in &self.root_paths {
                let root_node = &self.nodes[*root_id as usize];
                if query.regex().is_match(root_path.to_str().unwrap()) {
                    hits[*root_id as usize].store(true, Ordering::Relaxed);
                }

                self.match_path_impl(root_node, root_path, query, &regex_tls, &hits, abort_signal)?;
            }
        } else {
            for ((root_id, root_path), next_root_id) in self.root_paths.iter().zip(
                self.root_paths
                    .keys()
                    .skip(1)
                    .copied()
                    .chain(std::iter::once(self.nodes.len() as u32)),
            ) {
                let root_node = &self.nodes[*root_id as usize];
                if query.regex().is_match(root_path.to_str().unwrap()) {
                    (*root_id..next_root_id)
                        .into_par_iter()
                        .try_for_each(|id| {
                            if abort_signal.load(Ordering::Relaxed) {
                                return Err(Error::SearchAbort);
                            }
                            hits[id as usize].store(true, Ordering::Relaxed);
                            Ok(())
                        })?;
                } else {
                    self.match_path_impl(
                        root_node,
                        root_path,
                        query,
                        &regex_tls,
                        &hits,
                        abort_signal,
                    )?;
                }
            }
        }

        self.collect_hits(query, |(id, _)| {
            if abort_signal.load(Ordering::Relaxed) {
                return Some(Err(Error::SearchAbort));
            }

            hits[id as usize]
                .load(Ordering::Relaxed)
                .then(|| Ok(EntryId(id)))
        })
    }

    fn match_path_impl(
        &self,
        node: &EntryNode,
        path: &Path,
        query: &Query,
        regex_tls: &ThreadLocal<Regex>,
        hits: &[AtomicBool],
        abort_signal: &Arc<AtomicBool>,
    ) -> Result<()> {
        let regex = regex_tls.get_or(|| query.regex().clone());

        (node.child_start..node.child_end)
            .into_par_iter()
            .try_for_each(|id| {
                if abort_signal.load(Ordering::Relaxed) {
                    return Err(Error::SearchAbort);
                }

                let child = &self.nodes[id as usize];
                let child_path = path.join(&self.basename_from_node(child));
                if let Some(s) = child_path.to_str() {
                    if query.is_regex_enabled() {
                        if regex.is_match(s) {
                            hits[id as usize].store(true, Ordering::Relaxed);
                        }

                        if child.has_any_child() {
                            self.match_path_impl(
                                child,
                                &child_path,
                                query,
                                regex_tls,
                                hits,
                                abort_signal,
                            )?;
                        }
                    } else if regex.is_match(s) {
                        hits[id as usize].store(true, Ordering::Relaxed);

                        if child.has_any_child() {
                            self.match_all_descendants(child, hits, abort_signal)?;
                        }
                    } else if child.has_any_child() {
                        self.match_path_impl(
                            child,
                            &child_path,
                            query,
                            regex_tls,
                            hits,
                            abort_signal,
                        )?;
                    }
                }

                Ok(())
            })
    }

    fn match_all_descendants(
        &self,
        node: &EntryNode,
        hits: &[AtomicBool],
        abort_signal: &Arc<AtomicBool>,
    ) -> Result<()> {
        (node.child_start..node.child_end)
            .into_par_iter()
            .try_for_each(|id| {
                if abort_signal.load(Ordering::Relaxed) {
                    return Err(Error::SearchAbort);
                }

                hits[id as usize].store(true, Ordering::Relaxed);

                let child = &self.nodes[id as usize];
                if child.has_any_child() {
                    self.match_all_descendants(child, hits, abort_signal)?;
                }

                Ok(())
            })
    }

    fn collect_hits<F>(&self, query: &Query, func: F) -> Result<Vec<EntryId>>
    where
        F: Fn((u32, &EntryNode)) -> Option<Result<EntryId>> + Send + Sync,
    {
        let hits: Result<Vec<_>> = if let Some(sorted_ids) =
            self.sorted_ids[query.sort_by()].as_ref()
        {
            let iter = sorted_ids
                .par_iter()
                .map(|id| (*id, &self.nodes[*id as usize]));
            match query.sort_order() {
                SortOrder::Ascending => iter.filter_map(func).collect(),
                SortOrder::Descending => iter.rev().filter_map(func).collect(),
            }
        } else {
            let mut v = (0..self.nodes.len() as u32)
                .into_par_iter()
                .zip(self.nodes.par_iter())
                .filter_map(func)
                .collect::<Result<Vec<_>>>()?;

            let compare_func = util::get_compare_func(query.sort_by());
            match query.sort_order() {
                SortOrder::Ascending => v
                    .as_parallel_slice_mut()
                    .par_sort_unstable_by(|a, b| compare_func(&self.entry(*a), &self.entry(*b))),
                SortOrder::Descending => v
                    .as_parallel_slice_mut()
                    .par_sort_unstable_by(|a, b| compare_func(&self.entry(*b), &self.entry(*a))),
            };

            Ok(v)
        };

        if query.sort_dirs_before_files() {
            hits.map(|mut hits| {
                hits.as_parallel_slice_mut().par_sort_by(|a, b| {
                    self.nodes[b.0 as usize]
                        .is_dir
                        .cmp(&self.nodes[a.0 as usize].is_dir)
                });
                hits
            })
        } else {
            hits
        }
    }
}
