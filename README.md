# alacritty-agent-drop

让 **Windows Explorer / macOS Finder → Alacritty → tssh → Ubuntu/tmux → Codex / Claude 等 Agent CLI** 支持类似 Wave 的“拖入本地文件后，Agent 获得远端可访问路径”的体验。

## 为什么改成两段式架构

`0.1.x` 曾把 `agentdrop` 放在 Alacritty 和 `tssh` 之间，透明代理整个本地 PTY：

```text
Alacritty → agentdrop PTY proxy → tssh → tmux → Agent
```

这种设计会让 `agentdrop` 参与 Windows Console / ConPTY、方向键、Ctrl-A/E/R、terminal raw mode 等所有交互协议，容易破坏正常终端行为。

`0.2.x` 改成：

```text
Windows / macOS                         Ubuntu remote

Alacritty                               tmux
   │                                      │
   │ native terminal I/O                  ▼
   ▼                                agentdrop proxy
agentdrop connect                         │
   │                                      ▼
   └──── exec tssh directly ───────────► Codex / Claude
   │
   └─ local upload bridge
          ▲
          │ SSH RemoteForward (Unix socket, mode 0600)
          └──────────────────────────── agentdrop proxy
```

**本机 `agentdrop connect` 不读取、不解析、不重写 stdin/stdout。** 它只启动本地上传 bridge，然后以 inherited stdio 直接启动 `tssh`。因此方向键、Ctrl-A、Ctrl-E、Ctrl-R、tmux shortcut 等全部由 Alacritty + tssh 原生处理。

只有远端 `agentdrop proxy` 包住 Agent CLI，职责与之前的 `wave-paste-proxy` 类似：识别 bracketed paste 中的 Windows/macOS 本地路径，请求本机 bridge 上传，然后把远端绝对路径写入 Agent 输入框。

## 要求

### 本机 Windows / macOS

- Alacritty
- `tssh` / trzsz-ssh **0.1.23+**（需要 RemoteForward Unix socket 支持）
- Rust stable（从源码安装时）

Windows：

```powershell
winget install tssh
```

macOS：

```zsh
brew install trzsz-ssh
```

### 远端 Ubuntu / Linux

- `trz` 可执行文件在 `PATH`
- `agentdrop` 安装在远端
- Agent CLI（Codex、Claude Code 等）

确认：

```zsh
trz --version
```

## 安装

本机和远端都可以安装同一个 Rust crate：

```text
cargo install --git https://github.com/v0v0/alacritty-agent-drop.git --force
```

本机使用 `connect` 模式，远端 Ubuntu 使用 `proxy` 模式。

## 使用

### 1. 本机连接远端

原来：

```text
tssh dev
```

改成：

```text
agentdrop connect dev
```

额外 tssh 参数放在 `--` 后：

```text
agentdrop connect dev -- -A
```

自定义 tssh 路径：

```powershell
agentdrop connect dev --tssh C:\Tools\tssh.exe
```

```zsh
agentdrop connect dev --tssh /opt/homebrew/bin/tssh
```

`connect` 实际会给主 tssh 连接增加：

```text
-o EnableDragFile=no
-o StreamLocalBindUnlink=yes
-o StreamLocalBindMask=0177
-R /tmp/agentdrop-<uuid>.sock:127.0.0.1:<local-random-port>
```

这里主动关闭 `tssh` 自带 `EnableDragFile`。原因是 tssh 的原生拖拽实现会向当前 pane 发送 Ctrl-C，再运行 `trz`；如果 Codex/Claude 正在前台，会中断 Agent。

手工在 shell 中运行 `trz` / `tsz` 的能力不受影响。

### 2. 远端用 proxy 启动 Agent

如果 Agent 是普通 PATH 中的二进制：

```zsh
agentdrop proxy -- codex
```

```zsh
agentdrop proxy -- claude
```

如果你的 `codex` / `claude` 本身是 `~/.zshrc` 中的 **zsh function**，例如用来注入代理、API key 或其他环境变量，使用：

```zsh
agentdrop proxy --zsh -- codex
```

```zsh
agentdrop proxy --zsh -- claude
```

`--zsh` 会在 Agent PTY 内启动：

```text
zsh -lic '"$@"' agentdrop-proxy <agent> <args...>
```

因此 `.zshrc` 会正常加载，原有 Agent function 和环境初始化仍然生效。参数通过 positional arguments 传递，不通过字符串拼接或 `eval`。

> 说明：这里保证的是 zsh **function** 和启动环境。alias 是 zsh 的词法展开，不建议把 Agent 启动逻辑只放在 alias 里。

#### 推荐：保留原有 function，增加独立入口

不要直接把已有的 `codex()` / `claude()` 覆盖成代理函数，否则内层 `zsh -lic` 再加载 `.zshrc` 时容易递归。

推荐新增：

