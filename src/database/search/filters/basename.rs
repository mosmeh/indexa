use super::{Filter, FilterContext};
use crate::{Error, Result};

use rayon::prelude::*;
use std::sync::atomic::Ordering;

pub enum BasenameFilter {}

impl Filter for BasenameFilter {
    fn ordered(ctx: &FilterContext, ids: impl ParallelIterator<Item = u32>) -> Result<Vec<u32>> {
        ids.filter_map(|id| {
            if ctx.abort_signal.load(Ordering::Relaxed) {
                return Some(Err(Error::SearchAbort));
            }

            let node = &ctx.database.nodes[id as usize];
            ctx.thread_local_regex()
                .is_match(ctx.database.basename_from_node(node))
                .then(|| Ok(id))
        })
        .collect()
    }

    fn unordered(ctx: &FilterContext) -> Result<Vec<u32>> {
        let nodes = &ctx.database.nodes;
        (0..nodes.len() as u32)
            .into_par_iter()
            .zip(nodes.par_iter())
            .filter_map(|(id, node)| {
                if ctx.abort_signal.load(Ordering::Relaxed) {
                    return Some(Err(Error::SearchAbort));
                }

                ctx.thread_local_regex()
                    .is_match(ctx.database.basename_from_node(node))
                    .then(|| Ok(id))
            })
            .collect()
    }
}
