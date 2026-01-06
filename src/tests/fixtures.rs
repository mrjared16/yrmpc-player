use std::{
    cell::{Cell, RefCell},
    collections::{HashMap, HashSet},
    sync::{Arc, RwLock},
};

use crossbeam::channel::{Receiver, Sender, bounded, unbounded};
use rstest::fixture;

use crate::{
    app_state::AppState,
    config::Config,
    ctx::Ctx,
    domain::Status,
    mpd::version::Version,
    shared::{
        events::{ClientRequest, WorkRequest},
        image_cache::ImageCache,
        ring_vec::RingVec,
    },
};

#[fixture]
pub fn work_request_channel() -> (Sender<WorkRequest>, Receiver<WorkRequest>) {
    unbounded()
}

#[fixture]
pub fn client_request_channel() -> (Sender<ClientRequest>, Receiver<ClientRequest>) {
    unbounded()
}

/// Creates a Ctx with its own internal channels (receivers are leaked to keep
/// channels open)
#[fixture]
pub fn ctx() -> Ctx {
    let (tx, rx) = unbounded();
    let (work_tx, work_rx) = unbounded();
    let (client_tx, client_rx) = unbounded();

    // Keep receivers alive by leaking them
    std::mem::forget(rx);
    std::mem::forget(work_rx);
    std::mem::forget(client_rx);

    create_ctx_inner(tx, work_tx, client_tx)
}

/// Creates a Ctx using provided channels (for tests that need to intercept
/// messages)
#[fixture]
pub fn ctx_with_channels(
    work_request_channel: (Sender<WorkRequest>, Receiver<WorkRequest>),
    client_request_channel: (Sender<ClientRequest>, Receiver<ClientRequest>),
) -> Ctx {
    let (tx, rx) = unbounded();
    std::mem::forget(rx);

    let (work_tx, _work_rx) = work_request_channel;
    let (client_tx, _client_rx) = client_request_channel;

    create_ctx_inner(tx, work_tx, client_tx)
}

fn create_ctx_inner(
    tx: Sender<crate::shared::events::AppEvent>,
    work_tx: Sender<WorkRequest>,
    client_tx: Sender<ClientRequest>,
) -> Ctx {
    Ctx {
        backend_version: Version::new(0, 0, 0),
        config: Arc::new(Config::default()),
        status: Status::default(),
        image_cache: ImageCache::new(tx.clone()),
        app_state: Arc::new(RwLock::new(AppState::default())),
        controllers: crate::core::controllers::Controllers::new(
            vec![],
            tx.clone(),
            client_tx.clone(),
        ),
        stickers: HashMap::new(),
        active_tab: "Queue".into(),
        supported_commands: HashSet::new(),
        capabilities: &[], // Empty for tests - no backend capabilities
        db_update_start: None,
        app_event_sender: tx.clone(),
        work_sender: work_tx,
        client_request_sender: client_tx.clone(),
        needs_render: Cell::new(false),
        stickers_to_fetch: RefCell::new(HashSet::new()),
        lrc_index: Default::default(),
        rendered_frames: 0,
        messages: RingVec::default(),
        last_status_update: std::time::Instant::now(),
        song_played: None,
        stickers_supported: crate::ctx::StickersSupport::Unsupported,
        scheduler: crate::core::scheduler::Scheduler::new((tx.clone(), client_tx.clone())),
        debug_ui_log: None,
        queue_panel_visible: false,
        previous_tab: None,
    }
}

#[fixture]
pub fn config() -> Config {
    Config::default()
}

pub mod mpd_client {
    use std::io::BufRead;

    use rstest::fixture;

    use crate::mpd::{
        errors::MpdError,
        proto_client::{MpdLine, SocketClient},
        version::Version,
    };

    pub struct TestMpdClient {
        content: Option<Box<dyn BufRead>>,
    }

    impl TestMpdClient {
        pub fn new(buf: &[u8]) -> Self {
            Self { content: Some(Box::new(std::io::Cursor::new(buf.to_vec()))) }
        }

        pub fn set_read_content(&mut self, r: Box<dyn BufRead>) {
            self.content = Some(r);
        }

        pub fn set_read(&mut self, r: impl BufRead + 'static) {
            self.content = Some(Box::new(r));
        }
    }

    impl SocketClient for TestMpdClient {
        fn write(&mut self, _bytes: &[u8]) -> std::io::Result<()> {
            Ok(())
        }

        fn read(&mut self) -> &mut impl BufRead {
            self.content.as_mut().unwrap()
        }

        fn version(&self) -> Version {
            Version::new(0, 0, 0)
        }

        fn clear_read_buf(&mut self) -> anyhow::Result<()> {
            Ok(())
        }
    }

    #[fixture]
    pub fn client() -> TestMpdClient {
        TestMpdClient { content: Some(Box::new(std::io::Cursor::new(vec![]))) }
    }
}

#[fixture]
pub fn mock_backend() -> crate::backends::mock::MockBackend {
    crate::backends::mock::MockBackend::new()
}

#[fixture]
pub fn terminal() -> ratatui::Terminal<ratatui::backend::TestBackend> {
    ratatui::Terminal::new(ratatui::backend::TestBackend::new(10, 10)).unwrap()
}
