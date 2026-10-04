mod clipboard;
mod connect;
mod paste;
mod paths;
mod protocol;
mod proxy;
mod session;
mod shell_integration;
mod transfer;

use anyhow::Result;
use clap::{Args, Parser, Subcommand};
use std::ffi::OsString;
use std::path::PathBuf;

#[derive(Debug, Parser)]
#[command(
    name = "agentdrop",
    version,
    about = "Drop local files and paste screenshots into remote Agents over one SSH connection"
)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Debug, Args)]
struct Connection {
    /// SSH host alias or user@host
    destination: String,
    /// Local SSH executable (tssh recommended on Windows)
    #[arg(long, default_value = "tssh")]
    tssh: OsString,
    /// Extra SSH argument; repeat, e.g. --ssh-arg=-p --ssh-arg=2222
    #[arg(long = "ssh-arg", allow_hyphen_values = true)]
    ssh_args: Vec<OsString>,
    /// Restrict local file sharing to this directory (repeatable; symlinks resolved)
    #[arg(long)]
    allow_root: Vec<PathBuf>,
    /// Disable sharing local clipboard images
    #[arg(long)]
    no_clipboard: bool,
    /// Remote agentdrop executable
    #[arg(long, default_value = "agentdrop")]
    remote_bin: String,
}
impl Connection {
    fn options(self) -> connect::Options {
        connect::Options {
            tssh: self.tssh,
            destination: self.destination,
            tssh_args: self.ssh_args,
            allow_roots: self.allow_root,
            clipboard: !self.no_clipboard,
            remote_bin: self.remote_bin,
            command: Vec::new(),
            zsh: false,
            tmux: None,
        }
    }
}

#[derive(Debug, Subcommand)]
enum Command {
    /// Print opt-in shell integration; source it at the end of your remote .zshrc
    Init {
        #[arg(value_parser = ["zsh"])]
        shell: String,
    },
    /// Open a remote login shell with a bound bridge
    Connect {
        #[command(flatten)]
        connection: Connection,
        /// Extra tssh options (legacy syntax)
        #[arg(last = true, allow_hyphen_values = true)]
        tssh_args: Vec<OsString>,
    },
    /// Connect and launch an Agent in one command
    Run {
        #[command(flatten)]
        connection: Connection,
        /// Create or reconnect a dedicated tmux session
        #[arg(long)]
        tmux: Option<String>,
        /// Load remote .zshrc functions before running the Agent
        #[arg(long)]
        zsh: bool,
        #[arg(required = true, last = true, allow_hyphen_values = true)]
        command: Vec<OsString>,
    },
    /// Wrap an Agent on the remote Unix host
    Proxy {
        /// Explicit socket:token binding; normally inherited from the connection
        #[arg(long)]
        bridge: Option<String>,
        #[arg(long)]
        zsh: bool,
        #[arg(required = true, trailing_var_arg = true, allow_hyphen_values = true)]
        command: Vec<OsString>,
    },
    /// Bind and attach a dedicated tmux session (invoked by run --tmux)
    Attach {
        #[arg(long)]
        session: String,
        #[arg(long)]
        zsh: bool,
        #[arg(required = true, last = true, allow_hyphen_values = true)]
        command: Vec<OsString>,
    },
}

fn main() {
    let code = match run() {
        Ok(code) => code,
        Err(error) => {
            eprintln!("agentdrop: {error:#}");
            1
        }
    };
    std::process::exit(code);
}
fn run() -> Result<i32> {
    match Cli::parse().command {
        Command::Init { shell: _ } => {
            print!("{}", shell_integration::ZSH);
            Ok(0)
        }
        Command::Connect {
            connection,
            tssh_args,
        } => {
            let mut options = connection.options();
            options.tssh_args.extend(tssh_args);
            connect::run(options)
        }
        Command::Run {
            connection,
            tmux,
            zsh,
            command,
        } => {
            let mut options = connection.options();
            options.tmux = tmux;
            options.zsh = zsh;
            options.command = command;
            connect::run(options)
        }
        Command::Proxy {
            command,
            bridge,
            zsh,
        } => proxy::run(command, bridge, zsh),
        Command::Attach {
            command,
            session,
            zsh,
        } => session::attach(&session, command, zsh),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn run_preserves_agent_flags_and_ssh_options() {
        let cli = Cli::try_parse_from([
            "agentdrop",
            "run",
            "dev",
            "--tmux",
            "coding",
            "--ssh-arg=-p",
            "--ssh-arg=2222",
            "--",
            "codex",
            "--model",
            "x y",
        ])
        .unwrap();
        let Command::Run {
            connection,
            command,
            tmux,
            ..
        } = cli.command
        else {
            panic!()
        };
        assert_eq!(connection.ssh_args, ["-p", "2222"]);
        assert_eq!(command, ["codex", "--model", "x y"]);
        assert_eq!(tmux.as_deref(), Some("coding"));
        assert!(Cli::try_parse_from(["agentdrop", "run", "dev"]).is_err());
    }
}
