//! Opt-in shell startup integration; never edits shell or tmux configuration.
pub const ZSH: &str = r#"# Put eval "$(agentdrop init zsh)" at the END of ~/.zshrc.
# The child zsh reloads the original functions with this integration disabled.
if [[ -z "${AGENTDROP_PROXY_ACTIVE:-}" ]]; then
    function codex { command agentdrop proxy --zsh -- codex "$@"; }
    function claude { command agentdrop proxy --zsh -- claude "$@"; }
fi
"#;
