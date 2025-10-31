#![allow(dead_code)] // because its are test helpers

use std::{
    path::{Path, PathBuf},
    sync::mpsc::{self, TryRecvError},
    time::Duration,
};

use notify_types::event::{Event, EventKind};

use crate::{Config, Error, PollWatcher, RecommendedWatcher, RecursiveMode, Watcher, WatcherKind};
use pretty_assertions::assert_eq;

pub struct Receiver {
    pub rx: mpsc::Receiver<Result<Event, Error>>,
    pub timeout: Duration,
    pub detect_changes: Option<Box<dyn Fn()>>,
    pub kind: WatcherKind,
}

impl Receiver {
    const DEFAULT_TIMEOUT: Duration = Duration::from_secs(1);

    /// Waits for events in the same order as they are provided,
    /// and fails if it encounters an unexpected one.
    ///
    /// Before any actions, it'll call [`Self::detect_changes`].
    /// It simplify any tests with [`PollWatcher`]
    pub fn wait_exact(&mut self, expected: impl IntoIterator<Item = ExpectedEvent>) {
        self.detect_changes();
        for (idx, expected) in expected.into_iter().enumerate() {
            match self.try_recv() {
                Ok(result) => match result {
                    Ok(event) => assert_eq!(
                        ExpectedEvent::from_event(event),
                        expected,
                        "Unexpected event by index {idx}"
                    ),
                    Err(err) => panic!("Expected an event by index {idx} but got {err:?}"),
                },
                Err(err) => panic!("Unable to check the event by index {idx}: {err:?}"),
            }
        }
        match self.rx.try_recv() {
            Ok(res) => panic!("Unexpected extra event: {res:?}"),
            Err(err) => assert!(
                matches!(err, TryRecvError::Empty),
                "Unexpected error: expected Empty, actual: {err:?}"
            ),
        }
    }

    /// Waits for the provided events in any order and ignores unexpected ones.
    ///
    /// Before any actions, it'll call [`Self::detect_changes`].
    /// It simplify any tests with [`PollWatcher`]
    pub fn wait(&mut self, iter: impl IntoIterator<Item = ExpectedEvent>) {
        self.detect_changes();
        let mut expected = iter.into_iter().enumerate().collect::<Vec<_>>();
        let mut received = Vec::new();

        while !expected.is_empty() {
            match self.try_recv() {
                Ok(result) => match result {
                    Ok(event) => {
                        received.push(event.clone());
                        let mut found_idx = None;
                        let actual = ExpectedEvent::from_event(event);
                        for (idx, (_, expected)) in expected.iter().enumerate() {
                            if &actual == expected {
                                found_idx = Some(idx);
                                break;
                            }
                        }
                        if let Some(found_idx) = found_idx {
                            expected.swap_remove(found_idx);
                        }
                    }
                    Err(err) => panic!("Got an error from the watcher {:?}: {err:?}. Received: {received:#?}. Weren't received: {expected:#?}", self.kind),
                },
                Err(err) => panic!("Watcher {:?} recv error: {err:?}. Received: {received:#?}. Weren't received: {expected:#?}", self.kind),
            }
        }
    }

    /// Waits a specific event.
    ///
    /// Panics, if got an error
    pub fn wait_event(&mut self, filter: impl Fn(&Event) -> bool) -> Event {
        self.detect_changes();
        let mut received = Vec::new();
        loop {
            match self.try_recv() {
                Ok(res) => match res {
                    Ok(event) => {
                        if filter(&event) {
                            return event;
                        }
                        received.push(event);
                    },
                    Err(err) => panic!("Got an error from the watcher {:?} before the expected event has been received: {err:?}. Received: {received:#?}", self.kind),
                },
                Err(err) => panic!("Watcher {:?} recv error but expected event weren't received: {err:?}. Received: {received:#?}", self.kind),
            }
        }
    }

    pub fn try_recv(&mut self) -> Result<Result<Event, Error>, mpsc::RecvTimeoutError> {
        self.rx.recv_timeout(self.timeout)
    }

    pub fn recv(&mut self) -> Event {
        self.recv_result()
            .unwrap_or_else(|e| panic!("Unexpected error from the watcher {:?}: {e:?}", self.kind))
    }

    pub fn recv_result(&mut self) -> Result<Event, Error> {
        self.try_recv().unwrap_or_else(|e| match e {
            mpsc::RecvTimeoutError::Timeout => panic!("Unable to wait the next event from the watcher {:?}: timeout", self.kind),
            mpsc::RecvTimeoutError::Disconnected => {
                panic!("Unable to wait the next event: the watcher {:?} part of the channel was disconnected", self.kind)
            }
        })
    }

    pub fn detect_changes(&self) {
        if let Some(detect_changes) = self.detect_changes.as_deref() {
            detect_changes()
        }
    }

