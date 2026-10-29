use std::fmt;
use std::sync::Arc;

use tokio::sync::mpsc;

/// A high-level installation phase, used for progress display.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Phase {
    Version,
    Libraries,
    Assets,
    Natives,
    Launch,
}

impl fmt::Display for Phase {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let name = match self {
            Self::Version => "version",
            Self::Libraries => "libraries",
            Self::Assets => "assets",
            Self::Natives => "natives",
            Self::Launch => "launch",
        };
        f.write_str(name)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LogStream {
    Stdout,
    Stderr,
}

/// Progress information emitted by long-running core operations.
#[derive(Debug, Clone)]
pub enum ProgressEvent {
    /// A description of the current step.
    Step { phase: Phase, message: String },
    /// Aggregate progress for a batch of files.
    Batch {
        phase: Phase,
        completed: u64,
        total: u64,
        file: String,
    },
    /// A line captured from the game process.
    Log { stream: LogStream, line: String },
}

/// A thread-safe progress callback.
pub type ProgressSink = Arc<dyn Fn(ProgressEvent) + Send + Sync>;

/// A sink that discards all events.
pub fn noop_progress() -> ProgressSink {
    Arc::new(|_| {})
}

/// Create a progress sink backed by an unbounded channel.
pub fn channel_progress() -> (ProgressSink, mpsc::UnboundedReceiver<ProgressEvent>) {
    let (tx, rx) = mpsc::unbounded_channel();
    let sink: ProgressSink = Arc::new(move |event| {
        let _ = tx.send(event);
    });
    (sink, rx)
}
