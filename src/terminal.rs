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

impl TerminalInfo {
    /// Detect terminal information from the current environment
    ///
    /// This function performs several detections:
    /// - Terminal size (width/height) - fast, non-blocking
    /// - TTY detection - fast, non-blocking
    /// - Color support - fast, non-blocking
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

        TerminalInfo {
            width,
            height,
            is_tty,
            color_support,
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
