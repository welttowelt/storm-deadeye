//! Output framework — colored / tabular / plain / JSON rendering.
//!
//! Auto-detects whether stdout is a TTY and degrades to plain text when
//! piped. The renderer is a tiny owned struct so callers can pass it
//! around without lifetimes; it allocates on demand for color escapes.
//!
//! ```ignore
//! let mode = OutputMode::detect(cli_override, cli_no_color);
//! let renderer = Renderer::new(mode);
//! renderer.header("Markets");
//! renderer.print(&summary);
//! ```

use std::io::{self, IsTerminal, Write};

use owo_colors::{OwoColorize as _, Style};
use serde::Serialize;

/// How we render command output.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum OutputMode {
    /// Colored text + tables. Used on a TTY when not overridden.
    Pretty,
    /// `key: value` lines. No colors, no boxes.
    Plain,
    /// `serde_json::to_writer_pretty(stdout, value)`.
    Json,
}

impl OutputMode {
    /// Auto-detect: TTY → Pretty, pipe → Plain. CLI flag overrides.
    /// `NO_COLOR` env var or `--no-color` forces Plain when no override.
    pub(crate) fn detect(override_: Option<Self>, no_color: bool) -> Self {
        if let Some(o) = override_ {
            return o;
        }
        let is_tty = io::stdout().is_terminal();
        let no_color_env = std::env::var_os("NO_COLOR").is_some();
        if !is_tty || no_color || no_color_env {
            Self::Plain
        } else {
            Self::Pretty
        }
    }
}

/// Common rendering surface — every command writes through here.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Renderer {
    mode: OutputMode,
    color_enabled: bool,
}

impl Renderer {
    pub(crate) fn new(mode: OutputMode) -> Self {
        let color_enabled = matches!(mode, OutputMode::Pretty);
        Self {
            mode,
            color_enabled,
        }
    }

    pub(crate) const fn mode(&self) -> OutputMode {
        self.mode
    }

    /// Print a top-level value. Renders pretty or plain via the trait;
    /// JSON falls through to serde.
    pub(crate) fn print<T>(&self, value: &T) -> anyhow::Result<()>
    where
        T: Serialize + Render,
    {
        match self.mode {
            OutputMode::Json => {
                let stdout = io::stdout();
                let mut handle = stdout.lock();
                serde_json::to_writer_pretty(&mut handle, value)?;
                handle.write_all(b"\n")?;
                Ok(())
            },
            OutputMode::Pretty => {
                value.render_pretty(self);
                Ok(())
            },
            OutputMode::Plain => {
                let stdout = io::stdout();
                let mut handle = stdout.lock();
                value.render_plain(&mut handle)?;
                Ok(())
            },
        }
    }

    /// Print a list of renderable rows. JSON dumps the whole array; the
    /// row type controls the human-facing rendering.
    pub(crate) fn print_table<R>(&self, rows: &[R]) -> anyhow::Result<()>
    where
        R: Serialize + Render,
    {
        match self.mode {
            OutputMode::Json => {
                let stdout = io::stdout();
                let mut handle = stdout.lock();
                serde_json::to_writer_pretty(&mut handle, rows)?;
                handle.write_all(b"\n")?;
                Ok(())
            },
            OutputMode::Pretty => {
                if rows.is_empty() {
                    println!("{}", self.dim("(no results)"));
                } else if let Some(first) = rows.first() {
                    first.render_pretty_table_header(self);
                    for row in rows {
                        row.render_pretty_table_row(self);
                    }
                    first.render_pretty_table_footer(self);
                }
                Ok(())
            },
            OutputMode::Plain => {
                let stdout = io::stdout();
                let mut handle = stdout.lock();
                for (i, row) in rows.iter().enumerate() {
                    if i > 0 {
                        writeln!(handle, "---")?;
                    }
                    row.render_plain(&mut handle)?;
                }
                Ok(())
            },
        }
    }

    pub(crate) fn header(&self, text: &str) {
        if self.color_enabled {
            println!("{}", text.bold().cyan());
        } else {
            println!("{text}");
        }
    }

    pub(crate) fn success(&self, msg: &str) {
        if self.color_enabled {
            println!("{} {}", "✓".green(), msg);
        } else {
            println!("ok: {msg}");
        }
    }

    pub(crate) fn warning(&self, msg: &str) {
        if self.color_enabled {
            eprintln!("{} {}", "!".yellow(), msg);
        } else {
            eprintln!("warn: {msg}");
        }
    }

    #[allow(dead_code, reason = "Driver B will use this for error messages")]
    pub(crate) fn error(&self, msg: &str) {
        if self.color_enabled {
            eprintln!("{} {}", "✗".red(), msg);
        } else {
            eprintln!("error: {msg}");
        }
    }

    pub(crate) fn dim(&self, text: &str) -> String {
        if self.color_enabled {
            text.style(Style::new().dimmed()).to_string()
        } else {
            text.to_owned()
        }
    }

    pub(crate) fn highlight(&self, text: &str) -> String {
        if self.color_enabled {
            text.style(Style::new().bold().cyan()).to_string()
        } else {
            text.to_owned()
        }
    }

    /// Render a `key: value` pair in pretty mode.
    pub(crate) fn kv(&self, key: &str, value: &str) {
        if self.color_enabled {
            println!(
                "  {} {}",
                format!("{key}:").style(Style::new().dimmed()),
                value
            );
        } else {
            println!("{key}: {value}");
        }
    }
}

/// Renderable types render twice — once for `pretty` (boxes, color) and
/// once for `plain` (`key: value`). JSON output is handled by serde.
pub(crate) trait Render {
    /// Multi-line, colored, human-friendly rendering.
    fn render_pretty(&self, r: &Renderer);
    /// Plain `key: value` rendering (no colors).
    fn render_plain(&self, w: &mut dyn Write) -> io::Result<()>;

    /// Hooks for table-row rendering. Default implementations defer to
    /// `render_pretty` (i.e. treat the whole value as one cell).
    fn render_pretty_table_header(&self, _r: &Renderer) {}
    fn render_pretty_table_row(&self, r: &Renderer) {
        self.render_pretty(r);
    }
    fn render_pretty_table_footer(&self, _r: &Renderer) {}
}
