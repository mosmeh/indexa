mod filters;

use super::{util, Database, EntryId};
use crate::{
    query::{Query, SortOrder},
    Result,
};
use filters::{Filter, FilterContext};

use rayon::prelude::*;
use std::sync::{atomic::AtomicBool, Arc};

impl Database {
    pub fn search(&self, query: &Query, abort_signal: &Arc<AtomicBool>) -> Result<Vec<EntryId>> {
        if query.is_empty() {
            return self.filter_and_sort::<filters::PassthroughFilter>(query, abort_signal);
        }
        if !query.match_path() {
            return self.filter_and_sort::<filters::BasenameFilter>(query, abort_signal);
        }
        if query.is_regex_enabled() {
            return self.filter_and_sort::<filters::RegexPathFilter>(query, abort_signal);
        }
        if !query.has_path_separator() {
            return self.filter_and_sort::<filters::ComponentWisePathFilter>(query, abort_signal);
        }
        self.filter_and_sort::<filters::FullPathFilter>(query, abort_signal)
    }

    fn filter_and_sort<F: Filter>(
        &self,
        query: &Query,
        abort_signal: &Arc<AtomicBool>,
    ) -> Result<Vec<EntryId>> {
        let ctx = FilterContext::new(self, abort_signal, query.regex());

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
            let slice = hits.as_parallel_slice_mut();
            match query.sort_order() {
                SortOrder::Ascending => slice.par_sort_by(|a, b| {
                    Ord::cmp(
                        &self.nodes[*b as usize].is_dir,
                        &self.nodes[*a as usize].is_dir,
                    )
                }),
                SortOrder::Descending => slice.par_sort_by(|a, b| {
                    Ord::cmp(
                        &self.nodes[*a as usize].is_dir,
                        &self.nodes[*b as usize].is_dir,
                    )
                }),
            }
        }

        Ok(hits.into_iter().map(EntryId).collect())
    }
}
