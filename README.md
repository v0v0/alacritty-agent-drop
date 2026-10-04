# alacritty-agent-drop · v2

把本机文件和截图送进远端 Codex / Claude Code 输入框。支持 **Windows / macOS → Alacritty → tssh → Linux → tmux → Agent**。

v2 的目标是：**一次连接、一条启动命令，拖文件或按 Ctrl-V 即可。** 文件通过已有 SSH 的转发通道直接流式传输，无需 `trz`，无需第二次 SSH 认证。本机终端输入继续由 SSH 客户端原生处理。

> 本分支的软件版本为 `0.4.0`，线协议为 `3`。本机和远端需同时安装 v2，不能与 `main` 的 `0.3.x` 混用。设计评审与取舍见 [docs/v2-design.md](docs/v2-design.md)。

## 安装

本机和远端都安装（从源码需要 Rust stable）：

```sh
cargo install --git https://github.com/v0v0/alacritty-agent-drop.git --branch v2 --locked --force
```

确保 `agentdrop` 在 PATH 中。远端默认还会查找 `~/.cargo/bin` 和 `~/.local/bin`。

本机需要 Alacritty、tssh 0.1.23+（支持 Unix socket RemoteForward）；远端需要 Linux、Agent CLI。只有使用 `--tmux` 时才需要 tmux 3.2+。

```powershell
# Windows
winget install tssh
```

```sh
# macOS
brew install trzsz-ssh
```

也可以从本仓库 `ci` workflow 的 Artifacts 下载对应系统的二进制。Windows 为 `agentdrop.exe`；macOS 分 arm64 和 x86_64；Linux 为 x86_64。macOS 系统剪贴板需要在用户桌面会话中使用。

## 推荐：一条命令启动

```sh
agentdrop run dev -- codex
agentdrop run dev -- claude
```

`dev` 是你已有的 SSH Host 别名，原有密钥、跳板机和 SSH 配置继续使用。

需要断线后保留 Agent：

```sh
agentdrop run dev --tmux coding -- codex
```

断线或 `Ctrl-B D` 分离后，再执行**同一命令**重连。已有 Agent 进程会继续运行，粘贴通道切换到本次连接；已有 session 不会重新执行命令参数。不同任务或电脑使用不同名字，例如 `--tmux coding-mac`。

已有 session 仍有客户端连接时，`agentdrop` 会拒绝接管；先分离旧客户端。如果网络断开但服务器尚未发现断线，需要等 SSH 断线检测或手工分离旧客户端。

如果 `codex` / `claude` 是 `.zshrc` 中的 function：

```sh
agentdrop run dev --tmux coding --zsh -- codex
```

`--zsh` 在远端加载 `.zshrc`，通过 positional arguments 执行函数并保留参数，不拼接 `eval`。不保证 shell alias；请使用 function 或可执行文件。

## 拖文件与截图

- **拖文件**：从 Explorer / Finder 拖入 Agent 输入框。支持一次多个文件、中文、空格、Windows 盘符 / UNC、Finder 转义路径。文件完整保存到远端后，再注入带引号的绝对路径。
- **粘贴截图**：Windows `Win+Shift+S` 截图后，在 Agent 中按 **Ctrl-V**。macOS 把截图复制到剪贴板后也按 **Ctrl-V**。
- **粘贴普通文字**：保留 Alacritty 原来的快捷键，Windows 通常为 `Ctrl+Shift+V`，macOS 为 `Cmd-V`。文本里的 Ctrl-V 字节不会触发取图。
- 本机没有剪贴板图片时，原始 Ctrl-V 交给 Agent。
- 上传失败时显示错误，原始粘贴内容交给 Agent；不会把尚未写完的远端文件路径交给 Agent。

确保 Alacritty 的 Ctrl-V 向终端发送 `0x16`，而不是被自定义映射成 Paste。若有冲突，在现有 `alacritty.toml` 的键绑定数组中合并：

```toml
[[keyboard.bindings]]
key = "V"
mods = "Control"
chars = "\u0016"
```

文件拖拽仍要求 Agent 开启 bracketed paste。目录不上传。已经存在于远端的 Unix 路径按远端文件处理；本机和远端同名绝对路径冲突时，优先远端路径。

