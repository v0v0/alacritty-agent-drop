mod paste;
mod uploader;

use std::ffi::OsString;
use std::io::{self, Read, Write};
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread;
use std::time::Duration;

use anyhow::{Context, Result};
use clap::Parser;
use crossterm::terminal;
use paste::{BracketedPasteParser, InputEvent, write_bracketed_paste};
use portable_pty::{CommandBuilder, PtySize, native_pty_system};
use uploader::Uploader;

#[derive(Debug, Parser)]
#[command(
    name = "agentdrop",
    version,
    about = "Make Alacritty file drops usable inside remote agent CLIs over tssh"
)]
struct Cli {
    /// tssh destination, e.g. dev or user@example.com
    destination: String,

    /// Path or command name of the local tssh executable
    #[arg(long, default_value = "tssh")]
    tssh: OsString,

    /// Extra tssh options. Put them after `--`; they are inserted before the destination.
    #[arg(last = true, allow_hyphen_values = true)]
    tssh_args: Vec<OsString>,
}

struct RawModeGuard;

impl RawModeGuard {
    fn enable() -> Result<Self> {
        terminal::enable_raw_mode().context("failed to enable terminal raw mode")?;
        Ok(Self)
    }
}

impl Drop for RawModeGuard {
    fn drop(&mut self) {
        let _ = terminal::disable_raw_mode();
    }
}

struct LocalFilePaste {
    path: PathBuf,
    trailing_space: bool,
}

fn main() {
    let exit_code = match run() {
        Ok(code) => code,
        Err(error) => {
            eprintln!("agentdrop: {error:#}");
            1
        }
    };
    std::process::exit(exit_code);
}

fn run() -> Result<i32> {
    let cli = Cli::parse();
    let (cols, rows) = terminal::size().unwrap_or((120, 30));
    let pty_system = native_pty_system();
    let pair = pty_system
        .openpty(PtySize {
            rows,
            cols,
            pixel_width: 0,
            pixel_height: 0,
        })
        .context("failed to create child PTY")?;

    let mut command = CommandBuilder::new(&cli.tssh);
    for arg in &cli.tssh_args {
        command.arg(arg);
    }
    command.arg(&cli.destination);

    let mut child = pair
        .slave
        .spawn_command(command)
        .with_context(|| format!("failed to start {}", cli.tssh.to_string_lossy()))?;
    drop(pair.slave);

    let mut child_output = pair
        .master
        .try_clone_reader()
        .context("failed to open child PTY reader")?;
    let child_input = pair
        .master
        .take_writer()
        .context("failed to open child PTY writer")?;

    let _raw_mode = RawModeGuard::enable()?;

    let output_thread = thread::spawn(move || {
        let stdout = io::stdout();
        let mut stdout = stdout.lock();
        let _ = io::copy(&mut child_output, &mut stdout);
        let _ = stdout.flush();
    });

    let stop_resize = Arc::new(AtomicBool::new(false));
    let resize_flag = Arc::clone(&stop_resize);
    let master = pair.master;
    let resize_thread = thread::spawn(move || {
        let mut last_size = (cols, rows);
        while !resize_flag.load(Ordering::Relaxed) {
            if let Ok((new_cols, new_rows)) = terminal::size()
                && (new_cols, new_rows) != last_size
            {
                let _ = master.resize(PtySize {
                    rows: new_rows,
                    cols: new_cols,
                    pixel_width: 0,
                    pixel_height: 0,
                });
                last_size = (new_cols, new_rows);
            }
            thread::sleep(Duration::from_millis(150));
        }
    });

    let uploader = Uploader::new(
        cli.tssh.clone(),
        cli.destination.clone(),
        cli.tssh_args.clone(),
    );
    spawn_input_proxy(child_input, uploader);

    let status = child.wait().context("failed waiting for tssh")?;
    stop_resize.store(true, Ordering::Relaxed);
    let _ = resize_thread.join();
    let _ = output_thread.join();

    Ok(status.exit_code() as i32)
}

