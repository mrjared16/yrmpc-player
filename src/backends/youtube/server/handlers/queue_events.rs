//! Event handlers for `PlayQueue` events (Layer 2 Bridge).

use crate::shared::play_queue::{QueueEvent, QueueId, RepeatMode};

pub struct QueueEventHandler {}

impl QueueEventHandler {
    #[must_use]
    pub fn new() -> Self {
        Self {}
    }

    pub fn handle(&mut self, event: QueueEvent) {
        match event {
            QueueEvent::ItemsAdded { ids } => self.handle_items_added(&ids),
            QueueEvent::ItemsRemoved { ids } => self.handle_items_removed(&ids),
            QueueEvent::OrderChanged { play_order, current_id } => {
                self.handle_order_changed(&play_order, current_id);
            }
            QueueEvent::CurrentChanged { from, to } => self.handle_current_changed(from, to),
            QueueEvent::ModesChanged { shuffle, repeat } => {
                self.handle_modes_changed(shuffle, repeat);
            }
            QueueEvent::Cleared => self.handle_cleared(),
            QueueEvent::Stopped => self.handle_stopped(),
        }
    }

    fn handle_items_added(&mut self, ids: &[QueueId]) {
        log::debug!("QueueEvent::ItemsAdded: {ids:?}");
    }

    fn handle_items_removed(&mut self, ids: &[QueueId]) {
        log::debug!("QueueEvent::ItemsRemoved: {ids:?}");
    }

    fn handle_order_changed(&mut self, play_order: &[QueueId], current_id: Option<QueueId>) {
        log::debug!("QueueEvent::OrderChanged: {} items, current={current_id:?}", play_order.len());
    }

    fn handle_current_changed(&mut self, from: Option<QueueId>, to: Option<QueueId>) {
        log::debug!("QueueEvent::CurrentChanged: {from:?} -> {to:?}");
    }

    fn handle_modes_changed(&mut self, shuffle: bool, repeat: RepeatMode) {
        log::debug!("QueueEvent::ModesChanged: shuffle={shuffle}, repeat={repeat:?}");
    }

    fn handle_cleared(&mut self) {
        log::debug!("QueueEvent::Cleared");
    }

    fn handle_stopped(&mut self) {
        log::debug!("QueueEvent::Stopped");
    }
}

impl Default for QueueEventHandler {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn handler_creation() {
        let _handler = QueueEventHandler::new();
    }

    #[test]
    fn handle_all_event_types() {
        let mut handler = QueueEventHandler::new();

        handler.handle(QueueEvent::ItemsAdded { ids: vec![1, 2, 3] });
        handler.handle(QueueEvent::ItemsRemoved { ids: vec![1] });
        handler.handle(QueueEvent::OrderChanged { play_order: vec![3, 2, 1], current_id: Some(3) });
        handler.handle(QueueEvent::CurrentChanged { from: Some(1), to: Some(2) });
        handler.handle(QueueEvent::ModesChanged { shuffle: true, repeat: RepeatMode::All });
        handler.handle(QueueEvent::Cleared);
        handler.handle(QueueEvent::Stopped);
    }
}
