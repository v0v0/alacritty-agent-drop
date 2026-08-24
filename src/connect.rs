use std::ffi::OsString;
use std::fs;
use std::io::{BufRead, BufReader, Write};
use std::net::{TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread;
use std::time::Duration;

use anyhow::{Context, Result, bail};
use uuid::Uuid;

use crate::clipboard;
use crate::protocol::{BridgeRequest, BridgeResponse, PROTOCOL_VERSION};

#[derive(Clone)]
struct BridgeContext {
    tssh: OsString,
    destination: String,
    tssh_args: Vec<OsString>,
    session_id: String,
}

pub fn run(
    tssh: OsString,
    destination: String,
    tssh_args: Vec<OsString>,
) -> Result<i32> {
    let listener = TcpListener::bind(("127.0.0.1", 0)).context("failed to bind local upload bridge")?;
    listener
        .set_nonblocking(true)
        .context("failed to configure local upload bridge")?;
    let local_port = listener.local_addr()?.port();

    let session_id = Uuid::new_v4().simple().to_string();
    let remote_socket = format!("/tmp/agentdrop-{session_id}.sock");
    let reverse_forward = format!("{remote_socket}:127.0.0.1:{local_port}");

    let stop = Arc::new(AtomicBool::new(false));
    let bridge_stop = Arc::clone(&stop);
    let context = BridgeContext {
        tssh: tssh.clone(),
        destination: destination.clone(),
        tssh_args: tssh_args.clone(),
        session_id,
    };
    let bridge_thread = thread::spawn(move || run_bridge(listener, context, bridge_stop));

    // The interactive path is intentionally direct. agentdrop never reads or rewrites stdin/stdout
    // in connect mode; tssh owns the terminal exactly as if it had been launched by the user.
    let mut command = Command::new(&tssh);
    command.args(&tssh_args);
    // tssh's native drag handler sends Ctrl-C and starts `trz` in the foreground pane. Disable it:
    // the remote agent proxy handles dropped paths without interrupting the Agent TUI.
    command.arg("-o").arg("EnableDragFile=no");
    command.arg("-o").arg("StreamLocalBindUnlink=yes");
    command.arg("-o").arg("StreamLocalBindMask=0177");
    command.arg("-R").arg(&reverse_forward);
    command.arg(&destination);
    command.stdin(Stdio::inherit());
    command.stdout(Stdio::inherit());
    command.stderr(Stdio::inherit());

    let status = command
        .status()
        .with_context(|| format!("failed to start {}", tssh.to_string_lossy()))?;

    stop.store(true, Ordering::Relaxed);
    let _ = bridge_thread.join();

    Ok(status.code().unwrap_or(1))
}

fn run_bridge(listener: TcpListener, context: BridgeContext, stop: Arc<AtomicBool>) {
    let context = Arc::new(context);
    while !stop.load(Ordering::Relaxed) {
        match listener.accept() {
            Ok((stream, _)) => {
                let context = Arc::clone(&context);
                thread::spawn(move || {
                    let _ = handle_bridge_request(stream, &context);
                });
            }
            Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                thread::sleep(Duration::from_millis(40));
            }
            Err(_) => return,
        }
    }
}

fn handle_bridge_request(mut stream: TcpStream, context: &BridgeContext) -> Result<()> {
    stream.set_read_timeout(Some(Duration::from_secs(10)))?;
    stream.set_write_timeout(Some(Duration::from_secs(10)))?;

    let request = {
        let mut line = String::new();
        let mut reader = BufReader::new(stream.try_clone()?);
        if reader.read_line(&mut line)? == 0 {
            bail!("empty bridge request");
        }
        if line.len() > 64 * 1024 {
            bail!("bridge request is too large");
        }
        serde_json::from_str::<BridgeRequest>(line.trim_end()).context("invalid bridge request")?
    };

    let response = if request.version() != PROTOCOL_VERSION {
        BridgeResponse::failure(format!(
            "protocol version mismatch: client={}, bridge={PROTOCOL_VERSION}",
            request.version()
        ))
    } else {
        match request {
            BridgeRequest::UploadPath { path, .. } => match upload_local_file(context, Path::new(&path)) {
                Ok(path) => BridgeResponse::success(path),
                Err(error) => BridgeResponse::failure(format!("{error:#}")),
            },
            BridgeRequest::ClipboardImage { .. } => handle_clipboard_image_request(context),
        }
    };

    serde_json::to_writer(&mut stream, &response)?;
    stream.write_all(b"\n")?;
    stream.flush()?;
    Ok(())
}

fn handle_clipboard_image_request(context: &BridgeContext) -> BridgeResponse {
    let temp_path = match clipboard::capture_image_to_temp() {
        Ok(Some(path)) => path,
        Ok(None) => return BridgeResponse::no_clipboard_image(),
        Err(error) => return BridgeResponse::failure(format!("{error:#}")),
    };

    let result = upload_local_file(context, &temp_path);
    let _ = fs::remove_file(&temp_path);

    match result {
        Ok(path) => BridgeResponse::success(path),
        Err(error) => BridgeResponse::failure(format!("{error:#}")),
    }
}

fn upload_local_file(context: &BridgeContext, local_path: &Path) -> Result<String> {
    if !local_path.is_absolute() {
        bail!("local path is not absolute: {}", local_path.display());
    }
    if !local_path.is_file() {
        bail!("local file does not exist: {}", local_path.display());
    }

    let canonical = local_path
        .canonicalize()
        .with_context(|| format!("failed to resolve {}", local_path.display()))?;
    let file_name = canonical
        .file_name()
        .and_then(|name| name.to_str())
        .context("file name is not valid UTF-8")?;

    let request_id = Uuid::new_v4().simple().to_string();
    let remote_relative_dir = format!(
        ".cache/agentdrop/files/{}/{}",
        context.session_id, request_id
    );
    let remote_command = format!(
        "umask 077; d=\"$HOME/{remote_relative_dir}\"; mkdir -p -- \"$d\" && exec trz -y -q \"$d/\""
    );

    let mut command = Command::new(&context.tssh);
    command.args(&context.tssh_args);
    command.arg("-o").arg("EnableDragFile=no");
    command.arg("--upload-file").arg(&canonical);
    command.arg(&context.destination);
    command.arg(&remote_command);
    command.stdin(Stdio::null());

    let output = command.output().with_context(|| {
        format!(
            "failed to start {} for side-channel upload",
            context.tssh.to_string_lossy()
        )
    })?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        let stdout = String::from_utf8_lossy(&output.stdout);
        bail!(
            "tssh upload failed with {}: {}{}",
            output.status,
            stderr.trim(),
            if stdout.trim().is_empty() {
                String::new()
            } else {
                format!(" ({})", stdout.trim())
            }
        );
    }

    Ok(format!("{remote_relative_dir}/{file_name}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reverse_forward_uses_remote_unix_socket() {
        let session = "0123456789abcdef";
        let socket = format!("/tmp/agentdrop-{session}.sock");
        let spec = format!("{socket}:127.0.0.1:{}", 43210);
        assert_eq!(
            spec,
            "/tmp/agentdrop-0123456789abcdef.sock:127.0.0.1:43210"
        );
    }

    #[test]
    fn remote_upload_directory_contains_no_shell_metacharacters() {
        let session = Uuid::new_v4().simple().to_string();
        let request = Uuid::new_v4().simple().to_string();
        let path = PathBuf::from(format!(".cache/agentdrop/files/{session}/{request}"));
        let text = path.to_string_lossy();
        assert!(text.bytes().all(|b| b.is_ascii_alphanumeric() || b"./_-".contains(&b)));
    }
}
