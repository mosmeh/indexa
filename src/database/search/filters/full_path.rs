use super::{FilterContext, MatchEntries};
use crate::{database::EntryNode, Error, Result};

use rayon::prelude::*;
use std::{
    path::Path,
    sync::atomic::{AtomicBool, Ordering},
};

pub struct FullPathFilter;

impl MatchEntries for FullPathFilter {
    fn match_entries(ctx: &FilterContext, matched: &[AtomicBool]) -> Result<()> {
        let nodes = &ctx.database.nodes;
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
                    .try_for_each(|m| {
                        if ctx.abort_signal.load(Ordering::Relaxed) {
                            return Err(Error::SearchAbort);
                        }
                        m.store(true, Ordering::Relaxed);
                        Ok(())
                    })?;
            } else {
                let root_node = &nodes[*root_id as usize];
                traverse_tree(ctx, matched, root_node, root_path)?;
            }
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
                    if node.has_any_child() {
                        super::match_all_descendants(ctx, matched, node)?;
                    }
                    return Ok(());
                }

                if node.has_any_child() {
                    traverse_tree(ctx, matched, node, &child_path)?;
                    return Ok(());
                }
            }

            Ok(())
        })
}
