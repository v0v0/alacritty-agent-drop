mod connect;
mod paste;
mod protocol;
mod proxy;

use std::ffi::OsString;
use std::path::PathBuf;

use anyhow::Result;
use clap::{Parser, Subcommand};

#[derive(Debug, Parser)]
#[command(
    name = "agentdrop",
    version,
    about = "Bridge local file drops into remote Agent CLIs without proxying the SSH terminal input"
)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Debug, Subcommand)]
enum Command {
    /// Connect to a remote host with native tssh I/O plus a private upload side-channel.
    Connect {
        /// tssh destination, e.g. dev or user@example.com
        destination: String,

        /// Path or command name of the local tssh executable
        #[arg(long, default_value = "tssh")]
        tssh: OsString,

        /// Extra tssh options. Put them after `--`; they are inserted before the destination.
        #[arg(last = true, allow_hyphen_values = true)]
        tssh_args: Vec<OsString>,
    },

    /// Run on the remote Unix host and wrap only the Agent CLI process.
    Proxy {
        /// Explicit reverse-forwarded Unix socket. Normally auto-discovered from /tmp.
        #[arg(long)]
        bridge_socket: Option<PathBuf>,

        /// Start the Agent through `zsh -lic`, preserving functions/aliases/env setup from .zshrc.
        #[arg(long)]
        zsh: bool,

        /// Agent command and arguments, for example: `codex` or `claude`.
        #[arg(required = true, trailing_var_arg = true, allow_hyphen_values = true)]
        command: Vec<OsString>,
    },
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
    match cli.command {
        Command::Connect {
            destination,
            tssh,
            tssh_args,
        } => connect::run(tssh, destination, tssh_args),
        Command::Proxy {
            bridge_socket,
            zsh,
            command,
        } => proxy::run(command, bridge_socket, zsh),
    }
}
