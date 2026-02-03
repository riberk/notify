//! Configuration types

use notify_types::event::EventKindMask;
use std::{
    fmt::Debug,
    path::{Path, PathBuf},
    time::Duration,
};

/// Indicates whether only the provided directory or its sub-directories as well should be watched
#[derive(Copy, Clone, PartialEq, Eq, PartialOrd, Ord, Debug, Hash)]
pub enum RecursiveMode {
    /// Watch all sub-directories as well, including directories created after installing the watch
    Recursive,

    /// Watch only the provided directory
    NonRecursive,
}

impl RecursiveMode {
    pub(crate) fn is_recursive(&self) -> bool {
        match *self {
            RecursiveMode::Recursive => true,
            RecursiveMode::NonRecursive => false,
        }
    }
}

/// Watcher Backend configuration
///
/// This contains multiple settings that may relate to only one specific backend,
/// such as to correctly configure each backend regardless of what is selected during runtime.
///
/// ```rust
/// # use std::time::Duration;
/// # use notify::Config;
/// let config = Config::default()
///     .with_poll_interval(Duration::from_secs(2))
///     .with_compare_contents(true);
/// ```
///
/// Some options can be changed during runtime, others have to be set when creating the watcher backend.
#[derive(Copy, Clone, PartialEq, Eq, Debug, Hash)]
pub struct Config {
    /// See [Config::with_poll_interval]
    poll_interval: Option<Duration>,

    /// See [Config::with_compare_contents]
    compare_contents: bool,

    follow_symlinks: bool,

    /// See [Config::with_event_kinds]
    event_kinds: EventKindMask,
}

impl Config {
    /// For the [`PollWatcher`](crate::PollWatcher) backend.
    ///
    /// Interval between each re-scan attempt. This can be extremely expensive for large
    /// file trees so it is recommended to measure and tune accordingly.
    ///
    /// The default poll frequency is 30 seconds.
    ///
    /// This will enable automatic polling, overwriting [`with_manual_polling()`](Config::with_manual_polling).
    pub fn with_poll_interval(mut self, dur: Duration) -> Self {
        // TODO: v7.0 break signature to option
        self.poll_interval = Some(dur);
        self
    }

    /// Returns current setting
    pub fn poll_interval(&self) -> Option<Duration> {
        // Changed Signature to Option
        self.poll_interval
    }

    /// For the [`PollWatcher`](crate::PollWatcher) backend.
    ///
    /// Disable automatic polling. Requires calling [`crate::PollWatcher::poll()`] manually.
    ///
    /// This will disable automatic polling, overwriting [`with_poll_interval()`](Config::with_poll_interval).
    pub fn with_manual_polling(mut self) -> Self {
        self.poll_interval = None;
        self
    }

    /// For the [`PollWatcher`](crate::PollWatcher) backend.
    ///
    /// Optional feature that will evaluate the contents of changed files to determine if
    /// they have indeed changed using a fast hashing algorithm.  This is especially important
    /// for pseudo filesystems like those on Linux under /sys and /proc which are not obligated
    /// to respect any other filesystem norms such as modification timestamps, file sizes, etc.
    /// By enabling this feature, performance will be significantly impacted as all files will
    /// need to be read and hashed at each `poll_interval`.
    ///
    /// This can't be changed during runtime. Off by default.
    pub fn with_compare_contents(mut self, compare_contents: bool) -> Self {
        self.compare_contents = compare_contents;
        self
    }

    /// Returns current setting
    pub fn compare_contents(&self) -> bool {
        self.compare_contents
    }

    /// For the [INotifyWatcher](crate::INotifyWatcher), [KqueueWatcher](crate::KqueueWatcher),
    /// and [PollWatcher](crate::PollWatcher).
    ///
    /// Determine if symbolic links should be followed when recursively watching a directory.
    ///
    /// This can't be changed during runtime. On by default.
    pub fn with_follow_symlinks(mut self, follow_symlinks: bool) -> Self {
        self.follow_symlinks = follow_symlinks;
        self
    }

    /// Returns current setting
    pub fn follow_symlinks(&self) -> bool {
        self.follow_symlinks
    }

    /// Filter which event kinds are monitored.
    ///
    /// This allows you to control which types of filesystem events are delivered
    /// to your event handler. On backends that support kernel-level filtering
    /// (inotify), the mask is translated to native flags for optimal
    /// performance. On other backends (kqueue, Windows, FSEvents, PollWatcher),
    /// filtering is applied in userspace.
    ///
    /// The default is [`EventKindMask::ALL`], which includes all events.
    /// Use [`EventKindMask::CORE`] to exclude access events.
    ///
    /// This can't be changed during runtime.
    ///
    /// # Example
    ///
    /// ```rust
    /// use notify::{Config, EventKindMask};
    ///
    /// // Only monitor file creation and deletion
    /// let config = Config::default()
    ///     .with_event_kinds(EventKindMask::CREATE | EventKindMask::REMOVE);
    ///
    /// // Monitor everything including access events
    /// let config_all = Config::default()
    ///     .with_event_kinds(EventKindMask::ALL);
    /// ```
    pub fn with_event_kinds(mut self, event_kinds: EventKindMask) -> Self {
        self.event_kinds = event_kinds;
        self
    }