    /// Returns an iterator iterating by events
    ///
    /// It doesn't fail on timeout, instead it returns None
    ///
    /// This behaviour is better for tests, because allows us to determine which events was received
    pub fn iter(&mut self) -> impl Iterator<Item = Event> + '_ {
        struct Iter<'a> {
            rx: &'a mut Receiver,
        }

        impl Iterator for Iter<'_> {
            type Item = Event;

            fn next(&mut self) -> Option<Self::Item> {
                self.rx
                    .try_recv()
                    .ok()
                    .map(|res| res.unwrap_or_else(|err| panic!("Got an error: {err:#?}")))
            }
        }

        Iter { rx: self }
    }

    /// Ensures, that the receiver part is empty. It doesn't wait anything, just check the channel
    pub fn ensure_empty(&mut self) {
        if let Ok(event) = self.rx.try_recv() {
            panic!("Unexpected event was received: {event:#?}")
        }
    }
}

#[derive(Debug)]
pub struct ChannelConfig {
    timeout: Duration,
    watcher_config: Config,
}

impl ChannelConfig {
    pub fn with_watcher_config(mut self, watcher_config: Config) -> Self {
        self.watcher_config = watcher_config;
        self
    }

    pub fn with_timeout(mut self, timeout: Duration) -> Self {
        self.timeout = timeout;
        self
    }
}

impl Default for ChannelConfig {
    fn default() -> Self {
        Self {
            timeout: Receiver::DEFAULT_TIMEOUT,
            watcher_config: Default::default(),
        }
    }
}

pub struct TestWatcher<W> {
    pub watcher: W,
    pub kind: WatcherKind,
}

impl<W: Watcher> TestWatcher<W> {
    pub fn watch_recursively(&mut self, path: impl AsRef<Path>) {
        self.watch(path, RecursiveMode::Recursive);
    }

    pub fn watch_nonrecursively(&mut self, path: impl AsRef<Path>) {
        self.watch(path, RecursiveMode::NonRecursive);
    }

    pub fn watch(&mut self, path: impl AsRef<Path>, recursive_mode: RecursiveMode) {
        let path = path.as_ref();
        self.watcher
            .watch(path, recursive_mode)
            .unwrap_or_else(|e| panic!("Unable to watch {:?}: {e:#?}", path))
    }
}

pub fn channel_with_config<W: Watcher>(config: ChannelConfig) -> (TestWatcher<W>, Receiver) {
    let (tx, rx) = mpsc::channel();
    let watcher = W::new(tx, config.watcher_config).expect("Unable to create a watcher");
    (
        TestWatcher {
            watcher,
            kind: W::kind(),
        },
        Receiver {
            rx,
            timeout: config.timeout,
            detect_changes: None,
            kind: W::kind(),
        },
    )
}

pub fn channel<W: Watcher>() -> (TestWatcher<W>, Receiver) {
    channel_with_config(Default::default())
}

pub fn recommended_channel() -> (TestWatcher<RecommendedWatcher>, Receiver) {
    channel()
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExpectedEvent {
    kind: EventKind,
    paths: Vec<PathBuf>,
}

impl ExpectedEvent {
    pub fn new(kind: EventKind, paths: Vec<PathBuf>) -> Self {
        Self { kind, paths }
    }

    pub fn with_path(kind: EventKind, path: impl AsRef<Path>) -> Self {
        Self::new(kind, vec![path.as_ref().to_path_buf()])
    }

    pub fn from_event(e: Event) -> Self {
        Self {
            kind: e.kind,
            paths: e.paths,
        }
    }
}

pub fn event(kind: EventKind, path: impl AsRef<Path>) -> ExpectedEvent {
    ExpectedEvent::with_path(kind, path)
}

pub fn testdir() -> tempfile::TempDir {
    tempfile::tempdir().expect("Unable to create tempdir")
}

/// Creates a [`PollWatcher`] with comparable content and manual polling.
///
/// Returned [`Receiver`] will send messasge to poll changes before wait-methods
pub fn poll_watcher() -> (TestWatcher<PollWatcher>, Receiver) {
    let (tx, rx) = mpsc::channel();
    let watcher = PollWatcher::new(
        tx,
        Config::default()
            .with_compare_contents(true)
            .with_manual_polling(),
    )
    .expect("Unable to create PollWatcher");
    let sender = watcher.poll_sender();
    let watcher = TestWatcher {
        watcher,
        kind: PollWatcher::kind(),
    };
    let rx = Receiver {
        rx,
        timeout: Receiver::DEFAULT_TIMEOUT,
        detect_changes: Some(Box::new(move || {
            sender
                .send(())
                .expect("PollWatcher receiver part was disconnected")
        })),
        kind: watcher.kind,
    };

    (watcher, rx)
}
