use std::ffi::OsString;

use anyhow::Result;

pub fn run(command: Vec<OsString>, bridge_socket: Option<String>, zsh: bool) -> Result<i32> {
    imp::run(command, bridge_socket, zsh)
}

#[cfg(not(unix))]
mod imp {
    use std::ffi::OsString;

    use anyhow::{Result, bail};

    pub fn run(_command: Vec<OsString>, _bridge_socket: Option<String>, _zsh: bool) -> Result<i32> {
        bail!("agentdrop proxy is intended to run on the remote Unix/Linux host")
    }
}

#[cfg(unix)]
mod imp {
    use std::ffi::OsString;
    use std::fs;
    use std::io::{self, BufReader, Read, Write};
    use std::os::unix::net::UnixStream;
    use std::path::{Path, PathBuf};
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::mpsc::{self, RecvTimeoutError};
    use std::thread;
    use std::time::Duration;

    use anyhow::{Context, Result, bail};
    use crossterm::terminal;
    use portable_pty::{CommandBuilder, PtySize, native_pty_system};

    use crate::paste::{BracketedPasteParser, InputEvent, write_bracketed_paste};
    use crate::protocol::{
        self, BridgeRequest, BridgeResponse, Operation, Outcome, PROTOCOL_VERSION,
    };
    use crate::session::{current_endpoint, shell_quote};

