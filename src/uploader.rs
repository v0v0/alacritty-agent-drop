use std::ffi::{OsStr, OsString};
use std::path::Path;
use std::process::{Command, Stdio};

use anyhow::{Context, Result, bail};
use uuid::Uuid;

pub struct Uploader {
    tssh: OsString,
    destination: String,
    tssh_args: Vec<OsString>,
    session_id: String,
    remote_dir: Option<String>,
}

impl Uploader {
    pub fn new(tssh: OsString, destination: String, tssh_args: Vec<OsString>) -> Self {
        Self {
            tssh,
            destination,
            tssh_args,
            session_id: Uuid::new_v4().simple().to_string(),
            remote_dir: None,
        }
    }

    pub fn upload(&mut self, local_path: &Path) -> Result<String> {
        if !local_path.is_file() {
            bail!("only regular files are supported: {}", local_path.display());
        }

        let file_name = local_path
            .file_name()
            .and_then(OsStr::to_str)
            .context("local file name is not valid Unicode")?;
        let remote_dir = self.ensure_remote_dir()?.to_owned();
        let remote_command = format!("trz -q -y {}", shell_quote(&remote_dir));

        let output = self
            .base_command()
            .arg("--upload-file")
            .arg(local_path)
            .arg(&self.destination)
            .arg(remote_command)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .output()
            .with_context(|| format!("failed to launch {}", self.tssh.to_string_lossy()))?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            bail!(
                "tssh upload failed with {}: {}",
                output.status,
                stderr.trim()
            );
        }

        Ok(format!("{remote_dir}/{file_name}"))
    }

    fn ensure_remote_dir(&mut self) -> Result<&str> {
        if self.remote_dir.is_none() {
            let remote_command = format!(
                "umask 077; d=\"$HOME/.cache/agentdrop/{}\"; mkdir -p -- \"$d\" && cd -- \"$d\" && pwd -P",
                self.session_id
            );

            let output = self
                .base_command()
                .arg(&self.destination)
                .arg(remote_command)
                .stdin(Stdio::null())
                .stdout(Stdio::piped())
                .stderr(Stdio::piped())
                .output()
                .with_context(|| format!("failed to launch {}", self.tssh.to_string_lossy()))?;

            if !output.status.success() {
                let stderr = String::from_utf8_lossy(&output.stderr);
                bail!(
                    "failed to create remote drop directory with {}: {}",
                    output.status,
                    stderr.trim()
                );
            }

            let stdout = String::from_utf8(output.stdout)
                .context("remote directory response was not valid UTF-8")?;
            let remote_dir = stdout
                .lines()
                .rev()
                .map(str::trim)
                .find(|line| line.starts_with('/'))
                .context("tssh did not return an absolute remote directory")?;

            self.remote_dir = Some(remote_dir.to_owned());
        }

        Ok(self.remote_dir.as_deref().expect("remote_dir initialized"))
    }

    fn base_command(&self) -> Command {
        let mut command = Command::new(&self.tssh);
        command.args(&self.tssh_args);
        command
    }
}

fn shell_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\"'\"'"))
}

#[cfg(test)]
mod tests {
    use super::shell_quote;

    #[test]
    fn quotes_posix_shell_arguments() {
        assert_eq!(shell_quote("/tmp/a b"), "'/tmp/a b'");
        assert_eq!(shell_quote("/tmp/a'b"), "'/tmp/a'\"'\"'b'");
    }
}
