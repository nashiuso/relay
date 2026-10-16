//! Terminal output helpers.
//!
//! Relay keeps its terminal output plain: no animation, no spinner and no
//! colour when the output is not a terminal. Colours are only used to make
//! the important parts of a line easier to spot.

use std::io::IsTerminal;

/// The brand colours of Relay.
pub mod color {
    /// The accent colour of the brand.
    pub const ACCENT: &str = "\u{1b}[38;5;109m";
    /// The dim colour for secondary information.
    pub const DIM: &str = "\u{1b}[2m";
    /// The bold attribute.
    pub const BOLD: &str = "\u{1b}[1m";
    /// The red colour for errors.
    pub const RED: &str = "\u{1b}[31m";
    /// The green colour for success.
    pub const GREEN: &str = "\u{1b}[32m";
    /// The yellow colour for warnings.
    pub const YELLOW: &str = "\u{1b}[33m";
    /// Resets all attributes.
    pub const RESET: &str = "\u{1b}[0m";
}

/// Controls how the CLI writes to the terminal.
#[derive(Debug, Clone, Copy)]
pub struct Style {
    /// Whether ANSI colours may be used.
    pub color: bool,
    /// Whether informational output is suppressed.
    pub quiet: bool,
    /// Whether output has to be machine readable.
    pub json: bool,
}

impl Style {
    /// Creates a style that detects colour support from the environment.
    pub fn detect(no_color: bool, quiet: bool, json: bool) -> Style {
        let color = !no_color
            && !json
            && std::env::var_os("NO_COLOR").is_none()
            && std::io::stdout().is_terminal();
        Style { color, quiet, json }
    }

    /// Creates a style without colour.
    pub fn plain() -> Style {
        Style {
            color: false,
            quiet: false,
            json: false,
        }
    }

    fn paint(&self, code: &str, text: &str) -> String {
        if self.color {
            format!("{code}{text}{}", color::RESET)
        } else {
            text.to_owned()
        }
    }

    /// Highlights a value.
    pub fn accent(&self, text: &str) -> String {
        self.paint(color::ACCENT, text)
    }

    /// Emphasises a value.
    pub fn bold(&self, text: &str) -> String {
        self.paint(color::BOLD, text)
    }

    /// De-emphasises a value.
    pub fn dim(&self, text: &str) -> String {
        self.paint(color::DIM, text)
    }

    /// Marks an error.
    pub fn error(&self, text: &str) -> String {
        self.paint(color::RED, text)
    }

    /// Marks a success.
    pub fn success(&self, text: &str) -> String {
        self.paint(color::GREEN, text)
    }

    /// Marks a warning.
    pub fn warning(&self, text: &str) -> String {
        self.paint(color::YELLOW, text)
    }
}

/// Writes an informational line to stdout, unless the CLI is quiet.
pub fn info(style: &Style, text: &str) {
    if !style.quiet && !style.json {
        println!("{text}");
    }
}

/// Writes a note to stderr. Notes are never suppressed by `--quiet`.
pub fn note(style: &Style, text: &str) {
    if !style.json {
        eprintln!("{}", style.dim(text));
    }
}

/// Writes an error to stderr.
pub fn error(style: &Style, text: &str) {
    eprintln!("{} {}", style.error("error:"), text);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plain_styles_do_not_add_escape_codes() {
        let style = Style::plain();
        assert_eq!(style.accent("x"), "x");
        assert_eq!(style.error("x"), "x");
    }

    #[test]
    fn coloured_styles_wrap_the_text() {
        let style = Style {
            color: true,
            quiet: false,
            json: false,
        };
        assert!(style.accent("x").contains("x"));
        assert!(style.accent("x").starts_with('\u{1b}'));
    }
}
