use super::{Filter, FilterContext};
use crate::Result;

use rayon::prelude::*;

pub struct PassthroughFilter;

impl Filter for PassthroughFilter {
    fn ordered(_: &FilterContext, ids: impl ParallelIterator<Item = u32>) -> Result<Vec<u32>> {
        Ok(ids.collect())
    }

    fn unordered(ctx: &FilterContext) -> Result<Vec<u32>> {
        let hits = (0..ctx.database.num_entries() as u32).collect();
        Ok(hits)
    }
}