    /// Returns current setting
    pub fn event_kinds(&self) -> EventKindMask {
        self.event_kinds
    }
}

impl Default for Config {
    fn default() -> Self {
        Self {
            poll_interval: Some(Duration::from_secs(30)),
            compare_contents: false,
            follow_symlinks: true,
            event_kinds: EventKindMask::ALL,
        }
    }
}

/// Single watch backend configuration
///
/// This contains some settings that may relate to only one specific backend,
/// such as to correctly configure each backend regardless of what is selected during runtime.
#[derive(Debug)]
pub struct WatchPathConfig {
    pub(crate) recursive_mode: RecursiveMode,
    pub(crate) watch_filter: WatchFilter,
}

impl WatchPathConfig {
    /// Creates new instance with provided [`RecursiveMode`]
    pub fn new(recursive_mode: RecursiveMode) -> Self {
        Self {
            recursive_mode,
            watch_filter: Default::default(),
        }
    }

    /// Set [`RecursiveMode`] for the watch
    pub fn with_recursive_mode(mut self, recursive_mode: RecursiveMode) -> Self {
        self.recursive_mode = recursive_mode;
        self
    }

    /// Sets a [`WatchPathFilter`] used to decide which paths should be
    /// registered for watching when watches are added implicitly.
    ///
    /// The filter is evaluated when new paths are added in recursive mode.
    /// Returning `false` prevents the corresponding path from being watched.
    ///
    /// This is primarily useful for the `inotify` backend, where recursive
    /// watching is implemented by registering every subdirectory and
    /// iterating over directory entries.
    /// Using a filter allows limiting watch growth (e.g. skipping `.git`,
    /// `target`, temporary directories, etc.).
    ///
    /// # Important
    ///
    /// This filters *watches*, not *events*.
    ///
    /// On fully recursive backends such as macOS `FSEvents` and Windows
    /// `ReadDirectoryChangesW`, this has no effect because those backends
    /// do not register individual subdirectory watches.
    ///
    /// This also has no effect if watches are added explicitly via
    /// [`crate::Watcher::watch`] or [`crate::Watcher::update_paths`].
    pub fn with_implicit_watches_filter(
        mut self,
        filter: impl WatchPathFilter + Send + 'static,
    ) -> Self {
        self.watch_filter = WatchFilter::new(filter);
        self
    }

    /// Returns current setting
    pub fn recursive_mode(&self) -> RecursiveMode {
        self.recursive_mode
    }
}

/// An operation to apply to a watcher
///
/// See [`Watcher::update_paths`] for more information
#[derive(Debug)]
pub enum PathOp {
    /// Path should be watched
    Watch(PathBuf, WatchPathConfig),

    /// Path should be unwatched
    Unwatch(PathBuf),
}

impl PathOp {
    /// Watch the path with [`RecursiveMode::Recursive`]
    pub fn watch_recursive<P: Into<PathBuf>>(path: P) -> Self {
        Self::Watch(path.into(), WatchPathConfig::new(RecursiveMode::Recursive))
    }

    /// Watch the path with [`RecursiveMode::NonRecursive`]
    pub fn watch_non_recursive<P: Into<PathBuf>>(path: P) -> Self {
        Self::Watch(
            path.into(),
            WatchPathConfig::new(RecursiveMode::NonRecursive),
        )
    }

    /// Unwatch the path
    pub fn unwatch<P: Into<PathBuf>>(path: P) -> Self {
        Self::Unwatch(path.into())
    }

    /// Returns the path associated with this operation.
    pub fn as_path(&self) -> &Path {
        match self {
            PathOp::Watch(p, _) => p,
            PathOp::Unwatch(p) => p,
        }
    }

    /// Returns the path associated with this operation.
    pub fn into_path(self) -> PathBuf {
        match self {
            PathOp::Watch(p, _) => p,
            PathOp::Unwatch(p) => p,
        }
    }
}

/// A predicate used to decide whether a path should be watched.
///
/// Implementations can provide custom logic to include or exclude paths
/// during watch registration (for example, to skip certain directories
/// or temporary files).
///
/// The filter is stateful (`&mut self`) to allow implementations to
/// maintain internal caches or counters if necessary.
pub trait WatchPathFilter {
    /// Returns whether the given `path` should be watched.
    ///
    /// This method may be called multiple times during recursive watch
    /// traversal or when new filesystem entries appear.
    ///
    /// Returning `true` means the path should be watched.
    /// Returning `false` means the path should be skipped.
    fn should_watch(&mut self, path: &Path) -> bool;
}

