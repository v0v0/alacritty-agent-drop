# alacritty-agent-drop

让 **Windows Explorer / macOS Finder → Alacritty → tssh → Ubuntu/tmux → Codex / Claude 等 Agent CLI** 支持类似 Wave 的本地文件拖拽与截图粘贴体验。

支持两类输入：

```text
文件拖拽：Explorer / Finder → remote file path → Agent
截图粘贴：Win+Shift+S / macOS screenshot → clipboard image → Ctrl-V → remote PNG path → Agent
```

## 架构

`0.1.x` 曾把 `agentdrop` 放在 Alacritty 和 `tssh` 之间，透明代理整个本地 PTY：

```text
Alacritty → agentdrop PTY proxy → tssh → tmux → Agent
```

这种设计会让 `agentdrop` 参与 Windows Console / ConPTY、方向键、Ctrl-A/E/R、terminal raw mode 等所有交互协议，容易破坏正常终端行为。

`0.2.x` 起改成两段式架构，`0.3.x` 在同一 side-channel 上增加 clipboard image：

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
   └─ local bridge
          ▲
          │ SSH RemoteForward (Unix socket, mode 0600)
          └──────── file / clipboard ── agentdrop proxy
```

**本机 `agentdrop connect` 不读取、不解析、不重写 stdin/stdout。** 它只启动本地 bridge，然后以 inherited stdio 直接启动 `tssh`。方向键、Ctrl-A、Ctrl-E、Ctrl-R、tmux shortcut 等继续由 Alacritty + tssh 原生处理。

只有远端 `agentdrop proxy` 包住 Agent CLI：

- bracketed paste 中出现 Windows/macOS 本地文件路径时，请求本机上传；
- 普通 raw input 中出现 `Ctrl-V` (`0x16`) 时，请求本机读取 clipboard image；
- 上传完成后把 Ubuntu 绝对路径作为 bracketed paste 注入 Agent。

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

本机和远端都安装同一个 crate：

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

这里主动关闭 `tssh` 自带 `EnableDragFile`。tssh 的原生拖拽上传会在当前 pane 里发送 Ctrl-C 并运行 `trz`，会中断正在前台运行的 Agent TUI。

手工在 shell 中运行 `trz` / `tsz` 不受影响。

### 2. 远端用 proxy 启动 Agent

普通 PATH 二进制：

```zsh
agentdrop proxy -- codex
agentdrop proxy -- claude
```

如果 `codex` / `claude` 本身是 `~/.zshrc` 中的 **zsh function**，例如用于注入代理、API key 或其他环境变量：

```zsh
agentdrop proxy --zsh -- codex
agentdrop proxy --zsh -- claude
```

`--zsh` 会在 Agent PTY 内启动：

```text
zsh -lic '"$@"' agentdrop-proxy <agent> <args...>
```

因此 `.zshrc` 会正常加载，原有 Agent function 和环境初始化仍然生效。参数使用 positional arguments，不通过字符串拼接或 `eval`。

> 这里保证 zsh **function** 和启动环境；alias 属于词法展开，不建议把 Agent 启动逻辑只放在 alias 里。

推荐保留原 function，增加独立入口：

```zsh
codexd() {
    agentdrop proxy --zsh -- codex "$@"
}

clauded() {
    agentdrop proxy --zsh -- claude "$@"
}
```

需要增强能力时运行：

```zsh
codexd
clauded
```

`agentdrop proxy` 可以直接运行在 tmux pane 内，不需要修改 tmux 配置。

## 截图直接 Ctrl-V

### Windows

例如：

```text
Win+Shift+S
    ↓
框选截图，图片进入 Windows Clipboard
    ↓
回到 Alacritty 中正在运行的 remote Codex
    ↓
Ctrl-V
```

流程：

```text
1. Alacritty 把 Ctrl-V 作为 0x16 发给 tssh
2. tssh / SSH / tmux 原样传到 Ubuntu
3. agentdrop proxy 只在 Agent 边界识别 0x16
4. proxy 通过 /tmp/agentdrop-*.sock 请求本机 clipboard image
5. Windows agentdrop connect 使用系统 clipboard API 读取图片
6. 图片编码为本机临时 PNG
7. bridge 通过第二条 tssh --upload-file 上传 PNG
8. 本机临时 PNG 删除
9. Ubuntu 得到：
   $HOME/.cache/agentdrop/files/<session>/<request>/clipboard-<uuid>.png
