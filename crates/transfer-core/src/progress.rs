//! Terminal progress reporting helpers for streaming transfers.

use std::{
    io::{self, Write},
    sync::Arc,
    time::{Duration, Instant},
};

/// Shared callback used by UI integrations to observe transfer progress.
pub type ProgressListener = Arc<dyn Fn(ProgressUpdate) + Send + Sync + 'static>;

/// Snapshot of the current transfer progress.
#[derive(Debug, Clone)]
pub struct ProgressUpdate {
    /// Human-readable transfer label.
    pub label: String,
    /// Total number of bytes expected for the transfer.
    pub total_bytes: u64,
    /// Total number of bytes confirmed so far.
    pub transferred_bytes: u64,
    /// Percentage complete in the range `0.0..=100.0`.
    pub percent_complete: f64,
    /// Average transfer speed in mebibytes per second.
    pub average_mib_per_sec: f64,
    /// Whether this is the terminal completion update.
    pub completed: bool,
}

/// Final transfer timing and throughput information.
#[derive(Debug, Clone, Copy)]
pub struct ProgressSnapshot {
    /// Total number of bytes transferred.
    pub bytes_transferred: u64,
    /// Total elapsed wall-clock time.
    pub elapsed: Duration,
    /// Average transfer speed in mebibytes per second.
    pub average_mib_per_sec: f64,
}

/// Lightweight progress renderer for terminal-based transfers.
pub struct ProgressReporter {
    label: String,
    total_bytes: u64,
    transferred_bytes: u64,
    started_at: Instant,
    last_rendered_at: Instant,
    render_terminal: bool,
    listener: Option<ProgressListener>,
}

impl ProgressReporter {
    /// Creates a new progress reporter with a human-readable label.
    pub fn new(label: impl Into<String>, total_bytes: u64) -> Self {
        let now = Instant::now();
        Self {
            label: label.into(),
            total_bytes,
            transferred_bytes: 0,
            started_at: now,
            last_rendered_at: now,
            render_terminal: true,
            listener: None,
        }
    }

    /// Registers a callback that receives progress snapshots as the transfer advances.
    pub fn with_listener(mut self, listener: ProgressListener) -> Self {
        self.listener = Some(listener);
        self
    }

    /// Disables terminal rendering while keeping progress callbacks active.
    pub fn without_terminal(mut self) -> Self {
        self.render_terminal = false;
        self
    }

    /// Records additional transferred bytes and updates the terminal periodically.
    pub fn advance(&mut self, delta: u64) {
        self.transferred_bytes = self.transferred_bytes.saturating_add(delta);
        self.emit(false);

        let should_render = self.last_rendered_at.elapsed() >= Duration::from_millis(250)
            || self.transferred_bytes >= self.total_bytes;

        if should_render && self.render_terminal {
            self.render(false);
        }
    }

    /// Renders a final line and returns the summary snapshot.
    pub fn finish(&mut self) -> ProgressSnapshot {
        if self.render_terminal {
            self.render(true);
            println!();
        }

        let elapsed = self.started_at.elapsed();
        let snapshot = ProgressSnapshot {
            bytes_transferred: self.transferred_bytes,
            elapsed,
            average_mib_per_sec: mib_per_sec(self.transferred_bytes, elapsed),
        };
        self.emit(true);
        snapshot
    }

    fn emit(&self, completed: bool) {
        if let Some(listener) = &self.listener {
            listener(ProgressUpdate {
                label: self.label.clone(),
                total_bytes: self.total_bytes,
                transferred_bytes: self.transferred_bytes,
                percent_complete: percent_complete(self.transferred_bytes, self.total_bytes),
                average_mib_per_sec: mib_per_sec(self.transferred_bytes, self.started_at.elapsed()),
                completed,
            });
        }
    }

    fn render(&mut self, force: bool) {
        if !force && self.total_bytes > 0 && self.transferred_bytes == 0 {
            return;
        }

        let percent = percent_complete(self.transferred_bytes, self.total_bytes);
        let elapsed = self.started_at.elapsed();
        let speed = mib_per_sec(self.transferred_bytes, elapsed);

        print!("\r{}: {:>6.2}% at {:>8.2} MiB/s", self.label, percent, speed,);
        let _ = io::stdout().flush();
        self.last_rendered_at = Instant::now();
    }
}

fn mib_per_sec(bytes: u64, elapsed: Duration) -> f64 {
    let seconds = elapsed.as_secs_f64().max(0.001);
    bytes as f64 / 1024.0 / 1024.0 / seconds
}

fn percent_complete(transferred_bytes: u64, total_bytes: u64) -> f64 {
    if total_bytes == 0 {
        100.0
    } else {
        (transferred_bytes as f64 / total_bytes as f64) * 100.0
    }
}
