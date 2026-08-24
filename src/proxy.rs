use std::ffi::OsString;
use std::path::PathBuf;

use anyhow::Result;

pub fn run(command: Vec<OsString>, bridge_socket: Option<PathBuf>, zsh: bool) -> Result<i32> {
    imp::run(command, bridge_socket, zsh)
}

#[cfg(not(unix))]
mod imp {
    use std::ffi::OsString;
    use std::path::PathBuf;

    use anyhow::{Result, bail};

    pub fn run(
        _command: Vec<OsString>,
        _bridge_socket: Option<PathBuf>,
        _zsh: bool,
    ) -> Result<i32> {
        bail!("agentdrop proxy is intended to run on the remote Unix/Linux host")
    }
}

#[cfg(unix)]
mod imp {
    use std::ffi::OsString;
    use std::fs;
    use std::io::{self, BufRead, BufReader, Read, Write};
    use std::os::unix::fs::FileTypeExt;
    use std::os::unix::net::UnixStream;
    use std::path::{Component, Path, PathBuf};
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::mpsc::{self, RecvTimeoutError};
    use std::thread;
    use std::time::Duration;

    use anyhow::{Context, Result, bail};
    use crossterm::terminal;
    use portable_pty::{CommandBuilder, PtySize, native_pty_system};

    use crate::paste::{BracketedPasteParser, InputEvent, write_bracketed_paste};
    use crate::protocol::{PROTOCOL_VERSION, UploadRequest, UploadResponse};

    const AMBIGUOUS_ESCAPE_TIMEOUT: Duration = Duration::from_millis(40);

    struct RawModeGuard;

    impl RawModeGuard {
        fn enable() -> Result<Self> {
            terminal::enable_raw_mode().context("failed to enable remote terminal raw mode")?;
            Ok(Self)
        }
    }

    impl Drop for RawModeGuard {
        fn drop(&mut self) {
            let _ = terminal::disable_raw_mode();
        }
    }

    #[derive(Debug)]
    struct LocalPathPaste {
        path: String,
        trailing_space: bool,
    }

    struct BridgeClient {
        explicit_socket: Option<PathBuf>,
        cached_socket: Option<PathBuf>,
    }

    impl BridgeClient {
        fn new(explicit_socket: Option<PathBuf>) -> Self {
            Self {
                explicit_socket,
                cached_socket: None,
            }
        }

        fn upload(&mut self, local_path: &str) -> Result<PathBuf> {
            let mut candidates = Vec::new();
            if let Some(path) = &self.explicit_socket {
                candidates.push(path.clone());
            } else if let Some(path) = std::env::var_os("AGENTDROP_BRIDGE_SOCKET") {
                candidates.push(PathBuf::from(path));
            } else {
                if let Some(path) = &self.cached_socket {
                    candidates.push(path.clone());
                }
                for path in discover_bridge_sockets()? {
                    if !candidates.contains(&path) {
                        candidates.push(path);
                    }
                }
            }

            if candidates.is_empty() {
                bail!("no agentdrop bridge socket found; connect with `agentdrop connect <host>`")
            }

            let mut last_error = None;
            for socket in candidates {
                match request_upload(&socket, local_path) {
                    Ok(relative) => {
                        self.cached_socket = Some(socket);
                        return resolve_remote_path(&relative);
                    }
                    Err(error) => last_error = Some(error),
                }
            }

            Err(last_error.unwrap_or_else(|| anyhow::anyhow!("no usable agentdrop bridge socket")))
        }
    }

    pub fn run(command: Vec<OsString>, bridge_socket: Option<PathBuf>, zsh: bool) -> Result<i32> {
        if command.is_empty() {
            bail!("proxy requires a command, for example: agentdrop proxy -- codex")
        }

        let (cols, rows) = terminal::size().unwrap_or((120, 30));
        let pty_system = native_pty_system();
        let pair = pty_system
            .openpty(PtySize {
                rows,
                cols,
                pixel_width: 0,
                pixel_height: 0,
            })
            .context("failed to create agent PTY")?;

        let child_argv = agent_argv(&command, zsh);
        let mut child_command = CommandBuilder::new(&child_argv[0]);
        for arg in &child_argv[1..] {
            child_command.arg(arg);
        }

        let mut child = pair
            .slave
            .spawn_command(child_command)
            .with_context(|| format!("failed to start {}", child_argv[0].to_string_lossy()))?;
        drop(pair.slave);

        let mut child_output = pair
            .master
            .try_clone_reader()
            .context("failed to open agent PTY reader")?;
        let child_input = pair
            .master
            .take_writer()
            .context("failed to open agent PTY writer")?;

        let _raw_mode = RawModeGuard::enable()?;

        let output_thread = thread::spawn(move || {
            let stdout = io::stdout();
            let mut stdout = stdout.lock();
            let _ = copy_interactive_output(&mut child_output, &mut stdout);
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
                thread::sleep(Duration::from_millis(100));
            }
        });

        spawn_input_proxy(child_input, BridgeClient::new(bridge_socket));

