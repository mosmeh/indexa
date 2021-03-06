use super::util;
use super::{Database, EntryId, EntryNode};
use crate::query::{Query, SortOrder};
use crate::{Error, Result};

use rayon::prelude::*;
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

impl Database {
    pub fn search(&self, query: &Query, aborted: Arc<AtomicBool>) -> Result<Vec<EntryId>> {
        if query.is_empty() {
            self.match_all(query)
        } else if query.match_path() {
            self.match_path(query, aborted)
        } else {
            self.match_basename(query, aborted)
        }
    }

    fn match_all(&self, query: &Query) -> Result<Vec<EntryId>> {
        self.collect_hits(query, |(id, _)| Some(Ok(EntryId(id))))
    }

    fn match_basename(&self, query: &Query, aborted: Arc<AtomicBool>) -> Result<Vec<EntryId>> {
        self.collect_hits(query, |(id, node)| {
            if aborted.load(Ordering::Relaxed) {
                return Some(Err(Error::SearchAbort));
            }

            if query.regex().is_match(&self.basename_from_node(node)) {
                Some(Ok(EntryId(id)))
            } else {
                None
            }
        })
    }

    fn match_path(&self, query: &Query, aborted: Arc<AtomicBool>) -> Result<Vec<EntryId>> {
        let mut hits = Vec::with_capacity(self.entries.len());
        for _ in 0..self.entries.len() {
            hits.push(AtomicBool::new(false));
        }

        if query.regex_enabled() {
            for (root_id, root_path) in &self.root_paths {
                let root_node = &self.entries[*root_id as usize];
                if query.regex().is_match(&root_path.to_str().unwrap()) {
                    hits[*root_id as usize].store(true, Ordering::Relaxed);
                }

                self.match_path_impl(root_node, &root_path, query, &hits, aborted.clone())?;
            }
        } else {
            for ((root_id, root_path), next_root_id) in self.root_paths.iter().zip(
                self.root_paths
                    .keys()
                    .skip(1)
                    .copied()
                    .chain(std::iter::once(self.entries.len() as u32)),
            ) {
                let root_node = &self.entries[*root_id as usize];
                if query.regex().is_match(&root_path.to_str().unwrap()) {
                    (*root_id..next_root_id)
                        .into_par_iter()
                        .try_for_each(|id| {
                            if aborted.load(Ordering::Relaxed) {
                                return Err(Error::SearchAbort);
                            }
                            hits[id as usize].store(true, Ordering::Relaxed);
                            Ok(())
                        })?;
                } else {
                    self.match_path_impl(root_node, &root_path, query, &hits, aborted.clone())?;
                }
            }
        }

        self.collect_hits(query, |(id, _)| {
            if aborted.load(Ordering::Relaxed) {
                return Some(Err(Error::SearchAbort));
            }

            if hits[id as usize].load(Ordering::Relaxed) {
                Some(Ok(EntryId(id)))
            } else {
                None
            }
        })
    }

    fn match_path_impl(
        &self,
        node: &EntryNode,
        path: &Path,
        query: &Query,
        hits: &[AtomicBool],
        aborted: Arc<AtomicBool>,
    ) -> Result<()> {
        (node.child_start..node.child_end)
            .into_par_iter()
            .try_for_each(|id| {
                if aborted.load(Ordering::Relaxed) {
                    return Err(Error::SearchAbort);
                }

                let child = &self.entries[id as usize];
                let child_path = path.join(&self.basename_from_node(child));
                if let Some(s) = child_path.to_str() {
                    if query.regex_enabled() {
                        if query.regex().is_match(s) {
                            hits[id as usize].store(true, Ordering::Relaxed);
                        }

                        if child.has_any_child() {
                            self.match_path_impl(child, &child_path, query, hits, aborted.clone())?;
                        }
                    } else if query.regex().is_match(s) {
                        hits[id as usize].store(true, Ordering::Relaxed);

                        if child.has_any_child() {
                            self.match_all_descendants(child, hits, aborted.clone())?;
                        }
                    } else if child.has_any_child() {
                        self.match_path_impl(child, &child_path, query, hits, aborted.clone())?;
                    }
                }

                Ok(())
            })
    }

    fn match_all_descendants(
        &self,
        node: &EntryNode,
        hits: &[AtomicBool],
        aborted: Arc<AtomicBool>,
    ) -> Result<()> {
        (node.child_start..node.child_end)
            .into_par_iter()
            .try_for_each(|id| {
                if aborted.load(Ordering::Relaxed) {
                    return Err(Error::SearchAbort);
                }

                hits[id as usize].store(true, Ordering::Relaxed);

                let child = &self.entries[id as usize];
                if child.has_any_child() {
                    self.match_all_descendants(child, hits, aborted.clone())?;
                }

                Ok(())
            })
    }

    fn collect_hits<F>(&self, query: &Query, func: F) -> Result<Vec<EntryId>>
    where
        F: Fn((u32, &EntryNode)) -> Option<Result<EntryId>> + Send + Sync,
    {
        let hits: Result<Vec<_>> = if self.is_fast_sortable(query.sort_by()) {
            let iter = self.sorted_ids[query.sort_by()]
                .as_ref()
                .unwrap()
                .par_iter()
                .map(|id| (*id, &self.entries[*id as usize]));
            match query.sort_order() {
                SortOrder::Ascending => iter.filter_map(func).collect(),
                SortOrder::Descending => iter.rev().filter_map(func).collect(),
            }
        } else {
            let mut v = (0..self.entries.len() as u32)
                .into_par_iter()
                .zip(self.entries.par_iter())
                .filter_map(func)
                .collect::<Result<Vec<_>>>()?;

            let compare_func = util::build_compare_func(query.sort_by());
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
                    self.entries[b.0 as usize]
                        .is_dir
                        .cmp(&self.entries[a.0 as usize].is_dir)
                });
                hits
            })
        } else {
            hits
        }
    }
}
