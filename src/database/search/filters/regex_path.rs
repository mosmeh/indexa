use super::{FilterContext, MatchEntries};
use crate::{database::EntryNode, Error, Result};

use rayon::prelude::*;
use std::{
    path::Path,
    sync::atomic::{AtomicBool, Ordering},
};

pub struct RegexPathFilter;

impl MatchEntries for RegexPathFilter {
    fn match_entries(ctx: &FilterContext, matched: &mut [AtomicBool]) -> Result<()> {
        let nodes = &ctx.database.nodes;

        for (root_id, root_path) in &ctx.database.root_paths {
            if ctx
                .regex
                .is_match(root_path.to_str().ok_or(Error::NonUtf8Path)?)
            {
                *matched[*root_id as usize].get_mut() = true;
            }

            let root_node = &nodes[*root_id as usize];
            traverse_tree(ctx, matched, root_node, root_path)?;
        }

        Ok(())
    }
}

fn traverse_tree(
    ctx: &FilterContext,
    matched: &[AtomicBool],
    node: &EntryNode,
    path: &Path,
) -> Result<()> {
    let regex = ctx.thread_local_regex();

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

            let child_path = path.join(&ctx.database.basename_from_node(node));
            if let Some(s) = child_path.to_str() {
                if regex.is_match(s) {
                    m.store(true, Ordering::Relaxed);
                }
                if node.has_any_child() {
                    traverse_tree(ctx, matched, node, &child_path)?;
                }
            }

            Ok(())
        })
}