        let status = child.wait().context("failed waiting for agent command")?;
        stop_resize.store(true, Ordering::Relaxed);
        let _ = resize_thread.join();
        let _ = output_thread.join();

        Ok(status.exit_code() as i32)
    }

    fn agent_argv(command: &[OsString], zsh: bool) -> Vec<OsString> {
        if !zsh {
            return command.to_vec();
        }

        // `$@` is executed as the shell command after `.zshrc` is loaded. This preserves
        // zsh function resolution and environment setup without string-building or `eval`.
        let mut argv = vec![
            OsString::from("zsh"),
            OsString::from("-lic"),
            OsString::from("\"$@\""),
            OsString::from("agentdrop-proxy"),
        ];
        argv.extend(command.iter().cloned());
        argv
    }

    fn copy_interactive_output<R: Read, W: Write>(reader: &mut R, writer: &mut W) -> io::Result<()> {
        let mut buffer = [0_u8; 8192];
        loop {
            let read = reader.read(&mut buffer)?;
            if read == 0 {
                return Ok(());
            }
            writer.write_all(&buffer[..read])?;
            writer.flush()?;
        }
    }

    fn spawn_input_proxy(mut child_input: Box<dyn Write + Send>, mut bridge: BridgeClient) {
        thread::spawn(move || {
            let (sender, receiver) = mpsc::channel::<Option<Vec<u8>>>();

            // Keep the blocking terminal read in a dedicated thread. The parser thread can then
            // time out an ambiguous `ESC` / `ESC[` prefix without making stdin non-blocking or
            // changing tty file-status flags shared with the parent shell/tmux session.
            thread::spawn(move || {
                let stdin = io::stdin();
                let mut stdin = stdin.lock();
                let mut buffer = [0_u8; 4096];
                loop {
                    match stdin.read(&mut buffer) {
                        Ok(0) => {
                            let _ = sender.send(None);
                            return;
                        }
                        Ok(read) => {
                            if sender.send(Some(buffer[..read].to_vec())).is_err() {
                                return;
                            }
                        }
                        Err(_) => {
                            let _ = sender.send(None);
                            return;
                        }
                    }
                }
            });

            let mut parser = BracketedPasteParser::default();
            loop {
                match receiver.recv_timeout(AMBIGUOUS_ESCAPE_TIMEOUT) {
                    Ok(Some(bytes)) => {
                        if forward_events(
                            &mut child_input,
                            &mut bridge,
                            parser.feed(&bytes),
                        )
                        .is_err()
                        {
                            return;
                        }
                    }
                    Ok(None) | Err(RecvTimeoutError::Disconnected) => {
                        let _ = forward_events(
                            &mut child_input,
                            &mut bridge,
                            parser.finish(),
                        );
                        return;
                    }
                    Err(RecvTimeoutError::Timeout) => {
                        if forward_events(
                            &mut child_input,
                            &mut bridge,
                            parser.flush_ambiguous_prefix(),
                        )
                        .is_err()
                        {
                            return;
                        }
                    }
                }
            }
        });
    }

    fn forward_events<W: Write>(
        writer: &mut W,
        bridge: &mut BridgeClient,
        events: Vec<InputEvent>,
    ) -> io::Result<()> {
        for event in events {
            forward_event(writer, bridge, event)?;
        }
        Ok(())
    }

    fn forward_event<W: Write>(
        writer: &mut W,
        bridge: &mut BridgeClient,
        event: InputEvent,
    ) -> io::Result<()> {
        match event {
            InputEvent::Bytes(bytes) => {
                writer.write_all(&bytes)?;
                writer.flush()
            }
            InputEvent::Paste(payload) => {
                let Some(local_file) = local_path_from_paste(&payload) else {
                    return write_bracketed_paste(writer, &payload);
                };

                match bridge.upload(&local_file.path) {
                    Ok(remote_path) => {
                        let mut replacement = remote_path.to_string_lossy().into_owned();
                        if local_file.trailing_space {
                            replacement.push(' ');
                        }
                        write_bracketed_paste(writer, replacement.as_bytes())
                    }
                    Err(error) => {
                        let _ = io::stderr().write_all(
                            format!("\r\n[agentdrop] upload failed: {error:#}\r\n").as_bytes(),
                        );
                        let _ = io::stderr().flush();
                        write_bracketed_paste(writer, &payload)
                    }
                }
            }
        }
    }

    fn local_path_from_paste(payload: &[u8]) -> Option<LocalPathPaste> {
        let text = std::str::from_utf8(payload).ok()?;
        let text = text.trim_end_matches(['\r', '\n']);
        if text.is_empty() || text.contains('\r') || text.contains('\n') {
            return None;
        }

        // Alacritty appends one separator space after a dropped file path.
        let (text, trailing_space) = match text.strip_suffix(' ') {
            Some(path) => (path, true),
            None => (text, false),
        };
        let text = strip_matching_quotes(text);
        if text.is_empty() {
            return None;
        }

        let windows_absolute = is_windows_absolute_path(text);
        let unix_absolute = text.starts_with('/');
        if !windows_absolute && !unix_absolute {
            return None;
        }

        // Existing remote Unix paths are normal Agent input, not local drops. A macOS/Linux
        // local path is only bridged when that absolute path does not exist on the remote host.
        if !windows_absolute && Path::new(text).exists() {
            return None;
        }

        Some(LocalPathPaste {
            path: text.to_owned(),
            trailing_space,
        })
    }

    fn is_windows_absolute_path(value: &str) -> bool {
        let bytes = value.as_bytes();
        let drive = bytes.len() >= 3
            && bytes[0].is_ascii_alphabetic()
            && bytes[1] == b':'
            && matches!(bytes[2], b'\\' | b'/');
        let unc = value.starts_with("\\\\");
        drive || unc
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

    fn discover_bridge_sockets() -> Result<Vec<PathBuf>> {
        let mut candidates = Vec::new();
        for entry in fs::read_dir("/tmp").context("failed to scan /tmp for agentdrop bridge")? {
            let entry = match entry {
                Ok(entry) => entry,
                Err(_) => continue,
            };
            let name = entry.file_name();
            let Some(name) = name.to_str() else {
                continue;
            };
            if !name.starts_with("agentdrop-") || !name.ends_with(".sock") {
                continue;
            }
            let metadata = match entry.metadata() {
                Ok(metadata) => metadata,
                Err(_) => continue,
            };
            if !metadata.file_type().is_socket() {
                continue;
            }
            let modified = metadata.modified().ok();
            candidates.push((modified, entry.path()));
        }
        candidates.sort_by(|a, b| b.0.cmp(&a.0));
        Ok(candidates.into_iter().map(|(_, path)| path).collect())
    }

    fn request_upload(socket: &Path, local_path: &str) -> Result<String> {
        let mut stream = UnixStream::connect(socket)
            .with_context(|| format!("failed to connect bridge {}", socket.display()))?;
        stream.set_read_timeout(Some(Duration::from_secs(300)))?;
        stream.set_write_timeout(Some(Duration::from_secs(10)))?;

        let request = UploadRequest {
            version: PROTOCOL_VERSION,
            path: local_path.to_owned(),
        };
        serde_json::to_writer(&mut stream, &request)?;
        stream.write_all(b"\n")?;
        stream.flush()?;

        let mut line = String::new();
        let mut reader = BufReader::new(stream);
        if reader.read_line(&mut line)? == 0 {
            bail!("upload bridge closed without a response");
        }
        let response: UploadResponse = serde_json::from_str(line.trim_end())?;
        if response.version != PROTOCOL_VERSION {
            bail!("upload bridge protocol mismatch");
        }
        if let Some(error) = response.error {
            bail!("{error}");
        }
        response
            .remote_relative_path
            .context("upload bridge returned no remote path")
    }

    fn resolve_remote_path(relative: &str) -> Result<PathBuf> {
        let relative = Path::new(relative);
        if relative.is_absolute()
            || relative
                .components()
                .any(|component| matches!(component, Component::ParentDir))
        {
            bail!("bridge returned unsafe relative path: {}", relative.display());
        }
        let home = std::env::var_os("HOME").context("HOME is not set on remote host")?;
        let path = PathBuf::from(home).join(relative);
        Ok(path.canonicalize().unwrap_or(path))
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        #[test]
        fn zsh_mode_executes_agent_as_positional_command() {
            let command = vec![
                OsString::from("codex"),
                OsString::from("--model"),
                OsString::from("gpt-5"),
            ];
            let argv = agent_argv(&command, true);
            assert_eq!(
                argv,
                vec![
                    OsString::from("zsh"),
                    OsString::from("-lic"),
                    OsString::from("\"$@\""),
                    OsString::from("agentdrop-proxy"),
                    OsString::from("codex"),
                    OsString::from("--model"),
                    OsString::from("gpt-5"),
                ]
            );
        }

        #[test]
        fn direct_mode_keeps_original_argv() {
            let command = vec![OsString::from("codex"), OsString::from("resume")];
            assert_eq!(agent_argv(&command, false), command);
        }

        #[test]
        fn recognizes_windows_drive_and_unc_paths() {
            assert!(is_windows_absolute_path(r"C:\Users\me\shot.png"));
            assert!(is_windows_absolute_path("D:/images/shot.png"));
            assert!(is_windows_absolute_path(r"\\server\share\shot.png"));
            assert!(!is_windows_absolute_path("relative\\shot.png"));
        }

        #[test]
        fn recognizes_windows_drop_without_remote_filesystem_lookup() {
            let paste = local_path_from_paste(b"C:\\Users\\me\\shot.png ")
                .expect("Windows drop should be treated as a local path");
            assert_eq!(paste.path, r"C:\Users\me\shot.png");
            assert!(paste.trailing_space);
        }

        #[test]
        fn ignores_existing_remote_unix_path() {
            let path = std::env::temp_dir().join(format!(
                "agentdrop-remote-path-{}",
                uuid::Uuid::new_v4().simple()
            ));
            fs::write(&path, b"remote").expect("create remote path fixture");
            let payload = format!("{} ", path.display());
            assert!(local_path_from_paste(payload.as_bytes()).is_none());
            fs::remove_file(path).expect("remove remote path fixture");
        }
    }
}
