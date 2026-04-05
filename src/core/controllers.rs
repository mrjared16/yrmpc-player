use std::sync::Arc;

use crossbeam::channel::Sender;

use super::queue_state::QueueDaemon;
use crate::{
    AppEvent, Query, QueryResult,
    backends::youtube::protocol::{
        ServerCommand,
        play_intent::{PlayIntent, RequestId},
    },
    domain::Song,
    shared::events::ClientRequest,
};

pub struct CtxQueueDaemon {
    client_request_sender: Sender<ClientRequest>,
}

impl CtxQueueDaemon {
    pub(crate) fn new(client_request_sender: Sender<ClientRequest>) -> Self {
        Self { client_request_sender }
    }
}

impl QueueDaemon for CtxQueueDaemon {
    fn add(&self, songs: Vec<Song>) {
        let cmd = crate::PlayerCommand {
            callback: Box::new(move |client| {
                for song in songs {
                    client.add_song(&song, None)?;
                }
                Ok(())
            }),
        };
        let _ = self.client_request_sender.send(ClientRequest::Command(cmd));
    }

    fn add_and_play(&self, songs: Vec<Song>) {
        let cmd = crate::PlayerCommand {
            callback: Box::new(move |client| {
                let status = client.get_status()?;
                let start_idx = status.playlistlength as usize;
                for song in songs {
                    client.add_song(&song, None)?;
                }
                client.play_pos(start_idx)?;
                Ok(())
            }),
        };
        let _ = self.client_request_sender.send(ClientRequest::Command(cmd));
    }

    fn remove_ids(&self, ids: Vec<u32>) {
        let cmd = crate::PlayerCommand {
            callback: Box::new(move |client| {
                for id in ids {
                    client.delete_id(id)?;
                }
                Ok(())
            }),
        };
        let _ = self.client_request_sender.send(ClientRequest::Command(cmd));
    }

    fn move_id(&self, id: u32, to_position: u32) {
        let cmd = crate::PlayerCommand {
            callback: Box::new(move |client| {
                client.move_id(id, to_position)?;
                Ok(())
            }),
        };
        let _ = self.client_request_sender.send(ClientRequest::Command(cmd));
    }

    fn clear(&self) {
        let cmd = crate::PlayerCommand {
            callback: Box::new(move |client| {
                client.clear()?;
                Ok(())
            }),
        };
        let _ = self.client_request_sender.send(ClientRequest::Command(cmd));
    }

    fn refresh(&self) {
        let query = Query {
            id: "queue_refresh",
            target: None,
            replace_id: Some("queue_refresh"),
            callback: Box::new(|client| {
                let queue = client.playlist_info()?;
                Ok(QueryResult::Queue(Some(queue)))
            }),
        };
        let _ = self.client_request_sender.send(ClientRequest::Query(query));
    }

    fn play_with_intent(&self, intent: PlayIntent, request_id: RequestId) {
        let cmd = crate::PlayerCommand {
            callback: Box::new(move |client| {
                if let crate::backends::BackendDispatcher::YouTube(yt) = client {
                    yt.request_ok(ServerCommand::PlayWithIntent { intent, request_id })?;
                }
                Ok(())
            }),
        };
        let _ = self.client_request_sender.send(ClientRequest::Command(cmd));
    }
}

pub struct Controllers {
    pub queue_state: super::queue_state::QueueState,
}

impl std::fmt::Debug for Controllers {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Controllers").field("queue_state", &"QueueState { ... }").finish()
    }
}

impl Controllers {
    pub(crate) fn new(
        initial_queue: Vec<Song>,
        app_event_tx: Sender<AppEvent>,
        client_request_sender: Sender<ClientRequest>,
    ) -> Self {
        let daemon = Arc::new(CtxQueueDaemon::new(client_request_sender));
        let queue_state = super::queue_state::QueueState::new(initial_queue, app_event_tx, daemon);
        Self { queue_state }
    }
}
