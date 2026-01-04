use std::{
    collections::VecDeque,
    io::{self, Write},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    thread::Builder,
    time::Duration,
};

use anyhow::Result;
use crossbeam::{
    channel::{Receiver, Sender, bounded},
    select,
};
use drop_guard::ClientDropGuard;

use crate::{
    backends::BackendDispatcher,
    config::Config,
    mpd::{commands::idle::IdleEvent, errors::MpdError},
    shared::{
        events::{AppEvent, ClientRequest, WorkDone},
        macros::{status_error, try_break, try_skip},
    },
};

pub(crate) fn init(
    client_rx: Receiver<ClientRequest>,
    event_tx: Sender<AppEvent>,
    client: BackendDispatcher<'static>,
    config: Arc<Config>,
) -> io::Result<std::thread::JoinHandle<()>> {
    std::thread::Builder::new()
        .name("client".to_owned())
        .spawn(move || client_task(&client_rx, &event_tx, client, &config))
}

static HEALTHY: AtomicBool = AtomicBool::new(true);

macro_rules! health {
    ($e:expr, $msg:literal) => {
        {
            if HEALTHY.load(Ordering::Relaxed) == false {
                log::error!("Client is not healhty. Trying to end threads and reconnect");
                break;
            }

            match $e {
                Ok(v) => v,
                Err(e) => {
                    log::error!(error:? = e; $msg);
                    HEALTHY.store(false, Ordering::Relaxed);
                    break;
                }
            }
        }
    };
}

fn should_skip_request(buffer: &VecDeque<ClientRequest>, request: &ClientRequest) -> bool {
    buffer.iter().any(|request2| {
        if let (ClientRequest::Query(q1), ClientRequest::Query(q2)) = (&request, &request2) {
            q1.should_be_skipped(q2)
        } else {
            false
        }
    })
}