10. proxy 把该 Ubuntu 绝对路径作为 bracketed paste 注入 Codex
```

最终 Codex 接收到的是远端真实存在的图片路径，而不是 Windows bitmap 或本机路径。

如果本机 clipboard **没有图片**，`agentdrop proxy` 不会吞掉 Ctrl-V，而是把原始 `0x16` 转发给 Agent。

### macOS

本地 bridge 同样支持 macOS system clipboard image。触发协议当前仍然是远端 Agent 收到的 `Ctrl-V` (`0x16`)；macOS 常规文本粘贴继续使用 `Cmd-V`，不会经过这条 clipboard-image trigger。

## 普通文本粘贴

截图粘贴不会改变 Alacritty 原本的文本粘贴链路。

Windows 常规文本 paste 通常仍使用：

```text
Ctrl+Shift+V
```

它会进入 bracketed paste，并直接传给 Agent；其中即使文本里包含普通字符，也不会触发 clipboard-image 请求。

## 文件拖拽

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
10. Codex 输入框收到 remote absolute path
```

macOS Finder 的 `/Users/...` 路径采用同一机制。如果一个 Unix 绝对路径本身已经存在于远端，proxy 会认为它是正常远端路径，不触发上传。

## 为什么键盘行为不再被本机 agentdrop 破坏

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

本机 `agentdrop` 不调用 `enable_raw_mode()`，不创建 ConPTY/portable-pty，也不解析 `ESC[A`、Ctrl-A/E/R 等输入。

远端 proxy 需要一个 Unix PTY，因为 Codex/Claude 是 TUI。它只包住 Agent 进程，并且只对两类输入增强：bracketed-pasted 本地路径和单独的 Ctrl-V clipboard-image trigger。

## bridge socket 发现

默认情况下，远端 proxy 扫描：

```text
/tmp/agentdrop-*.sock
```

并优先尝试最新且可连接的 socket。

同一远端账号同时开多条 `agentdrop connect` 时可显式指定：

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

因此远端 socket 只允许当前 Unix 用户访问（0600）。

但必须注意：**与 Agent 同一个远端 Unix 账号运行的其他进程属于同一信任边界。** 能连接该 socket 的进程可以：

- 请求上传一个它知道绝对路径的本机普通文件；
- 请求读取并上传当前本机 clipboard image。

因此不要在不可信的远端账号、共享账号或同一账号运行不可信代码的环境中启用此 bridge。

bridge 不提供任意本机文件枚举；文件上传仍要求请求方知道具体绝对路径。clipboard 请求只读取 image format，不读取 clipboard text。

## side-channel 上传认证

上传使用第二条短连接：

```text
tssh --upload-file <local-file> dev 'trz ...'
```

推荐使用 SSH key、ssh-agent、Pageant 或 tssh 已保存的认证信息。否则每次拖文件/粘贴截图都可能需要再次认证。

## 临时文件与远端缓存

clipboard image 会先写入本机系统 temp 下的：

```text
agentdrop/clipboard/clipboard-<uuid>.png
```

上传结束后立即删除该本机临时文件。

远端文件保存在：

```text
$HOME/.cache/agentdrop/files/<session>/<request>/
```

远端缓存当前不自动删除，可以周期清理：

```zsh
find ~/.cache/agentdrop/files -mindepth 1 -maxdepth 1 -type d -mtime +7 -exec rm -rf -- {} +
```

## 当前限制

- 自动桥接普通文件，不上传目录。
- clipboard image 在 Windows/macOS 本机支持；Linux `connect` 不读取桌面 clipboard。
- 截图粘贴 trigger 当前固定为 `Ctrl-V` (`0x16`)。
- 文件拖拽依赖 Agent/TUI 开启 bracketed paste。
- 本地文件名需要能够表示为 UTF-8。
- side-channel 需要能独立完成 SSH 认证。
- 同一远端 Unix 用户被视为信任边界。
- `proxy` 模式面向 Unix/Linux 远端；Windows/macOS 是主要 `connect` 客户端平台。

## 开发

```text
cargo test --all-targets
cargo build --release
```

CI 验证 Windows、macOS Apple Silicon、macOS Intel 和 Ubuntu，并构建 Windows/macOS/Linux release artifact。

## License

MIT