    const AMBIGUOUS_ESCAPE_TIMEOUT: Duration = Duration::from_millis(40);
    const CTRL_V: u8 = 0x16;

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
        paths: Vec<String>,
        trailing_space: bool,
    }

    struct BridgeClient {
        explicit_endpoint: Option<String>,
    }

    impl BridgeClient {
        fn new(explicit_endpoint: Option<String>) -> Self {
            Self { explicit_endpoint }
        }

        fn upload(&mut self, local_path: &str) -> Result<PathBuf> {
            self.request(Operation::UploadPath {
                path: local_path.into(),
            })?
            .context("bridge returned no file")
        }

        fn clipboard_image(&mut self) -> Result<Option<PathBuf>> {
            self.request(Operation::ClipboardImage)
        }

        fn request(&self, operation: Operation) -> Result<Option<PathBuf>> {
            let endpoint = current_endpoint(self.explicit_endpoint.as_deref())?;
            let mut stream = UnixStream::connect(&endpoint.socket)
                .context("bridge disconnected; reconnect with agentdrop")?;
            stream.set_read_timeout(Some(Duration::from_secs(30)))?;
            stream.set_write_timeout(Some(Duration::from_secs(10)))?;
            protocol::write_json(&mut stream, &BridgeRequest::new(endpoint.token, operation))?;
            let mut reader = BufReader::new(stream);
            let response: BridgeResponse = protocol::read_json(&mut reader)?;
            if response.version != PROTOCOL_VERSION {
                bail!("protocol mismatch; install the same v2 version on both hosts");
            }
            match response.outcome {
                Outcome::NoClipboardImage => Ok(None),
                Outcome::Error { message } => bail!("{message}"),
                Outcome::File { name, size } => {
                    let home = std::env::var_os("HOME").context("HOME is not set")?;
                    let cache = PathBuf::from(home).join(".cache");
                    fs::create_dir_all(&cache)?;
                    let base = cache.join("agentdrop");
                    Ok(Some(crate::transfer::receive(
                        &mut reader,
                        &base,
                        &name,
                        size,
                    )?))
                }
            }
        }
    }

    pub fn run(command: Vec<OsString>, bridge_socket: Option<String>, zsh: bool) -> Result<i32> {
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

    fn copy_interactive_output<R: Read, W: Write>(
        reader: &mut R,
        writer: &mut W,
    ) -> io::Result<()> {
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
            let (sender, receiver) = mpsc::sync_channel::<Option<Vec<u8>>>(64);

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
                        if forward_events(&mut child_input, &mut bridge, parser.feed(&bytes))
                            .is_err()
                        {
                            return;
                        }
                    }
                    Ok(None) | Err(RecvTimeoutError::Disconnected) => {
                        let _ = forward_events(&mut child_input, &mut bridge, parser.finish());
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
            InputEvent::Bytes(bytes) => forward_raw_bytes(writer, bridge, &bytes),
            InputEvent::Literal(bytes) => {
                writer.write_all(&bytes)?;
                writer.flush()
            }
            InputEvent::Paste(payload) => {
                let Some(local_file) = local_path_from_paste(&payload) else {
                    return write_bracketed_paste(writer, &payload);
                };

                let uploaded = local_file
                    .paths
                    .iter()
                    .map(|path| {
                        if !crate::paths::is_windows(path) && Path::new(path).exists() {
                            Ok(PathBuf::from(path))
                        } else {
                            bridge.upload(path)
                        }
                    })
                    .collect::<Result<Vec<_>>>();
                match uploaded {
                    Ok(remote_paths) => {
                        let mut replacement = remote_paths
                            .iter()
                            .map(|p| shell_quote(&p.to_string_lossy()))
                            .collect::<Vec<_>>()
                            .join(" ");
                        if local_file.trailing_space {
                            replacement.push(' ');
                        }
                        write_bracketed_paste(writer, replacement.as_bytes())
                    }
                    Err(error) => {
                        show_bridge_error("upload failed", &error);
                        write_bracketed_paste(writer, &payload)
                    }
                }
            }
        }
    }

    fn forward_raw_bytes<W: Write>(
        writer: &mut W,
        bridge: &mut BridgeClient,
        bytes: &[u8],
    ) -> io::Result<()> {
        let mut start = 0;
        for (index, byte) in bytes.iter().enumerate() {
            if *byte != CTRL_V {
                continue;
            }

            if start < index {
                writer.write_all(&bytes[start..index])?;
                writer.flush()?;
            }

            match bridge.clipboard_image() {
                Ok(Some(remote_path)) => {
                    let replacement = format!("{} ", shell_quote(&remote_path.to_string_lossy()));
                    write_bracketed_paste(writer, replacement.as_bytes())?;
                }
                Ok(None) => {
                    // There is no image in the local clipboard. Preserve the Agent's original
                    // Ctrl-V behavior instead of stealing the key.
                    writer.write_all(&[CTRL_V])?;
                    writer.flush()?;
                }
                Err(error) => {
                    show_bridge_error("clipboard image paste failed", &error);
                    writer.write_all(&[CTRL_V])?;
                    writer.flush()?;
                }
            }

            start = index + 1;
        }

        if start < bytes.len() {
            writer.write_all(&bytes[start..])?;
            writer.flush()?;
        }
        Ok(())
    }

    fn show_bridge_error(context: &str, error: &anyhow::Error) {
        let _ =
            io::stderr().write_all(format!("\r\n[agentdrop] {context}: {error:#}\r\n").as_bytes());
        let _ = io::stderr().flush();
    }

    fn local_path_from_paste(payload: &[u8]) -> Option<LocalPathPaste> {
        let (paths, trailing_space) = crate::paths::parse(payload)?;
        if paths
            .iter()
            .all(|p| !crate::paths::is_windows(p) && Path::new(p).exists())
        {
            return None;
        }
        Some(LocalPathPaste {
            paths,
            trailing_space,
        })
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
            assert!(crate::paths::is_windows(r"C:\Users\me\shot.png"));
            assert!(crate::paths::is_windows("D:/images/shot.png"));
            assert!(crate::paths::is_windows(r"\\server\share\shot.png"));
            assert!(!crate::paths::is_windows("relative\\shot.png"));
        }

        #[test]
        fn recognizes_windows_drop_without_remote_filesystem_lookup() {
            let paste = local_path_from_paste(b"C:\\Users\\me\\shot.png ")
                .expect("Windows drop should be treated as a local path");
            assert_eq!(paste.paths, [r"C:\Users\me\shot.png"]);
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

        #[derive(Default)]
        struct RecordingWriter(Vec<u8>);

        impl Write for RecordingWriter {
            fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
                self.0.extend_from_slice(buf);
                Ok(buf.len())
            }

            fn flush(&mut self) -> io::Result<()> {
                Ok(())
            }
        }

        #[test]
        fn bytes_without_ctrl_v_are_unchanged() {
            // This test protects the common terminal path: the Ctrl-V enhancement must not
            // rewrite ordinary keys, control bytes, or escape sequences.
            let bytes = b"abc\x01\x05\x12\x1b[A";
            assert!(!bytes.contains(&CTRL_V));
            let mut writer = RecordingWriter::default();
            forward_raw_bytes(&mut writer, &mut BridgeClient::new(None), bytes)
                .expect("forward keys");
            assert_eq!(writer.0, bytes);
        }
    }
}
