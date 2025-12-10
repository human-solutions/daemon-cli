use serde::{Deserialize, Serialize};
use std::io::IsTerminal;

/// Terminal information detected from the client environment
#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct TerminalInfo {
    /// Terminal width in columns (can fail in headless environments)
    pub width: Option<u16>,
    /// Terminal height in rows (can fail in headless environments)
    pub height: Option<u16>,
    /// Whether stdout is connected to a TTY (always determinable)
    pub is_tty: bool,
    /// Level of color support (always returns at least ColorSupport::None)
    pub color_support: ColorSupport,
    /// Terminal color theme (None if detection fails/times out)
    #[serde(default)]
    pub theme: Option<Theme>,
}

/// Level of color support in the terminal
#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq, Eq)]
pub enum ColorSupport {
    /// No color support
    None,
    /// Basic 16 colors (ANSI)
    Basic16,
    /// 256 colors
    Colors256,
    /// Truecolor (16 million colors)
    Truecolor,
}

/// Terminal color theme (dark or light mode)
#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq, Eq)]
pub enum Theme {
    /// Dark background with light text
    Dark,
    /// Light background with dark text
    Light,
}

impl TerminalInfo {
    /// Detect terminal information from the current environment
    ///
    /// This function performs several detections:
    /// - Terminal size (width/height) - fast, non-blocking
    /// - TTY detection - fast, non-blocking
    /// - Color support - fast, non-blocking
    /// - Theme detection - up to 50ms timeout
    ///
    /// Individual detections can fail, but the function always returns
    /// a TerminalInfo struct with whatever information could be gathered.
    pub async fn detect() -> Self {
        // Fast, non-blocking detections
        let is_tty = std::io::stdout().is_terminal();

        let (width, height) = terminal_size::terminal_size()
            .map(|(terminal_size::Width(w), terminal_size::Height(h))| (Some(w), Some(h)))
            .unwrap_or((None, None));

        let color_support = detect_color_support();
        let theme = detect_theme();

        TerminalInfo {
            width,
            height,
            is_tty,
            color_support,
            theme,
        }
    }
}

/// Detect color support level
fn detect_color_support() -> ColorSupport {
    match supports_color::on(supports_color::Stream::Stdout) {
        Some(level) => {
            if level.has_16m {
                ColorSupport::Truecolor
            } else if level.has_256 {
                ColorSupport::Colors256
            } else if level.has_basic {
                ColorSupport::Basic16
            } else {
                ColorSupport::None
            }
        }
        None => ColorSupport::None,
    }
}

/// Check if we're running in a test environment where terminal queries may hang.
/// See: https://github.com/bash/terminal-colorsaurus/issues/38
fn is_test_environment() -> bool {
    // NEXTEST is set by cargo-nextest
    // RUST_TEST_THREADS is set by cargo test
    std::env::var("NEXTEST").is_ok() || std::env::var("RUST_TEST_THREADS").is_ok()
}

/// Detect terminal theme (dark/light mode)
fn detect_theme() -> Option<Theme> {
    use std::time::Duration;
    use terminal_colorsaurus::{QueryOptions, color_scheme};

    // Skip terminal detection in test environments to avoid hangs
    if is_test_environment() {
        return None;
    }

    let mut options = QueryOptions::default();
    options.timeout = Duration::from_millis(50);

    match color_scheme(options) {
        Ok(terminal_colorsaurus::ColorScheme::Dark) => Some(Theme::Dark),
        Ok(terminal_colorsaurus::ColorScheme::Light) => Some(Theme::Light),
        Err(_) => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_terminal_info_detect() {
        // Should always return a TerminalInfo struct
        let info = TerminalInfo::detect().await;

        // is_tty and color_support are always determinable
        // Just verify we got a result (values depend on test environment)
        assert!(matches!(
            info.color_support,
            ColorSupport::None
                | ColorSupport::Basic16
                | ColorSupport::Colors256
                | ColorSupport::Truecolor
        ));
    }

    #[test]
    fn test_color_support_detection() {
        let color_support = detect_color_support();
        // Should always return a valid ColorSupport value
        assert!(matches!(
            color_support,
            ColorSupport::None
                | ColorSupport::Basic16
                | ColorSupport::Colors256
                | ColorSupport::Truecolor
        ));
    }
}
