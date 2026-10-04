use std::ffi::OsString;
use std::fs::{self, File};
use std::io::{BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::thread;
use std::time::Duration;

use anyhow::{Context, Result, bail};
#[cfg(test)]
use uuid::Uuid;

use crate::clipboard;
use crate::protocol::{
    self, BridgeRequest, BridgeResponse, MAX_FILE_SIZE, Operation, Outcome, PROTOCOL_VERSION,
};
use crate::session::{Endpoint, shell_quote};

#[derive(Clone)]
struct BridgeContext {
    token: String,
    allow_roots: Vec<PathBuf>,
    clipboard: bool,
}

pub struct Options {
    pub tssh: OsString,
    pub destination: String,
    pub tssh_args: Vec<OsString>,
    pub allow_roots: Vec<PathBuf>,
    pub clipboard: bool,
    pub remote_bin: String,
    pub command: Vec<OsString>,
    pub zsh: bool,
    pub tmux: Option<String>,
}

pub fn run(options: Options) -> Result<i32> {
    if options.destination.starts_with('-') {
        bail!("destination cannot start with '-'");
    }
    let allow_roots = options
        .allow_roots
        .iter()
        .map(|p| {
            let root = p
                .canonicalize()
                .with_context(|| format!("invalid allow-root {}", p.display()))?;
            if !root.is_dir() {
                bail!("allow-root must be a directory");
            }
            Ok(root)
        })
        .collect::<Result<Vec<_>>>()?;
    let listener = TcpListener::bind(("127.0.0.1", 0)).context("failed to bind local bridge")?;
    listener.set_nonblocking(true)?;
    let local_port = listener.local_addr()?.port();
    let endpoint = Endpoint::fresh();
    let remote_command = remote_command(&options, &endpoint)?;
    let stop = Arc::new(AtomicBool::new(false));
    let bridge_stop = Arc::clone(&stop);
    let context = BridgeContext {
        token: endpoint.token.clone(),
        allow_roots,
        clipboard: options.clipboard,
    };
    let bridge_thread = thread::spawn(move || run_bridge(listener, context, bridge_stop));

    // No local PTY, raw mode or stdin interception. Only the SSH client owns the terminal.
    let mut command = Command::new(&options.tssh);
    let client_name = Path::new(&options.tssh)
        .file_stem()
        .and_then(|s| s.to_str());
    if client_name != Some("ssh") {
        command.args(["-o", "EnableDragFile=no"]);
    }
    command.args([
        "-o",
        "StreamLocalBindUnlink=yes",
        "-o",
        "StreamLocalBindMask=0177",
        "-o",
        "ExitOnForwardFailure=yes",
    ]);
    command.args(&options.tssh_args);
    command.arg("-tt").arg("-R").arg(format!(
        "{}:127.0.0.1:{local_port}",
        endpoint.socket.display()
    ));
    command.arg(&options.destination).arg(remote_command);
    command
        .stdin(Stdio::inherit())
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit());
    let result = command
        .status()
        .with_context(|| format!("failed to start {}", options.tssh.to_string_lossy()));
    stop.store(true, Ordering::Relaxed);
    let _ = bridge_thread.join();
    Ok(result?.code().unwrap_or(1))
}

fn remote_command(options: &Options, endpoint: &Endpoint) -> Result<String> {
    let launch = if options.command.is_empty() {
        "exec \"${SHELL:-/bin/sh}\" -l".to_owned()
    } else {
        let mut args = vec![options.remote_bin.clone()];
        if let Some(session) = &options.tmux {
            crate::session::validate_session(session)?;
            args.extend(["attach".into(), "--session".into(), session.clone()]);
        } else {
            args.push("proxy".into());
        }
        if options.zsh {
            args.push("--zsh".into());
        }
        args.push("--".into());
        for arg in &options.command {
            args.push(
                arg.to_str()
                    .context("remote command arguments must be UTF-8")?
                    .to_owned(),
            );
        }
        format!(
            "exec {}",
            args.iter()
                .map(|s| shell_quote(s))
                .collect::<Vec<_>>()
                .join(" ")
        )
    };
    // Always use a known POSIX shell even if the account login shell is fish.
    let script = format!(
        "export AGENTDROP_BRIDGE={}; export PATH=\"$HOME/.cargo/bin:$HOME/.local/bin:$PATH\"; {launch}",
        shell_quote(&endpoint.encode())
    );
    Ok(format!("sh -c {}", shell_quote(&script)))
}

fn run_bridge(listener: TcpListener, context: BridgeContext, stop: Arc<AtomicBool>) {
    let context = Arc::new(context);
    let active = Arc::new(AtomicUsize::new(0));
    while !stop.load(Ordering::Relaxed) {
        match listener.accept() {
            Ok((stream, _)) => {
                if active.load(Ordering::Relaxed) >= 8 {
                    drop(stream);
                    continue;
                }
                active.fetch_add(1, Ordering::Relaxed);
                let context = Arc::clone(&context);
                let active = Arc::clone(&active);
                thread::spawn(move || {
                    let _ = handle_bridge_request(stream, &context);
                    active.fetch_sub(1, Ordering::Relaxed);
                });
            }
            Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                thread::sleep(Duration::from_millis(40))
            }
            Err(_) => return,
        }
    }
}

