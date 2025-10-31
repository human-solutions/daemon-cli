//! Client-side error context capture
//!
//! Maintains a circular buffer of recent log entries that are normally suppressed
//! but displayed to stderr when an error occurs.

use parking_lot::Mutex;
use std::collections::VecDeque;
use std::fmt::Write as FmtWrite;
use std::sync::{Arc, OnceLock};
use tracing::field::{Field, Visit};
use tracing::{Event, Subscriber};
use tracing_subscriber::layer::{Context, Layer};
use tracing_subscriber::registry::LookupSpan;

const BUFFER_SIZE: usize = 50;

/// Global error context buffer shared across all client instances
static GLOBAL_ERROR_CONTEXT: OnceLock<ErrorContextBuffer> = OnceLock::new();

/// A single log entry in the error context buffer
#[derive(Clone)]
struct LogEntry {
    level: tracing::Level,
    message: String,
    timestamp: std::time::Instant,
}

/// Circular buffer for capturing recent log entries
#[derive(Clone)]
pub struct ErrorContextBuffer {
    entries: Arc<Mutex<VecDeque<LogEntry>>>,
    start_time: std::time::Instant,
}

impl ErrorContextBuffer {
    pub fn new() -> Self {
        Self {
            entries: Arc::new(Mutex::new(VecDeque::with_capacity(BUFFER_SIZE))),
            start_time: std::time::Instant::now(),
        }
    }

    fn add_entry(&self, level: tracing::Level, message: String) {
        let mut entries = self.entries.lock();
        if entries.len() >= BUFFER_SIZE {
            entries.pop_front();
        }
        entries.push_back(LogEntry {
            level,
            message,
            timestamp: std::time::Instant::now(),
        });
    }

    /// Dump buffered logs to stderr, typically called on error
    pub fn dump_to_stderr(&self) {
        let entries = self.entries.lock();
        if entries.is_empty() {
            return;
        }

        eprintln!("\n--- Client Debug Log (last {} entries) ---", entries.len());
        for entry in entries.iter() {
            let elapsed = entry.timestamp.duration_since(self.start_time);
            eprintln!(
                "[{:>8.3}s] {:5} {}",
                elapsed.as_secs_f64(),
                entry.level,
                entry.message
            );
        }
        eprintln!("--- End Debug Log ---\n");
    }
}

impl Default for ErrorContextBuffer {
    fn default() -> Self {
        Self::new()
    }
}

/// Get or initialize the global error context buffer and ensure tracing is set up
///
/// This function is idempotent - it can be called multiple times safely.
/// The first call initializes the global buffer and tracing subscriber,
/// subsequent calls return the same shared buffer.
pub fn get_or_init_global_error_context() -> ErrorContextBuffer {
    GLOBAL_ERROR_CONTEXT
        .get_or_init(|| {
            let buffer = ErrorContextBuffer::new();
            let layer = ErrorContextLayer::new(buffer.clone());

            // Initialize tracing subscriber (only succeeds once per process)
            use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt};
            let _ = tracing_subscriber::registry().with(layer).try_init();

            buffer
        })
        .clone()
}

/// Tracing layer that captures events to the error context buffer
pub struct ErrorContextLayer {
    buffer: ErrorContextBuffer,
}

impl ErrorContextLayer {
    pub fn new(buffer: ErrorContextBuffer) -> Self {
        Self { buffer }
    }
}

impl<S> Layer<S> for ErrorContextLayer
where
    S: Subscriber + for<'a> LookupSpan<'a>,
{
    fn on_event(&self, event: &Event<'_>, _ctx: Context<'_, S>) {
        let metadata = event.metadata();
        let mut visitor = MessageVisitor::default();
        event.record(&mut visitor);

        self.buffer
            .add_entry(*metadata.level(), visitor.message);
    }
}

/// Visitor to extract the message from a tracing event
#[derive(Default)]
struct MessageVisitor {
    message: String,
}

impl Visit for MessageVisitor {
    fn record_debug(&mut self, field: &Field, value: &dyn std::fmt::Debug) {
        if field.name() == "message" {
            let _ = write!(self.message, "{:?}", value);
        } else {
            if !self.message.is_empty() {
                self.message.push_str(", ");
            }
            let _ = write!(self.message, "{}={:?}", field.name(), value);
        }
    }

    fn record_str(&mut self, field: &Field, value: &str) {
        if field.name() == "message" {
            self.message.push_str(value);
        } else {
            if !self.message.is_empty() {
                self.message.push_str(", ");
            }
            let _ = write!(self.message, "{}={}", field.name(), value);
        }
    }
}
