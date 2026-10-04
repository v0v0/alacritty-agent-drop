use anyhow::{Context, Result, bail};
use std::ffi::OsString;
use std::path::PathBuf;

pub const BRIDGE_ENV: &str = "AGENTDROP_BRIDGE";

#[derive(Clone)]
pub struct Endpoint {
    pub socket: PathBuf,
    pub token: String,
}
impl Endpoint {
    pub fn fresh() -> Self {
        Self {
            socket: PathBuf::from(format!(
                "/tmp/agentdrop-{}.sock",
                uuid::Uuid::new_v4().simple()
            )),
            token: format!(
                "{}{}",
                uuid::Uuid::new_v4().simple(),
                uuid::Uuid::new_v4().simple()
            ),
        }
    }
    pub fn encode(&self) -> String {
        format!("{}:{}", self.socket.display(), self.token)
    }
    pub fn parse(text: &str) -> Result<Self> {
        let (socket, token) = text
            .rsplit_once(':')
            .context("invalid AGENTDROP_BRIDGE binding")?;
        if !socket.starts_with('/')
            || socket.chars().any(char::is_control)
            || token.len() != 64
            || !token.bytes().all(|b| b.is_ascii_hexdigit())
        {
            bail!("invalid AGENTDROP_BRIDGE binding");
        }
        Ok(Self {
            socket: socket.into(),
            token: token.into(),
        })
    }
}

pub fn shell_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\\''"))
}

pub fn validate_session(session: &str) -> Result<()> {
    if session.is_empty()
        || session.len() > 80
        || !session
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"_-".contains(&b))
    {
        bail!("tmux session must contain 1–80 letters, digits, '_' or '-'");
    }
    Ok(())
}

#[cfg(unix)]
pub fn current_endpoint(explicit: Option<&str>) -> Result<Endpoint> {
    use std::process::Command;
    if let Some(endpoint) = explicit {
        return Endpoint::parse(endpoint);
    }
    if let Some(pane) = std::env::var_os("TMUX_PANE") {
        // Resolve at every request, so an already-running Agent survives a reconnect.
        let clients = Command::new("tmux")
            .args(["list-clients", "-t"])
            .arg(&pane)
            .args(["-F", "#{client_name}"])
            .output()?;
        if !clients.status.success() {
            bail!("cannot resolve tmux session clients");
        }
        if String::from_utf8_lossy(&clients.stdout).lines().count() != 1 {
            bail!("tmux needs exactly one attached client for automatic clipboard/file routing");
        }
        let output = Command::new("tmux")
            .args(["show-environment", "-t"])
            .arg(pane)
            .arg(BRIDGE_ENV)
            .output()?;
        if !output.status.success() {
            bail!(
                "tmux bridge is not bound; reconnect using `agentdrop run HOST --tmux NAME -- AGENT`"
            );
        }
        let text = std::str::from_utf8(&output.stdout)?;
        return Endpoint::parse(
            text.trim_end()
                .strip_prefix("AGENTDROP_BRIDGE=")
                .context("tmux bridge is unset")?,
        );
    }
    Endpoint::parse(&std::env::var(BRIDGE_ENV).context("no bridge binding; connect using `agentdrop connect HOST` or `agentdrop run HOST -- AGENT`")?)
}

#[cfg(unix)]
pub fn attach(session: &str, command: Vec<OsString>, zsh: bool) -> Result<i32> {
    use std::os::unix::process::CommandExt;
    use std::process::Command;
    validate_session(session)?;
    let endpoint = Endpoint::parse(
        &std::env::var(BRIDGE_ENV).context("attach requires an agentdrop connection")?,
    )?;
    let target = format!("={session}");
    let exists = Command::new("tmux")
        .args(["has-session", "-t", &target])
        .stderr(std::process::Stdio::null())
        .status()
        .context("tmux is required for --tmux")?
        .success();
    if exists {
        let clients = Command::new("tmux")
            .args(["list-clients", "-t", &target, "-F", "#{client_name}"])
            .output()?;
        if !clients.status.success() || !clients.stdout.is_empty() {
            bail!(
                "tmux session is already attached; detach it first or choose another --tmux name"
            );
        }
    } else {
        let binary = std::env::current_exe()?;
        let mut args = vec![binary.into_os_string(), "proxy".into()];
        if zsh {
            args.push("--zsh".into());
        }
        args.push("--".into());
        args.extend(command);
        let script = args
            .iter()
            .map(|arg| Ok(shell_quote(arg.to_str().context("command must be UTF-8")?)))
            .collect::<Result<Vec<_>>>()?
            .join(" ");
        let status = Command::new("tmux")
            .args([
                "new-session",
                "-d",
                "-s",
                session,
                "-e",
                &format!("{BRIDGE_ENV}={}", endpoint.encode()),
                &format!("exec {script}"),
            ])
            .status()?;
        if !status.success() {
            bail!("failed to create tmux session");
        }
    }
    let status = Command::new("tmux")
        .args([
            "set-environment",
            "-t",
            &target,
            BRIDGE_ENV,
            &endpoint.encode(),
        ])
        .status()?;
    if !status.success() {
        bail!("failed to bind tmux session");
    }
    Err(Command::new("tmux")
        .args(["attach-session", "-t", &target])
        .exec()
        .into())
}

#[cfg(not(unix))]
pub fn attach(_session: &str, _command: Vec<OsString>, _zsh: bool) -> Result<i32> {
    bail!("attach runs on the remote Unix host")
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn endpoint_roundtrip_and_session_validation() {
        let endpoint = Endpoint::fresh();
        let parsed = Endpoint::parse(&endpoint.encode()).unwrap();
        assert_eq!(parsed.socket, endpoint.socket);
        assert_eq!(parsed.token, endpoint.token);
        for bad in ["", "/tmp/a:short", "relative:abcd"] {
            assert!(Endpoint::parse(bad).is_err());
        }
        for bad in ["", "-bad.name", "a:b", "a;touch /tmp/x"] {
            assert!(validate_session(bad).is_err());
        }
        assert!(validate_session("codex_dev-1").is_ok());
    }
    #[test]
    fn shell_quoting_keeps_arguments_literal() {
        assert_eq!(
            shell_quote("a'b $(touch /tmp/x)"),
            "'a'\\''b $(touch /tmp/x)'"
        );
        assert_eq!(shell_quote(""), "''");
    }
}
