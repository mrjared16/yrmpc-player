use anyhow::Result;

use super::navigator_types::MoveDirection;
use crate::{
    actions::{ActionDispatcher, HandleResult, Intent},
    ctx::Ctx,
};

pub struct PaneActionExecutor<'a> {
    action_dispatcher: &'a ActionDispatcher,
}

impl<'a> PaneActionExecutor<'a> {
    pub fn new(action_dispatcher: &'a ActionDispatcher) -> Self {
        Self { action_dispatcher }
    }

    pub fn execute_queue_delete(&self, ctx: &mut Ctx, ids: Vec<u32>) -> Result<()> {
        if ids.is_empty() {
            return Ok(());
        }

        log::info!("PaneActionExecutor: Deleting {} items from queue", ids.len());
        ctx.queue_store().remove_ids(&ids);
        ctx.render()?;
        Ok(())
    }

    pub fn execute_queue_move(
        &self,
        ctx: &mut Ctx,
        ids: Vec<u32>,
        direction: MoveDirection,
    ) -> Result<()> {
        if ids.is_empty() {
            return Ok(());
        }

        log::info!("PaneActionExecutor: Moving {} items {:?}", ids.len(), direction);

        let queue = ctx.queue_store().read();
        let queue_len = queue.len();

        let mut positions: Vec<usize> = ids
            .iter()
            .filter_map(|&id| queue.iter().position(|song| song.id == Some(id)))
            .collect();

        if positions.is_empty() {
            log::warn!("PaneActionExecutor: No valid positions found for move");
            return Ok(());
        }

        positions.sort();

        let first_pos = positions[0];
        let last_pos = positions[positions.len() - 1];

        if direction == MoveDirection::Up && first_pos == 0 {
            log::debug!("PaneActionExecutor: Already at top, cannot move up");
            return Ok(());
        }
        if direction == MoveDirection::Down && last_pos >= queue_len.saturating_sub(1) {
            log::debug!("PaneActionExecutor: Already at bottom, cannot move down");
            return Ok(());
        }

        let (move_id, target_pos) = if direction == MoveDirection::Up {
            let above_pos = first_pos - 1;
            let above_id = queue.get(above_pos).and_then(|s| s.id);
            if let Some(id) = above_id {
                (id, last_pos as u32)
            } else {
                return Ok(());
            }
        } else {
            let below_pos = last_pos + 1;
            let below_id = queue.get(below_pos).and_then(|s| s.id);
            if let Some(id) = below_id {
                (id, first_pos as u32)
            } else {
                return Ok(());
            }
        };

        drop(queue);

        log::debug!(
            "PaneActionExecutor: Block move - moving id {} to position {}",
            move_id,
            target_pos
        );

        ctx.queue_store().move_id(move_id, target_pos as usize);
        ctx.render()?;
        Ok(())
    }

    pub fn execute_intent(&self, ctx: &mut Ctx, intent: Intent) -> Result<()> {
        log::info!("PaneActionExecutor: Executing intent {:?}", intent.action);

        match self.action_dispatcher.dispatch(&intent, ctx)? {
            HandleResult::Done => {
                log::debug!("PaneActionExecutor: Intent executed successfully");
            }
            HandleResult::NotApplicable(reason) => {
                log::warn!("PaneActionExecutor: Intent not applicable: {}", reason);
            }
            HandleResult::Skip => {
                log::debug!("PaneActionExecutor: No strategy handled the intent");
            }
        }

        ctx.render()?;
        Ok(())
    }
}