#[allow(clippy::single_match_else)]
fn client_task(
    request_rx: &Receiver<ClientRequest>,
    event_tx: &Sender<AppEvent>,
    client: BackendDispatcher<'_>,
    config: &Config,
) {
    // TODO probably a good idea to drop the channels on each reconnect loop
    let mut first_loop = true;
    let (client_received_tx, client_received_rx) = &bounded::<()>(0);
    let (client_return_tx, client_return_rx) = &bounded::<BackendDispatcher<'_>>(1);

    std::thread::scope(|s| {
        client_return_tx.send(client).expect("Client init to succeed");

        loop {
            log::trace!(first_loop; "Starting worker threads");

            HEALTHY.store(true, Ordering::Relaxed);

            let _ = client_received_rx.try_iter().collect::<Vec<_>>();

            log::trace!(first_loop; "Trying to get returned client");
            let mut client = match client_return_rx.recv() {
                Ok(client) => client,
                Err(err) => {
                    log::error!(err:?; "Did not receive client from the return channel");
                    break;
                }
            };
            let is_client_ok =
                check_connection(first_loop, &mut client, request_rx, event_tx, config);
            first_loop = false;

            if is_client_ok {
                let mut client_write =
                    client.try_clone_stream().expect("Client write clone to succeed");

                let idle = Builder::new()
                    .name("idle".to_string())
                    .spawn_scoped(s, move || {
                        'outer: loop {
                            log::trace!("Waiting to acquire client");
                            let client = health!(client_return_rx.recv(), "Failed to receive client from request thread");
                            let mut client = ClientDropGuard::new(client_return_tx, client);
                            let timeout = config.mpd_idle_read_timeout_ms;
                            log::trace!(timeout:?; "Successfully acquired client, setting read timeout");

                            health!(client.set_read_timeout(config.mpd_idle_read_timeout_ms), "Failed to set read timeout for idle client");

                            log::trace!("Read timeout set, entering idle state");
                            health!(client.enter_idle(), "Failed to enter idle state");

                            log::trace!("Sending client received confirmation");
                            health!(client_received_tx.send_timeout((), Duration::from_secs(3)), "Failed to send client received confirmation");

                            log::trace!("Idle confirmation sent, waiting for events");
                            let events: Vec<IdleEvent> = loop {
                                match client.read_response() {
                                    Ok(events) => break events,
                                    Err(err) => {
                                        if let Some(MpdError::TimedOut(_)) = err.downcast_ref::<MpdError>() {
                                            log::trace!("Idle timeout, yielding to request thread");
                                            // IMPORTANT: Always break on timeout to yield the client.
                                            //
                                            // For MPD: `noidle` can interrupt read_response() via socket,
                                            //          but timeouts are rare (default: infinite wait).
                                            //
                                            // For YouTube: `write_noidle()` is a no-op, so this is the ONLY
                                            //              way to yield the client to the request thread.
                                            //
                                            // Breaking here ensures the request thread gets a chance to
                                            // process requests regardless of backend implementation.
                                            break vec![];
                                        }

                                        log::error!(error:? = err; "Encountered error while reading idle events");
                                        HEALTHY.store(false, Ordering::Relaxed);
                                        break 'outer;
                                    }
                                }
                            };

                            log::trace!(events:?; "Got idle events");
                            for ev in events {
                                if let Err(err) = event_tx.send(AppEvent::IdleEvent(ev)) {
                                    log::error!(err:?; "Failed to send idle event");
                                    break 'outer;
                                }
                            }

                            log::trace!("Stopping idle, dropping client");
                            drop(client);
                            log::trace!("Client dropped, waiting for confirmation");
                            health!(client_received_rx.recv_timeout(Duration::from_secs(3)), "Did not receive confirmation from worker thread");
                            log::trace!("Confirmation received");
                        }
                        log::trace!("idle loop ended");
                    })
                    .expect("failed to spawn thread");

                try_skip!(client_return_tx.send(client), "Failed to request for client idle");
                try_break!(client_received_rx.recv(), "Idle confirmation failed");

                let work = Builder::new()
                    .name("request".to_string())
                    .spawn_scoped(s, move || {
                        let mut buffer = VecDeque::new();

                        loop {
                            log::trace!("Waiting for client requests");
                            let msg = select! {
                                recv(request_rx) -> msg => {
                                    health!(msg, "Failed to receive client request")
                                }
                                recv(client_return_rx) -> client => {
                                    let client = match client {
                                        Ok(client) => ClientDropGuard::new(client_return_tx, client),
                                        Err(err) => {
                                            log::error!(err:?; "Failed to receive client from idle thread");
                                            HEALTHY.store(false, Ordering::Relaxed);
                                            break;
                                        }
                                    };

                                    if !HEALTHY.load(Ordering::Relaxed) {
                                        log::error!("Received client from idle thread while not healthy. Breaking the loop.");
                                        break;
                                    }

                                    log::trace!(client:?; "Received client from idle. No work to do. Sending it back.");
                                    health!(client_received_tx.send_timeout((), Duration::from_secs(3)), "Failed to send client received confirmation");
                                    drop(client);
                                    log::trace!("Waiting for confirmation from idle thread");
                                    health!(client_received_rx.recv_timeout(Duration::from_secs(3)), "Did not receive confirmation from idle thread");
                                    continue;
                                }
                            };
                            buffer.push_back(msg);

                            log::trace!(buffer:?; "Got requests. Trying to receive client from idle thread");
                            health!(client_write.write_noidle(), "Failed to write noidle command");
                            log::trace!("Sent noidle command (if applicable)");

                            let client = health!(client_return_rx.recv(), "Failed to receive client from idle thread");
                            let mut client = ClientDropGuard::new(client_return_tx, client);
                            log::trace!("Successfully received client from idle thread. Sending confirmation.");

                            health!(client_received_tx.send_timeout((), Duration::from_secs(3)), "Failed to send client received confirmation");

                            log::trace!(timeout:? = config.mpd_read_timeout; "Setting read timeout");
                            health!(client.set_read_timeout(Some(config.mpd_read_timeout)), "Failed to set read timeout");

                            while let Some(request) = buffer.pop_front() {
                                while let Ok(request) = request_rx.try_recv() {
                                    log::trace!(count = buffer.len(), buffer:?; "Got more requests");
                                    buffer.push_back(request);
                                }

                                if should_skip_request(&buffer, &request) {
                                    log::trace!(request:?; "Skipping duplicated request");
                                    continue;
                                }

                                match handle_client_request(&mut client, request) {
                                    Ok(result) => {
                                        health!(
                                            event_tx.send(AppEvent::WorkDone(Ok(result))),
                                            "Failed to send work done success event"
                                        );
                                    }
                                    Err(err) => match err.downcast_ref::<MpdError>() {
                                        Some(MpdError::TimedOut(err)) => {
                                            status_error!(err:?; "Reading response from MPD timed out, will try to reconnect");
                                            health!(client.reconnect(), "Failed to reconnect");
                                            health!(client.set_write_timeout(Some(config.mpd_write_timeout)), "Failed to set write timeout");
                                            client_write = health!(client.try_clone_stream(), "Client write clone to succeed");
                                        },
                                        _ => {
                                            log::error!(error:? = err; "Failed to handle client request");
                                            health!(
                                                event_tx.send(AppEvent::WorkDone(Err(err))),
                                                "Failed to send work done error event"
                                            );
                                        },
                                    },
                                }
                            }

                            log::trace!("All requests processed, returning client to idle thread");
                            drop(client);
                            log::trace!("Client returned to idle thread. Waiting for confirmation");
                            health!(client_received_rx.recv_timeout(Duration::from_secs(3)), "Did not receive confirmation from idle thread");
                        }

                        log::error!("Work loop ended. Shutting down MPD client.");
                        HEALTHY.store(false, Ordering::Relaxed);
                        try_skip!(client_write.shutdown_both(), "Failed to shutdown MPD client");
                    })
                    .expect("failed to spawn thread");

                idle.join().expect("idle thread not to panic");
                work.join().expect("work thread not to panic");
            } else {
                client_return_tx.send(client).expect("To be able to return the client");
            }

            let wait_time = std::time::Duration::from_secs(1);
            log::debug!(wait_time:?; "Lost connection to MPD, waiting before trying again");
            try_skip!(
                event_tx.send(AppEvent::LostConnection),
                "Failed to send lost connection event"
            );
            std::thread::sleep(wait_time);
        }
    });
}

