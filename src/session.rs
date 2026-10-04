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
        // tmux clients inherit the connection environment even when the pane/server
        // predates SSH. Read the attached client on every request, never mutate tmux.
        #[cfg(target_os = "linux")]
        {
            let client_pid = || -> Result<u32> {
                let output = Command::new("tmux")
                    .args(["list-clients", "-t"])
                    .arg(&pane)
                    .args(["-F", "#{client_pid}"])
                    .output()?;
                if !output.status.success() {
                    bail!("cannot resolve tmux clients");
                }
                single_client_pid(std::str::from_utf8(&output.stdout)?)
            };
            let pid = client_pid()?;
            let endpoint = linux_client_endpoint(pid)?;
            if client_pid()? != pid {
                bail!("tmux client changed during bridge lookup; retry the paste");
            }
            return Ok(endpoint);
        }
        #[cfg(not(target_os = "linux"))]
        {
            let clients = Command::new("tmux")
                .args(["list-clients", "-t"])
                .arg(&pane)
                .args(["-F", "#{client_name}"])
                .output()?;
            if !clients.status.success()
                || String::from_utf8_lossy(&clients.stdout).lines().count() != 1
            {
                bail!(
                    "tmux needs exactly one attached client for automatic clipboard/file routing"
                );
            }
            let output = Command::new("tmux")
                .args(["show-environment", "-t"])
                .arg(pane)
                .arg(BRIDGE_ENV)
                .output()?;
            if !output.status.success() {
                bail!("outside Linux, use `agentdrop run --tmux` or an explicit --bridge");
            }
            return Endpoint::parse(
                std::str::from_utf8(&output.stdout)?
                    .trim_end()
                    .strip_prefix("AGENTDROP_BRIDGE=")
                    .context("tmux bridge is unset")?,
            );
        }
    }
    Endpoint::parse(&std::env::var(BRIDGE_ENV).context("no bridge binding; connect using `agentdrop connect HOST` or `agentdrop run HOST -- AGENT`")?)
}

#[cfg(any(target_os = "linux", test))]
fn single_client_pid(text: &str) -> Result<u32> {
    let clients: Vec<_> = text.lines().collect();
    if clients.len() != 1 {
        bail!("tmux needs exactly one attached client for automatic clipboard/file routing");
    }
    let pid: u32 = clients[0].parse().context("invalid tmux client PID")?;
    if pid <= 1 {
        bail!("invalid tmux client PID");
    }
    Ok(pid)
}

#[cfg(any(target_os = "linux", test))]
fn endpoint_from_environ(bytes: &[u8]) -> Result<Endpoint> {
    for entry in bytes.split(|b| *b == 0) {
        if let Some(value) = entry.strip_prefix(b"AGENTDROP_BRIDGE=") {
            return Endpoint::parse(
                std::str::from_utf8(value).context("invalid bridge environment")?,
            );
        }
    }
    bail!(
        "attached tmux client has no bridge; connect using `agentdrop connect HOST`, then attach normally"
    )
}

#[cfg(target_os = "linux")]
fn linux_client_endpoint(pid: u32) -> Result<Endpoint> {
    use std::io::Read;
    use std::os::unix::fs::MetadataExt;
    let file = std::fs::File::open(format!("/proc/{pid}/environ")).context(
        "cannot read tmux client environment; check /proc permissions or use explicit --bridge",
    )?;
    if file.metadata()?.uid() != unsafe { libc::geteuid() } {
        bail!("tmux client belongs to another user");
    }
    let mut bytes = Vec::new();
    file.take(1024 * 1024 + 1).read_to_end(&mut bytes)?;
    if bytes.len() > 1024 * 1024 {
        bail!("tmux client environment is too large");
    }
    endpoint_from_environ(&bytes)
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
    fn passive_client_lookup_rejects_ambiguity_and_missing_binding() {
        assert_eq!(single_client_pid("123\n").unwrap(), 123);
        for value in ["", "1\n", "123\n456\n", "bad\n"] {
            assert!(single_client_pid(value).is_err());
        }
        let endpoint = Endpoint::fresh();
        let env = format!(
            "PATH=/bin\0OTHER=value\0AGENTDROP_BRIDGE={}\0",
            endpoint.encode()
        );
        assert_eq!(
            endpoint_from_environ(env.as_bytes()).unwrap().encode(),
            endpoint.encode()
        );
        assert!(endpoint_from_environ(b"PATH=/bin\0").is_err());
        assert!(endpoint_from_environ(b"AGENTDROP_BRIDGE=invalid\0").is_err());
    }

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
