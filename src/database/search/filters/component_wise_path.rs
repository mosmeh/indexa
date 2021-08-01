use super::{FilterContext, MatchEntries};
use crate::{database::EntryNode, Error, Result};

use rayon::prelude::*;
use std::sync::atomic::{AtomicBool, Ordering};

pub struct ComponentWisePathFilter;

impl MatchEntries for ComponentWisePathFilter {
    fn match_entries(ctx: &FilterContext, matched: &mut [AtomicBool]) -> Result<()> {
        let nodes = &ctx.database.nodes;
        let root_paths = &ctx.database.root_paths;

        for ((root_id, root_path), next_root_id) in root_paths.iter().zip(
            root_paths
                .keys()
                .skip(1)
                .copied()
                .chain(std::iter::once(nodes.len() as u32)),
        ) {
            if ctx
                .regex
                .is_match(root_path.to_str().ok_or(Error::NonUtf8Path)?)
            {
                for m in &mut matched[*root_id as usize..next_root_id as usize] {
                    *m.get_mut() = true;
                }
            } else {
                let root_node = &nodes[*root_id as usize];
                traverse_tree(ctx, matched, root_node)?;
            }
        }

        Ok(())
    }
}

fn traverse_tree(ctx: &FilterContext, matched: &[AtomicBool], node: &EntryNode) -> Result<()> {
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

            if regex.is_match(ctx.database.basename_from_node(node)) {
                m.store(true, Ordering::Relaxed);
                if node.has_any_child() {
                    super::match_all_descendants(ctx, matched, node)?;
                }
                return Ok(());
            }

            if node.has_any_child() {
                traverse_tree(ctx, matched, node)?;
                return Ok(());
            }

            Ok(())
        })
}