fn spawn_input_proxy(mut child_input: Box<dyn Write + Send>, mut uploader: Uploader) {
    thread::spawn(move || {
        let stdin = io::stdin();
        let mut stdin = stdin.lock();
        let mut parser = BracketedPasteParser::default();
        let mut buffer = [0_u8; 4096];

        loop {
            let read = match stdin.read(&mut buffer) {
                Ok(0) => {
                    for event in parser.finish() {
                        if forward_event(&mut child_input, &mut uploader, event).is_err() {
                            return;
                        }
                    }
                    return;
                }
                Ok(read) => read,
                Err(_) => return,
            };

            for event in parser.feed(&buffer[..read]) {
                if forward_event(&mut child_input, &mut uploader, event).is_err() {
                    return;
                }
            }
        }
    });
}

fn forward_event<W: Write>(writer: &mut W, uploader: &mut Uploader, event: InputEvent) -> io::Result<()> {
    match event {
        InputEvent::Bytes(bytes) => {
            writer.write_all(&bytes)?;
            writer.flush()
        }
        InputEvent::Paste(payload) => {
            let Some(local_file) = local_file_from_paste(&payload) else {
                return write_bracketed_paste(writer, &payload);
            };

            match uploader.upload(&local_file.path) {
                Ok(remote_path) => {
                    let replacement = if local_file.trailing_space {
                        format!("{remote_path} ")
                    } else {
                        remote_path
                    };
                    write_bracketed_paste(writer, replacement.as_bytes())
                }
                Err(error) => {
                    let _ = io::stderr().write_all(
                        format!("\r\n[agentdrop] upload failed: {error:#}\r\n").as_bytes(),
                    );
                    write_bracketed_paste(writer, &payload)
                }
            }
        }
    }
}

fn local_file_from_paste(payload: &[u8]) -> Option<LocalFilePaste> {
    let text = std::str::from_utf8(payload).ok()?;
    let text = text.trim_end_matches(['\r', '\n']);
    if text.is_empty() || text.contains('\r') || text.contains('\n') {
        return None;
    }

    // Alacritty appends one space to every DroppedFile path before paste on all platforms.
    let (text, trailing_space) = match text.strip_suffix(' ') {
        Some(path) => (path, true),
        None => (text, false),
    };
    let text = strip_matching_quotes(text);
    if text.is_empty() {
        return None;
    }

    let path = PathBuf::from(text);
    if path.is_absolute() && path.is_file() {
        Some(LocalFilePaste {
            path,
            trailing_space,
        })
    } else {
        None
    }
}

fn strip_matching_quotes(value: &str) -> &str {
    if value.len() >= 2 {
        let first = value.as_bytes()[0];
        let last = value.as_bytes()[value.len() - 1];
        if (first == b'\'' && last == b'\'') || (first == b'"' && last == b'"') {
            return &value[1..value.len() - 1];
        }
    }
    value
}

#[cfg(test)]
mod tests {
    use std::fs;

    use uuid::Uuid;

    use super::{local_file_from_paste, strip_matching_quotes};

    #[test]
    fn strips_shell_style_wrapping_quotes() {
        assert_eq!(strip_matching_quotes("'C:\\a b\\x.png'"), "C:\\a b\\x.png");
        assert_eq!(strip_matching_quotes("\"C:\\a b\\x.png\""), "C:\\a b\\x.png");
        assert_eq!(strip_matching_quotes("C:\\a\\x.png"), "C:\\a\\x.png");
        assert_eq!(strip_matching_quotes("'/Users/me/a b/x.png'"), "/Users/me/a b/x.png");
    }

    #[test]
    fn recognizes_native_absolute_file_path_with_alacritty_separator() {
        let dir = std::env::temp_dir().join(format!(
            "agentdrop-test-{} dir",
            Uuid::new_v4().simple()
        ));
        fs::create_dir_all(&dir).expect("create temporary test directory");
        let path = dir.join("shot image.png");
        fs::write(&path, b"test image placeholder").expect("create temporary test file");

        let payload = format!("{} ", path.to_string_lossy());
        let parsed = local_file_from_paste(payload.as_bytes())
            .expect("native absolute path should be recognized");

        assert_eq!(parsed.path, path);
        assert!(parsed.trailing_space);

        fs::remove_dir_all(dir).expect("remove temporary test directory");
    }
}