/// An optional, dynamically-dispatched [`WatchPathFilter`].
///
/// `WatchFilter` is a lightweight wrapper around
/// `Option<Box<dyn WatchPathFilter>>` that provides a convenient way
/// to configure path filtering without exposing trait objects directly.
///
/// By default, no filter is installed, meaning all paths are allowed.
#[derive(Default)]
pub struct WatchFilter(Option<Box<dyn WatchPathFilter + Send + 'static>>);

impl Debug for WatchFilter {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_tuple("WatchFilter")
            .field(match &self.0 {
                Some(_) => &"Some(...)",
                None => &"None",
            })
            .finish()
    }
}

impl WatchFilter {
    /// Creates a `WatchFilter` with no filtering.
    ///
    /// All paths will be considered allowed.
    ///
    /// This is equivalent to [`Default::default`].
    pub const fn none() -> Self {
        Self(None)
    }

    /// Creates a `WatchFilter` from the given filter implementation.
    ///
    /// The provided filter must be `Send` and `'static` since it may be
    /// stored inside a watcher that runs on a background thread.
    ///
    /// # Example
    ///
    /// ### Using a closure (`FnMut`):
    /// ```
    /// # use std::path::Path;
    /// # use notify::{WatchFilter, WatchPathFilter};
    ///
    /// let mut counter = 0usize;
    /// let mut filter = WatchFilter::new(move |_: &Path| {
    ///     if counter >= 2 {
    ///         false
    ///     } else {
    ///         counter += 1;
    ///         true
    ///     }
    /// });
    ///
    /// assert!(filter.as_filter_mut().is_some_and(|filter| filter.should_watch(Path::new("1"))));
    /// assert!(filter.as_filter_mut().is_some_and(|filter| filter.should_watch(Path::new("1"))));
    /// assert!(!filter.as_filter_mut().is_some_and(|filter| filter.should_watch(Path::new("1"))));
    /// ```
    ///
    /// ### Using a custom type
    /// ```
    /// # use std::path::Path;
    /// # use notify::{WatchFilter, WatchPathFilter};
    /// struct SkipGitDir;
    ///
    /// impl WatchPathFilter for SkipGitDir {
    ///     fn should_watch(&mut self, path: &Path) -> bool {
    ///         !path.ends_with(".git")
    ///     }
    /// }
    ///
    /// let mut filter = WatchFilter::new(SkipGitDir);
    /// assert!(!filter.as_filter_mut().is_some_and(|filter| filter.should_watch(Path::new("parent/.git"))));
    /// assert!(filter.as_filter_mut().is_some_and(|filter| filter.should_watch(Path::new("parent/README"))));
    /// ```
    pub fn new(filter: impl WatchPathFilter + Send + 'static) -> Self {
        Self(Some(Box::new(filter)))
    }

    /// Returns a mutable reference to the underlying filter, if present.
    pub fn as_filter_mut(&mut self) -> Option<&mut (dyn WatchPathFilter + Send + 'static)> {
        self.0.as_deref_mut()
    }

    /// Consumes the `WatchFilter` and returns the underlying boxed filter.
    pub fn into_filter(self) -> Option<Box<(dyn WatchPathFilter + Send + 'static)>> {
        self.0
    }
}

impl<F> WatchPathFilter for F
where
    F: FnMut(&Path) -> bool,
{
    fn should_watch(&mut self, path: &Path) -> bool {
        self(path)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use notify_types::event::EventKindMask;

    #[test]
    fn config_default_event_kinds_is_all() {
        let config = Config::default();
        assert_eq!(config.event_kinds(), EventKindMask::ALL);
    }

    #[test]
    fn config_with_event_kinds() {
        let mask = EventKindMask::CREATE | EventKindMask::REMOVE;
        let config = Config::default().with_event_kinds(mask);
        assert_eq!(config.event_kinds(), mask);
    }

    #[test]
    fn config_with_all_events_includes_access() {
        let config = Config::default().with_event_kinds(EventKindMask::ALL);
        assert!(config.event_kinds().intersects(EventKindMask::ALL_ACCESS));
    }

    #[test]
    fn config_with_empty_mask() {
        let config = Config::default().with_event_kinds(EventKindMask::empty());
        assert!(config.event_kinds().is_empty());
    }

    #[test]
    fn event_kind_mask_default_matches_config_default() {
        // Verify cross-crate consistency: both defaults should be ALL
        assert_eq!(EventKindMask::default(), Config::default().event_kinds());
        assert_eq!(EventKindMask::default(), EventKindMask::ALL);
    }
}