mod drop_guard {
    use std::ops::DerefMut;

    use crossbeam::channel::Sender;

    use crate::backends::BackendDispatcher;

    #[derive(Debug)]
    pub struct ClientDropGuard<'sender, 'client> {
        tx: &'sender Sender<BackendDispatcher<'client>>,
        client: Option<BackendDispatcher<'client>>,
    }

    impl<'sender, 'client> ClientDropGuard<'sender, 'client> {
        pub fn new(
            tx: &'sender Sender<BackendDispatcher<'client>>,
            client: BackendDispatcher<'client>,
        ) -> Self {
            Self { tx, client: Some(client) }
        }
    }

    impl Drop for ClientDropGuard<'_, '_> {
        fn drop(&mut self) {
            if let Some(client) = self.client.take() {
                log::trace!("Sending back client on drop");
                if let Err(err) = self.tx.send(client) {
                    log::error!(error:? = err; "Failed to send client back on drop");
                }
            }
        }
    }

    impl<'client> std::ops::Deref for ClientDropGuard<'_, 'client> {
        type Target = BackendDispatcher<'client>;

        fn deref(&self) -> &Self::Target {
            self.client.as_ref().expect("Cannot deref because client was None")
        }
    }

    impl DerefMut for ClientDropGuard<'_, '_> {
        fn deref_mut(&mut self) -> &mut Self::Target {
            self.client.as_mut().expect("Cannot deref_mut because client was None")
        }
    }
}

fn check_connection(
    first_loop: bool,
    client: &mut BackendDispatcher<'_>,
    client_rx: &Receiver<ClientRequest>,
    event_tx: &Sender<AppEvent>,
    config: &Config,
) -> bool {
    if first_loop {
        true
    } else if client.reconnect().is_ok() {
        // empty the work queue after reconnect as they might no longer be
        // relevant
        let _ = client_rx.try_iter().collect::<Vec<_>>();
        try_skip!(event_tx.send(AppEvent::Reconnected), "Failed to send reconnected event");
        if let Err(err) = client.set_read_timeout(Some(config.mpd_read_timeout)) {
            log::error!(error:? = err; "Failed to set read timeout");
            return false;
        }
        if let Err(err) = client.set_write_timeout(Some(config.mpd_write_timeout)) {
            log::error!(error:? = err; "Failed to set write timeout");
            return false;
        }
        true
    } else {
        false
    }
}

fn handle_client_request(
    client: &mut BackendDispatcher<'_>,
    request: ClientRequest,
) -> Result<WorkDone> {
    match request {
        ClientRequest::Query(query) => Ok(WorkDone::QueryFinished {
            id: query.id,
            target: query.target,
            data: (query.callback)(client)?,
        }),
        ClientRequest::Command(command) => {
            (command.callback)(client)?;
            Ok(WorkDone::None)
        }
        ClientRequest::QuerySync(query) => {
            let result = (query.callback)(client)?;
            query.tx.send(result)?;
            Ok(WorkDone::None)
        }
    }
}