fn handle_bridge_request(mut stream: TcpStream, context: &BridgeContext) -> Result<()> {
    stream.set_read_timeout(Some(Duration::from_secs(10)))?;
    stream.set_write_timeout(Some(Duration::from_secs(30)))?;
    let request: BridgeRequest = protocol::read_json(&mut BufReader::new(stream.try_clone()?))?;
    // Authenticate before touching filesystem or clipboard.
    let prepared = if request.version != PROTOCOL_VERSION {
        Err(anyhow::anyhow!(
            "protocol mismatch; install the same v2 version on both hosts"
        ))
    } else if request.token != context.token {
        Err(anyhow::anyhow!("bridge authentication failed"))
    } else {
        prepare_file(context, request.operation)
    };
    match prepared {
        Ok(Some(mut source)) => {
            protocol::write_json(
                &mut stream,
                &BridgeResponse::new(Outcome::File {
                    name: source.name.clone(),
                    size: source.size,
                }),
            )?;
            let copied = std::io::copy(&mut (&mut source.file).take(source.size), &mut stream)?;
            if copied != source.size {
                bail!("local file changed during transfer");
            }
            stream.flush()?;
        }
        Ok(None) => {
            protocol::write_json(&mut stream, &BridgeResponse::new(Outcome::NoClipboardImage))?
        }
        Err(error) => protocol::write_json(
            &mut stream,
            &BridgeResponse::new(Outcome::Error {
                message: format!("{error:#}"),
            }),
        )?,
    }
    Ok(())
}

struct Source {
    file: File,
    name: String,
    size: u64,
    temporary: Option<PathBuf>,
}
impl Drop for Source {
    fn drop(&mut self) {
        // On Windows an open file cannot be deleted. The clipboard capture is read into a
        // separately opened handle with FILE_SHARE_DELETE (Rust default); drop is best-effort.
        if let Some(path) = &self.temporary {
            let _ = fs::remove_file(path);
        }
    }
}

fn prepare_file(context: &BridgeContext, operation: Operation) -> Result<Option<Source>> {
    match operation {
        Operation::UploadPath { path } => {
            Ok(Some(open_source(Path::new(&path), &context.allow_roots)?))
        }
        Operation::ClipboardImage => {
            if !context.clipboard {
                bail!("clipboard sharing is disabled on this connection");
            }
            let Some(path) = clipboard::capture_image_to_temp()? else {
                return Ok(None);
            };
            match open_source(&path, &[]) {
                Ok(mut source) => {
                    source.temporary = Some(path);
                    Ok(Some(source))
                }
                Err(error) => {
                    let _ = fs::remove_file(path);
                    Err(error)
                }
            }
        }
    }
}

fn open_source(path: &Path, allow_roots: &[PathBuf]) -> Result<Source> {
    if !path.is_absolute() {
        bail!("local path must be absolute");
    }
    let canonical = path.canonicalize().context("local file does not exist")?;
    if !allow_roots.is_empty() && !allow_roots.iter().any(|root| canonical.starts_with(root)) {
        bail!("local file is outside the configured allow-roots");
    }
    // Check before open to avoid blocking on FIFO/device paths; recheck the opened descriptor.
    if !canonical.is_file() {
        bail!("only regular files are supported");
    }
    let file = File::open(&canonical).context("failed to open local file")?;
    let metadata = file.metadata()?;
    if !metadata.is_file() {
        bail!("only regular files are supported");
    }
    if metadata.len() > MAX_FILE_SIZE {
        bail!("file exceeds 256 MiB limit");
    }
    let name = canonical
        .file_name()
        .and_then(|s| s.to_str())
        .context("filename must be UTF-8")?
        .to_owned();
    crate::transfer::validate_name(&name)?;
    Ok(Source {
        file,
        name,
        size: metadata.len(),
        temporary: None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Read;

    #[test]
    fn authenticated_bridge_streams_file_and_rejects_wrong_token() {
        let root = std::env::temp_dir().join(format!("agentdrop-test-{}", Uuid::new_v4()));
        fs::create_dir(&root).unwrap();
        let path = root.join("中文 ' a.png");
        fs::write(&path, b"\0binary\xff\n").unwrap();
        for valid in [true, false] {
            let listener = TcpListener::bind("127.0.0.1:0").unwrap();
            let addr = listener.local_addr().unwrap();
            let context = BridgeContext {
                token: "secret".into(),
                allow_roots: vec![root.canonicalize().unwrap()],
                clipboard: false,
            };
            let server = thread::spawn(move || {
                handle_bridge_request(listener.accept().unwrap().0, &context).unwrap()
            });
            let mut client = TcpStream::connect(addr).unwrap();
            protocol::write_json(
                &mut client,
                &BridgeRequest::new(
                    if valid { "secret" } else { "wrong" }.into(),
                    Operation::UploadPath {
                        path: path.to_str().unwrap().into(),
                    },
                ),
            )
            .unwrap();
            let mut reader = BufReader::new(client);
            let response: BridgeResponse = protocol::read_json(&mut reader).unwrap();
            let mut bytes = Vec::new();
            reader.read_to_end(&mut bytes).unwrap();
            if valid {
                assert_eq!(bytes, b"\0binary\xff\n");
                assert!(matches!(response.outcome, Outcome::File { .. }));
            } else {
                assert!(bytes.is_empty());
                assert!(matches!(response.outcome, Outcome::Error { .. }));
            }
            server.join().unwrap();
        }
        assert!(open_source(&path, &[root.join("other")]).is_err());
        fs::remove_dir_all(root).unwrap();
    }
}