## 保留先连接、再启动的方式

```sh
agentdrop connect dev
# 进入远端后：
agentdrop proxy -- codex
# 或：
agentdrop proxy --zsh -- claude
```

需要 tmux 时推荐直接从本机使用 `run --tmux`。也可以在上述远端 shell 中使用：

```sh
agentdrop attach --session coding -- codex
```

不要依赖普通 `tmux attach` 自动更新桥接变量；由 `agentdrop attach` 管理绑定和重连。同一 tmux session 有多个客户端时，自动上传和取图会报错，不猜测来源。直接代理和不同 tmux session 相互独立。

## 连接参数

```sh
# 自定义 SSH 客户端和远端二进制路径
agentdrop run dev --tssh /opt/homebrew/bin/tssh --remote-bin /opt/bin/agentdrop -- codex

# 额外 SSH 参数，必须与 Agent 参数区分
agentdrop run dev --ssh-arg=-p --ssh-arg=2222 -- codex --model my-model

# connect 仍支持旧的额外参数形式
agentdrop connect dev -- -A

# Unix 上也可使用支持 Unix socket 转发的 OpenSSH
agentdrop run dev --tssh ssh -- codex
```

本项目管理 `-tt`、`-R`、`EnableDragFile`、`StreamLocalBindMask` 和远端启动命令。额外参数用于端口、身份文件、跳板机等；不要同时配置 `-N`、`-f`、`-T`、`RemoteCommand`、清除转发或另一条远端命令。

## 分享范围

默认允许请求本机已知绝对路径的普通文件和当前剪贴板图片。可以缩小范围：

```powershell
agentdrop run dev --allow-root C:\Users\me\Pictures --allow-root C:\work -- codex
```

```sh
agentdrop run dev --allow-root "$HOME/Pictures" --no-clipboard -- codex
```

`--allow-root` 可重复，对路径先解析符号链接再检查；它限制文件拖拽，剪贴板由 `--no-clipboard` 单独控制。

每次连接生成独立 socket 和随机 token，远端 socket 为 0600，本机 bridge 只监听 loopback。**远端同一 Unix 用户及本机可读取启动参数/环境的进程仍属于信任边界**，token 不能隔离它们。只向可信主机和账号开启分享。

## 缓存与限制

远端文件位于：

```text
~/.cache/agentdrop/files/<request-id>/<original-name>
```

缓存目录 0700，文件 0600。先写临时文件，校验接收长度并落盘，最后重命名；异常时清理该请求目录。原文件名含路径分隔符或控制字符会被拒绝。

- 单文件上限 256 MiB；单次粘贴最多识别 32 个路径。
- 每个连接最多同时处理 8 个 bridge 请求；控制头上限 64 KiB。
- 传输停顿超时 30 秒；等待期间输入按原顺序排队，有界队列避免无限增长。大文件传输期间按键可能延后，当前没有独立取消键。
- 超过 8 MiB 的文本粘贴以原字节透传，不做文件识别或 Ctrl-V 解释。
- 本机截图临时文件在请求结束时清理；进程被强制结束可能留下 `agentdrop-clipboard-*.png`。
- 远端缓存不自动过期，避免长任务引用的附件突然消失。确认 Agent 不再使用后可手动删除旧请求目录。
- Linux 本机不读取桌面剪贴板；仍可桥接本地普通文件。
- 不提供本地 PTY 代理、目录递归上传、跨用户分享或多个客户端共同控制同一 Agent 的自动剪贴板路由。

## 开发与验证

```sh
cargo fmt --check
cargo clippy --locked --all-targets -- -D warnings
cargo test --locked --all-targets
cargo build --locked
# Linux，有 Python 3、tmux 和正常的 PTY/Unix socket 权限：
python3 tests/e2e.py
```

CI 对 Windows、macOS arm64、macOS x86_64 和 Ubuntu 运行单元测试并构建二进制；Ubuntu 额外运行真实 PTY / Unix socket / tmux 重连集成测试。真实 Alacritty 拖拽和系统剪贴板仍需在桌面环境手工验收，详见设计文档。

MIT License.
