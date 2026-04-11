//! Terminal progress reporting helpers for transfer progress updates.

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
    /// Current transfer phase such as `scanning` or `sending`.
    pub phase: String,
    /// Total number of bytes expected for the transfer.
    pub total_bytes: u64,
    /// Total number of bytes confirmed so far.
    pub transferred_bytes: u64,
    /// Percentage complete in the range `0.0..=100.0`.
    pub percent_complete: f64,
    /// Average transfer speed in mebibytes per second.
    pub average_mib_per_sec: f64,
    /// Number of files completed so far.
    pub completed_files: u64,
    /// Total number of files discovered for the package.
    pub total_files: u64,
    /// Current relative file path being processed, when available.
    pub current_path: Option<String>,
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
    phase: String,
    total_bytes: u64,
    transferred_bytes: u64,
    total_files: u64,
    completed_files: u64,
    current_path: Option<String>,
    started_at: Instant,
    last_rendered_at: Instant,
    render_terminal: bool,
    listener: Option<ProgressListener>,
}

impl ProgressReporter {
    /// Creates a new progress reporter with a human-readable label.
    pub fn new(label: impl Into<String>, total_bytes: u64, total_files: u64) -> Self {
        let now = Instant::now();
        Self {
            label: label.into(),
            phase: "sending".to_owned(),
            total_bytes,
            transferred_bytes: 0,
            total_files,
            completed_files: 0,
            current_path: None,
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

    /// Updates the current transfer phase and emits a progress snapshot.
    pub fn set_phase(&mut self, phase: impl Into<String>) {
        self.phase = phase.into();
        self.emit(false);
    }

    /// Updates discovered transfer totals and emits a progress snapshot.
    pub fn set_totals(&mut self, total_bytes: u64, total_files: u64) {
        self.total_bytes = total_bytes;
        self.total_files = total_files;
        self.completed_files = self.completed_files.min(self.total_files);
        self.emit(false);
    }

    /// Updates the current file path and label context.
    pub fn set_current_path(&mut self, path: Option<String>) {
        self.current_path = path;
        self.emit(false);
    }

    /// Updates the number of completed files.
    pub fn set_completed_files(&mut self, completed_files: u64) {
        self.completed_files = completed_files.min(self.total_files);
        self.emit(false);
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
                phase: self.phase.clone(),
                total_bytes: self.total_bytes,
                transferred_bytes: self.transferred_bytes,
                percent_complete: percent_complete(self.transferred_bytes, self.total_bytes),
                average_mib_per_sec: mib_per_sec(self.transferred_bytes, self.started_at.elapsed()),
                completed_files: self.completed_files,
                total_files: self.total_files,
                current_path: self.current_path.clone(),
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
        let current_path = self.current_path.as_deref().unwrap_or("preparing package");

        print!(
            "\r{} [{}]: {:>6.2}% at {:>8.2} MiB/s [{} / {} files] {}",
            self.label,
            self.phase,
            percent,
            speed,
            self.completed_files,
            self.total_files,
            current_path,
        );
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