```zsh
codexd() {
    agentdrop proxy --zsh -- codex "$@"
}

clauded() {
    agentdrop proxy --zsh -- claude "$@"
}
```

这样原来的：

```zsh
codex
claude
```

完全保持原状；需要拖拽桥接时使用：

```zsh
codexd
clauded
```

如果希望最终仍然输入 `codex` 就自动进入 proxy，建议在确认方案稳定后再做单独的 zsh integration，而不是直接覆盖原函数。

`agentdrop proxy` 可以在 tmux pane 内运行，不需要修改 tmux 配置。

## 拖拽时发生什么

例如 Codex 正在前台，从 Windows Explorer 拖：

```text
C:\Users\me\Desktop\shot.png
```

流程：

```text
1. Alacritty 把本地路径作为 bracketed paste 发送
2. tssh 原样透传，不做 drag upload
3. Ubuntu 上的 agentdrop proxy 看到 C:\... 本地路径
4. proxy 连接 /tmp/agentdrop-*.sock
5. SSH RemoteForward 把请求转回本机 agentdrop bridge
6. 本机 bridge 验证文件真实存在
7. bridge 另开一条 tssh --upload-file 连接上传
8. 保存到：
   $HOME/.cache/agentdrop/files/<session>/<request>/shot.png
9. proxy 收到相对路径，在 Ubuntu 解析成绝对路径
10. Codex 输入框最终收到：
    /home/me/.cache/agentdrop/files/.../shot.png
```

当前 Agent TUI 不会被切换到 `trz`，主 SSH 连接也不会被上传协议占用。

macOS Finder 的 `/Users/...` 路径采用同一机制。如果一个 Unix 绝对路径本身已经存在于远端，proxy 会认为它是正常远端路径，不触发上传。

## 为什么键盘行为不会再被本机 agentdrop 破坏

`connect` 使用标准 `std::process::Command`：

```text
stdin  = inherit
stdout = inherit
stderr = inherit
```

因此：

```text
Alacritty keyboard event
        ↓
tssh 自己的 Windows/macOS terminal implementation
        ↓
SSH
        ↓
tmux
```

本机 `agentdrop` 不再调用 `enable_raw_mode()`，不创建 ConPTY/portable-pty，也不解析 `ESC[A` 或 Ctrl 控制字节。

远端 proxy 仍然需要一个 PTY，因为 Codex/Claude 是 TUI；但这个代理运行在 Ubuntu Unix PTY 上，而且只包住 Agent 进程，不影响 SSH/tmux 的全局终端语义。

## bridge socket 发现

默认情况下，远端 proxy 会扫描：

```text
/tmp/agentdrop-*.sock
```

并优先尝试最新且可连接的 socket。

如果同一远端账号同时开了多条 `agentdrop connect`，可显式指定：

```zsh
agentdrop proxy --bridge-socket /tmp/agentdrop-<uuid>.sock -- codex
```

或：

```zsh
export AGENTDROP_BRIDGE_SOCKET=/tmp/agentdrop-<uuid>.sock
```

## 安全边界

RemoteForward socket 使用：

```text
StreamLocalBindMask=0177
```

因此远端 socket 权限为当前 Unix 用户私有（0600）。其他远端用户无法通过该 socket 请求本机上传。

但要注意：**与 Agent 同一个远端 Unix 账号运行的其他进程，也处于相同信任边界内。** 它们如果能够连接这个 socket，并知道一个本机绝对路径，也可以请求 bridge 上传该文件。不要在不可信的远端账号或共享账号中启用此 bridge。

bridge 只接受“本机存在的普通文件”，当前不支持目录。

## side-channel 上传认证

上传使用第二条短连接：

```text
tssh --upload-file <local-file> dev 'trz ...'
```

因此推荐使用 SSH key、ssh-agent、Pageant 或 tssh 已保存的认证信息。否则每次拖文件都可能需要重新认证。

## 远端缓存

文件保存到：

```text
$HOME/.cache/agentdrop/files/<session>/<request>/
```

当前不自动删除。可以周期清理：

```zsh
find ~/.cache/agentdrop/files -mindepth 1 -maxdepth 1 -type d -mtime +7 -exec rm -rf -- {} +
```

## 当前限制

- 自动桥接普通文件，不上传目录。
- 依赖 Agent/TUI 开启 bracketed paste。
- 本地文件名需要能够表示为 UTF-8。
- side-channel 需要能独立完成 SSH 认证。
- 同一远端 Unix 用户被视为信任边界。
- `proxy` 模式面向 Unix/Linux 远端；Windows/macOS 是 `connect` 客户端平台。

## 开发

```text
cargo test --all-targets
cargo build --release
```

CI 验证 Windows、macOS Apple Silicon、macOS Intel 和 Ubuntu，并构建 Windows/macOS/Linux release artifact。

## License

MIT
