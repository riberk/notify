#![allow(dead_code)] // because its are test helpers

use std::{
    collections::VecDeque,
    fmt::Debug,
    ops::Deref,
    path::{Path, PathBuf},
    sync::mpsc::{self},
    time::Duration,
};

use notify_types::event::Event;

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

    fn wait_expected<C: ExpectedCollection>(&mut self, mut state: ExpectedState<C>) {
        self.detect_changes();
        while !state.is_empty() {
            match self.try_recv() {
                Ok(res) => match res {
                    Ok(event) => state.check(event),
                    Err(err) => panic!(
                        "Got an error from the watcher {:?}: {err:?}. State: {state:#?}",
                        self.kind
                    ),
                },
                Err(e) => panic!(
                    "Recv error: {e:?}. Watcher: {:?}. State: {state:#?}",
                    self.kind
                ),
            }
        }
    }

    /// Waits for the events in the same order as they provided and fails on an unexpected one.
    pub fn wait_ordered_exact(&mut self, expected: impl IntoIterator<Item = ExpectedEvent>) {
        self.wait_expected(ExpectedState::ordered(expected).disallow_unexpected())
    }

    /// Waits for the events in the same order as they provided and ignores unexpected ones.
    pub fn wait_ordered(&mut self, expected: impl IntoIterator<Item = ExpectedEvent>) {
        self.wait_expected(ExpectedState::ordered(expected).allow_unexpected())
    }

    /// Waits for the events in any order and fails on an unexpected one.
    pub fn wait_unordered_exact(&mut self, expected: impl IntoIterator<Item = ExpectedEvent>) {
        self.wait_expected(ExpectedState::unordered(expected).disallow_unexpected())
    }

    /// Waits for the events in any order and ignores unexpected ones.
    pub fn wait_unordered(&mut self, expected: impl IntoIterator<Item = ExpectedEvent>) {
        self.wait_expected(ExpectedState::unordered(expected).allow_unexpected())
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum UnexpectedEventBehaviour {
    Ignore,
    Panic,
}

#[derive(Debug)]
struct ExpectedState<C> {
    remain: C,
    received: Vec<Event>,
    unexpeted_event_behaviour: UnexpectedEventBehaviour,
}

impl ExpectedState<Ordered> {
    pub fn ordered(iter: impl IntoIterator<Item = ExpectedEvent>) -> Self {
        Self::new(iter)
    }
}

impl ExpectedState<Unordered> {
    pub fn unordered(iter: impl IntoIterator<Item = ExpectedEvent>) -> Self {
        Self::new(iter)
    }
}

impl<C: ExpectedCollection + Debug> ExpectedState<C> {
    pub fn new(iter: impl IntoIterator<Item = ExpectedEvent>) -> Self {
        Self {
            remain: iter.into_iter().collect(),
            received: Default::default(),
            unexpeted_event_behaviour: UnexpectedEventBehaviour::Ignore,
        }
    }

    pub fn allow_unexpected(mut self) -> Self {
        self.unexpeted_event_behaviour = UnexpectedEventBehaviour::Ignore;
        self
    }

    pub fn disallow_unexpected(mut self) -> Self {
        self.unexpeted_event_behaviour = UnexpectedEventBehaviour::Panic;
        self
    }

    fn is_empty(&self) -> bool {
        self.remain.is_empty()
    }

    fn check(&mut self, event: Event) {
        let is_expected = self.remain.is_expected_event(&event);
        self.received.push(event);
        if !is_expected && self.unexpeted_event_behaviour == UnexpectedEventBehaviour::Panic {
            panic!("Unexpected event. State: {:#?}", self)
        }
    }
}

trait ExpectedCollection: Debug + FromIterator<ExpectedEvent> {
    fn is_empty(&self) -> bool;

    /// Returns true if the event is expected by this collection
    fn is_expected_event(&mut self, event: &Event) -> bool;
}

/// Stores original indexes for events to debug purposes
#[derive(Debug)]
struct Unordered(Vec<(usize, ExpectedEvent)>);

#[derive(Debug)]
struct Ordered(VecDeque<ExpectedEvent>);

impl ExpectedCollection for Unordered {
    fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    fn is_expected_event(&mut self, event: &Event) -> bool {
        let found_idx = self
            .0
            .iter()
            .enumerate()
            .find(|(_, (_, expected))| expected == event)
            .map(|(idx, _)| idx);
        match found_idx {
            Some(found_idx) => {
                self.0.swap_remove(found_idx);
                true
            }
            None => false,
        }
    }
}

impl ExpectedCollection for Ordered {
    fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    fn is_expected_event(&mut self, event: &Event) -> bool {
        if self.0.front().is_some_and(|expected| expected == event) {
            self.0.pop_front();
            true
        } else {
            false
        }
    }
}

impl FromIterator<ExpectedEvent> for Unordered {
    fn from_iter<T: IntoIterator<Item = ExpectedEvent>>(iter: T) -> Self {
        Self(iter.into_iter().enumerate().collect())
    }
}

impl FromIterator<ExpectedEvent> for Ordered {
    fn from_iter<T: IntoIterator<Item = ExpectedEvent>>(iter: T) -> Self {
        Self(iter.into_iter().collect())
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

pub use expect::*;

mod expect {
    use std::path::{Path, PathBuf};

    use notify_types::event::{
        AccessKind, AccessMode, CreateKind, DataChange, Event, EventKind, MetadataKind, ModifyKind,
        RemoveKind, RenameMode,
    };

    pub fn expected(path: impl AsRef<Path>) -> ExpectedEvent {
        ExpectedEvent::default().add_path(path)
    }

    #[derive(Debug, Default, Clone)]
    pub struct ExpectedEvent {
        kind: Option<ExpectedEventKind>,
        paths: Option<Vec<PathBuf>>,
    }

    #[derive(Debug, Clone, Copy)]
    enum ExpectedEventKind {
        Any,
        Access(Option<ExpectedAccessKind>),
        Create(Option<CreateKind>),
        Modify(Option<ExpectedModifyKind>),
        Remove(Option<RemoveKind>),
        Other,
    }

    impl PartialEq<EventKind> for ExpectedEventKind {
        fn eq(&self, other: &EventKind) -> bool {
            match self {
                ExpectedEventKind::Any => matches!(other, EventKind::Any),
                ExpectedEventKind::Access(expected) => {
                    let EventKind::Access(other) = other else {
                        return false;
                    };
                    expected.is_none_or(|expected| &expected == other)
                }
                ExpectedEventKind::Create(expected) => {
                    let EventKind::Create(other) = other else {
                        return false;
                    };
                    expected.is_none_or(|expected| &expected == other)
                }
                ExpectedEventKind::Modify(expected) => {
                    let EventKind::Modify(other) = other else {
                        return false;
                    };
                    expected.is_none_or(|expected| &expected == other)
                }
                ExpectedEventKind::Remove(expected) => {
                    let EventKind::Remove(other) = other else {
                        return false;
                    };
                    expected.is_none_or(|expected| &expected == other)
                }
                ExpectedEventKind::Other => matches!(other, EventKind::Other),
            }
        }
    }

    #[derive(Debug, Clone, Copy)]
    enum ExpectedAccessKind {
        Any,
        Read,
        Open(Option<AccessMode>),
        Close(Option<AccessMode>),
        Other,
    }

    impl PartialEq<AccessKind> for ExpectedAccessKind {
        fn eq(&self, other: &AccessKind) -> bool {
            match self {
                ExpectedAccessKind::Any => matches!(other, AccessKind::Any),
                ExpectedAccessKind::Read => matches!(other, AccessKind::Read),
                ExpectedAccessKind::Open(expected) => {
                    let AccessKind::Open(other) = other else {
                        return false;
                    };
                    expected.is_none_or(|expected| &expected == other)
                }
                ExpectedAccessKind::Close(expected) => {
                    let AccessKind::Close(other) = other else {
                        return false;
                    };
                    expected.is_none_or(|expected| &expected == other)
                }
                ExpectedAccessKind::Other => matches!(other, AccessKind::Other),
            }
        }
    }

    #[derive(Debug, Clone, Copy)]
    enum ExpectedModifyKind {
        Any,
        Data(Option<DataChange>),
        Metadata(Option<MetadataKind>),
        Name(Option<RenameMode>),
        Other,
    }

    impl PartialEq<ModifyKind> for ExpectedModifyKind {
        fn eq(&self, other: &ModifyKind) -> bool {
            match self {
                ExpectedModifyKind::Any => matches!(other, ModifyKind::Any),
                ExpectedModifyKind::Data(expected) => {
                    let ModifyKind::Data(other) = other else {
                        return false;
                    };
                    expected.is_none_or(|expected| &expected == other)
                }
                ExpectedModifyKind::Metadata(expected) => {
                    let ModifyKind::Metadata(other) = other else {
                        return false;
                    };
                    expected.is_none_or(|expected| &expected == other)
                }
                ExpectedModifyKind::Name(expected) => {
                    let ModifyKind::Name(other) = other else {
                        return false;
                    };
                    expected.is_none_or(|expected| &expected == other)
                }
                ExpectedModifyKind::Other => matches!(other, ModifyKind::Other),
            }
        }
    }

    impl PartialEq<Event> for ExpectedEvent {
        fn eq(&self, other: &Event) -> bool {
            let Self { kind, paths } = self;
            kind.is_none_or(|kind| kind == other.kind)
                | paths
                    .as_ref()
                    .is_none_or(|expected| expected == &other.paths)
        }
    }

    macro_rules! kind {
        ($name: ident, $kind: expr) => {
            pub fn $name(self) -> Self {
                self.kind($kind)
            }
        };
    }

    impl ExpectedEvent {
        pub fn add_path(mut self, path: impl AsRef<Path>) -> Self {
            match &mut self.paths {
                Some(paths) => paths.push(path.as_ref().to_path_buf()),
                None => self.paths = Some(vec![path.as_ref().to_path_buf()]),
            }
            self
        }

        kind!(any, ExpectedEventKind::Any);
        kind!(other, ExpectedEventKind::Other);

        kind!(modify, ExpectedEventKind::Modify(None));
        kind!(
            modify_any,
            ExpectedEventKind::Modify(Some(ExpectedModifyKind::Any))
        );
        kind!(
            modify_other,
            ExpectedEventKind::Modify(Some(ExpectedModifyKind::Other))
        );

        kind!(
            modify_data,
            ExpectedEventKind::Modify(Some(ExpectedModifyKind::Data(None)))
        );
        kind!(
            modify_data_any,
            ExpectedEventKind::Modify(Some(ExpectedModifyKind::Data(Some(DataChange::Any))))
        );
        kind!(
            modify_data_other,
            ExpectedEventKind::Modify(Some(ExpectedModifyKind::Data(Some(DataChange::Other))))
        );
        kind!(
            modify_data_content,
            ExpectedEventKind::Modify(Some(ExpectedModifyKind::Data(Some(DataChange::Content))))
        );
        kind!(
            modify_data_size,
            ExpectedEventKind::Modify(Some(ExpectedModifyKind::Data(Some(DataChange::Size))))
        );

        kind!(
            modify_meta,
            ExpectedEventKind::Modify(Some(ExpectedModifyKind::Metadata(None)))
        );
        kind!(
            modify_meta_any,
            ExpectedEventKind::Modify(Some(ExpectedModifyKind::Metadata(Some(MetadataKind::Any))))
        );
        kind!(
            modify_meta_other,
            ExpectedEventKind::Modify(Some(ExpectedModifyKind::Metadata(Some(
                MetadataKind::Other
            ))))
        );
        kind!(
            modify_meta_atime,
            ExpectedEventKind::Modify(Some(ExpectedModifyKind::Metadata(Some(
                MetadataKind::AccessTime
            ))))
        );
        kind!(
            modify_meta_mtime,
            ExpectedEventKind::Modify(Some(ExpectedModifyKind::Metadata(Some(
                MetadataKind::WriteTime
            ))))
        );
        kind!(
            modify_meta_owner,
            ExpectedEventKind::Modify(Some(ExpectedModifyKind::Metadata(Some(
                MetadataKind::Ownership
            ))))
        );
        kind!(
            modify_meta_ext,
            ExpectedEventKind::Modify(Some(ExpectedModifyKind::Metadata(Some(
                MetadataKind::Extended
            ))))
        );
        kind!(
            modify_meta_perm,
            ExpectedEventKind::Modify(Some(ExpectedModifyKind::Metadata(Some(
                MetadataKind::Permissions
            ))))
        );

        kind!(
            rename,
            ExpectedEventKind::Modify(Some(ExpectedModifyKind::Name(None)))
        );
        kind!(
            rename_any,
            ExpectedEventKind::Modify(Some(ExpectedModifyKind::Name(Some(RenameMode::Any))))
        );
        kind!(
            rename_other,
            ExpectedEventKind::Modify(Some(ExpectedModifyKind::Name(Some(RenameMode::Other))))
        );
        kind!(
            rename_from,
            ExpectedEventKind::Modify(Some(ExpectedModifyKind::Name(Some(RenameMode::From))))
        );
        kind!(
            rename_to,
            ExpectedEventKind::Modify(Some(ExpectedModifyKind::Name(Some(RenameMode::To))))
        );
        kind!(
            rename_both,
            ExpectedEventKind::Modify(Some(ExpectedModifyKind::Name(Some(RenameMode::Both))))
        );

        kind!(create, ExpectedEventKind::Create(None));
        kind!(create_any, ExpectedEventKind::Create(Some(CreateKind::Any)));
        kind!(
            create_other,
            ExpectedEventKind::Create(Some(CreateKind::Other))
        );
        kind!(
            create_file,
            ExpectedEventKind::Create(Some(CreateKind::File))
        );
        kind!(
            create_folder,
            ExpectedEventKind::Create(Some(CreateKind::Folder))
        );

        kind!(remove, ExpectedEventKind::Remove(None));
        kind!(remove_any, ExpectedEventKind::Remove(Some(RemoveKind::Any)));
        kind!(
            remove_other,
            ExpectedEventKind::Remove(Some(RemoveKind::Other))
        );
        kind!(
            remove_file,
            ExpectedEventKind::Remove(Some(RemoveKind::File))
        );
        kind!(
            remove_folder,
            ExpectedEventKind::Remove(Some(RemoveKind::Folder))
        );

        kind!(access, ExpectedEventKind::Access(None));
        kind!(
            access_any,
            ExpectedEventKind::Access(Some(ExpectedAccessKind::Any))
        );
        kind!(
            access_other,
            ExpectedEventKind::Access(Some(ExpectedAccessKind::Other))
        );
        kind!(
            access_read,
            ExpectedEventKind::Access(Some(ExpectedAccessKind::Read))
        );

        kind!(
            access_open,
            ExpectedEventKind::Access(Some(ExpectedAccessKind::Open(None)))
        );
        kind!(
            access_open_any,
            ExpectedEventKind::Access(Some(ExpectedAccessKind::Open(Some(AccessMode::Any))))
        );
        kind!(
            access_open_other,
            ExpectedEventKind::Access(Some(ExpectedAccessKind::Open(Some(AccessMode::Other))))
        );
        kind!(
            access_open_read,
            ExpectedEventKind::Access(Some(ExpectedAccessKind::Open(Some(AccessMode::Read))))
        );
        kind!(
            access_open_write,
            ExpectedEventKind::Access(Some(ExpectedAccessKind::Open(Some(AccessMode::Write))))
        );
        kind!(
            access_open_execute,
            ExpectedEventKind::Access(Some(ExpectedAccessKind::Open(Some(AccessMode::Execute))))
        );

        kind!(
            access_close,
            ExpectedEventKind::Access(Some(ExpectedAccessKind::Close(None)))
        );
        kind!(
            access_close_any,
            ExpectedEventKind::Access(Some(ExpectedAccessKind::Close(Some(AccessMode::Any))))
        );
        kind!(
            access_close_other,
            ExpectedEventKind::Access(Some(ExpectedAccessKind::Close(Some(AccessMode::Other))))
        );
        kind!(
            access_close_read,
            ExpectedEventKind::Access(Some(ExpectedAccessKind::Close(Some(AccessMode::Read))))
        );
        kind!(
            access_close_write,
            ExpectedEventKind::Access(Some(ExpectedAccessKind::Close(Some(AccessMode::Write))))
        );
        kind!(
            access_close_execute,
            ExpectedEventKind::Access(Some(ExpectedAccessKind::Close(Some(AccessMode::Execute))))
        );

        fn kind(mut self, kind: ExpectedEventKind) -> Self {
            self.kind = Some(kind);
            self
        }
    }
}

pub struct TestDir {
    dir: tempfile::TempDir,
    /// This is cacnonicalized path
    /// due to macos behaviour - it creates
    /// a dir with path '/var/...' but actualy it is '/private/var/...'
    ///
    /// Our watchers use canonicalized paths
    /// and send events with canonicalized paths, tho we need it
    path: PathBuf,
}

impl TestDir {
    pub fn path(&self) -> &Path {
        &self.path
    }
}

impl AsRef<Path> for TestDir {
    fn as_ref(&self) -> &Path {
        self.path()
    }
}

pub fn testdir() -> TestDir {
    let dir = tempfile::tempdir().expect("Unable to create tempdir");
    let path = std::fs::canonicalize(dir.path()).unwrap_or_else(|e| {
        panic!(
            "unable to canonicalize tempdir path {:?}: {e:?}",
            dir.path()
        )
    });
    TestDir { dir, path }
}

/// Creates a [`PollWatcher`] with comparable content and manual polling.
///
/// Returned [`Receiver`] will send a message to poll changes before wait-methods
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

#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct Cookies(Vec<usize>);

impl Cookies {
    /// Pushes new cookie if it is some and is not equal to the last
    pub fn try_push(&mut self, event: &Event) -> bool {
        let Some(tracker) = event.attrs.tracker() else {
            return false;
        };

        if self.0.last() != Some(&tracker) {
            self.0.push(tracker);
            true
        } else {
            false
        }
    }

    pub fn ensure_len<E: Debug>(&self, len: usize, events: &[E]) {
        assert_eq!(
            self.len(),
            len,
            "Unexpected cookies len. events: {events:#?}"
        )
    }
}

impl Deref for Cookies {
    type Target = [usize];

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

pub struct StoreCookies<'a, I> {
    cookies: &'a mut Cookies,
    inner: I,
}

impl<I: Iterator<Item = Event>> Iterator for StoreCookies<'_, I> {
    type Item = Event;

    fn next(&mut self) -> Option<Self::Item> {
        let event = self.inner.next()?;
        self.cookies.try_push(&event);
        Some(event)
    }
}

pub struct IgnoreAccess<I> {
    inner: I,
}

impl<I: Iterator<Item = Event>> Iterator for IgnoreAccess<I> {
    type Item = Event;

    fn next(&mut self) -> Option<Self::Item> {
        loop {
            let event = self.inner.next()?;

            if !event.kind.is_access() {
                break Some(event);
            }
        }
    }
}

pub trait TestIteratorExt: Sized {
    /// Skips any [`EventKind::Access`] events
    fn ignore_access(self) -> IgnoreAccess<Self>
    where
        Self: Sized,
    {
        IgnoreAccess { inner: self }
    }

    /// Stores any encountered [`notify_types::event::EventAttributes::tracker`] into the provided [`Cookies`]
    fn store_cookies(self, cookies: &mut Cookies) -> StoreCookies<'_, Self> {
        StoreCookies {
            cookies,
            inner: self,
        }
    }
}

impl<I: Iterator<Item = Event>> TestIteratorExt for I {}

#[rustfmt::skip] // due to annoying macro invocations formatting 
pub mod filters {
    use notify_types::event::{
        AccessKind, AccessMode, CreateKind, DataChange, Event, EventKind, MetadataKind, ModifyKind,
        RemoveKind, RenameMode,
    };

    macro_rules! filter {
        ($name: ident, $pattern:pat $(if $guard:expr)?) => {
            pub fn $name(event: Event) -> bool {
                filter_kind(event, |kind| matches!(kind, $pattern $(if $guard)?))
            }
        };
    }
    
    fn filter_kind(event: Event, f: impl FnOnce(EventKind) -> bool) -> bool {
        if f(event.kind) {
            true
        } else {
            false
        }
    }

    filter!(any, EventKind::Any);
    filter!(other, EventKind::Other);
    filter!(create, EventKind::Create(_));
    filter!(create_any, EventKind::Create(CreateKind::Any));
    filter!(create_other, EventKind::Create(CreateKind::Other));
    filter!(create_file, EventKind::Create(CreateKind::File));
    filter!(create_folder, EventKind::Create(CreateKind::Folder));

    filter!(remove, EventKind::Remove(_));
    filter!(remove_any, EventKind::Remove(RemoveKind::Any));
    filter!(remove_other, EventKind::Remove(RemoveKind::Other));
    filter!(remove_file, EventKind::Remove(RemoveKind::File));
    filter!(remove_folder, EventKind::Remove(RemoveKind::Folder));

    filter!(modify, EventKind::Modify(_));
    filter!(modify_any, EventKind::Modify(ModifyKind::Any));
    filter!(modify_other, EventKind::Modify(ModifyKind::Other));

    filter!(modify_data, EventKind::Modify(ModifyKind::Data(_)));
    filter!(modify_data_any, EventKind::Modify(ModifyKind::Data(DataChange::Any)));
    filter!(modify_data_other, EventKind::Modify(ModifyKind::Data(DataChange::Other)));
    filter!(modify_data_content, EventKind::Modify(ModifyKind::Data(DataChange::Content)));
    filter!(modify_data_size, EventKind::Modify(ModifyKind::Data(DataChange::Size)));

    filter!(modify_meta,EventKind::Modify(ModifyKind::Metadata(_)));
    filter!(modify_meta_any,EventKind::Modify(ModifyKind::Metadata(MetadataKind::Any)));
    filter!(modify_meta_other, EventKind::Modify(ModifyKind::Metadata(MetadataKind::Other)));
    filter!(modify_meta_extended, EventKind::Modify(ModifyKind::Metadata(MetadataKind::Extended)));
    filter!(modify_meta_owner, EventKind::Modify(ModifyKind::Metadata(MetadataKind::Ownership)));
    filter!(modify_meta_perm, EventKind::Modify(ModifyKind::Metadata(MetadataKind::Permissions)));
    filter!(modify_meta_mtime, EventKind::Modify(ModifyKind::Metadata(MetadataKind::WriteTime)));
    filter!(modify_meta_atime, EventKind::Modify(ModifyKind::Metadata(MetadataKind::AccessTime)));

    filter!(rename, EventKind::Modify(ModifyKind::Name(_)));
    filter!(rename_from, EventKind::Modify(ModifyKind::Name(RenameMode::From)));
    filter!(rename_to, EventKind::Modify(ModifyKind::Name(RenameMode::To)));
    filter!(rename_both, EventKind::Modify(ModifyKind::Name(RenameMode::Both)));

    filter!(access, EventKind::Access(_));
    filter!(access_any, EventKind::Access(AccessKind::Any));
    filter!(access_other, EventKind::Access(AccessKind::Other));
    filter!(access_read, EventKind::Access(AccessKind::Read));
    filter!(access_open, EventKind::Access(AccessKind::Open(_)));
    filter!(access_open_any, EventKind::Access(AccessKind::Open(AccessMode::Any)));
    filter!(access_open_other, EventKind::Access(AccessKind::Open(AccessMode::Other)));
    filter!(access_open_read, EventKind::Access(AccessKind::Open(AccessMode::Read)));
    filter!(access_open_write, EventKind::Access(AccessKind::Open(AccessMode::Write)));
    filter!(access_open_exec, EventKind::Access(AccessKind::Open(AccessMode::Execute)));
    filter!(access_close, EventKind::Access(AccessKind::Close(_)));
    filter!(access_close_any, EventKind::Access(AccessKind::Close(AccessMode::Any)));
    filter!(access_close_other, EventKind::Access(AccessKind::Close(AccessMode::Other)));
    filter!(access_close_read, EventKind::Access(AccessKind::Close(AccessMode::Read)));
    filter!(access_close_write, EventKind::Access(AccessKind::Close(AccessMode::Write)));
    filter!(access_close_exec, EventKind::Access(AccessKind::Close(AccessMode::Execute)));
}
