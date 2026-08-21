# alacritty-agent-drop

让 **Windows Explorer / macOS Finder → Alacritty → tssh → Ubuntu/tmux → Codex / Claude 等 Agent CLI** 支持类似 Wave 的拖拽文件体验。

当 Agent TUI 正在前台运行时，把本机图片或其他普通文件拖进 Alacritty：

```text
Windows: C:\Users\me\Desktop\shot.png
macOS:   /Users/me/Desktop/shot.png
```

`agentdrop` 会在本机 PTY 与 `tssh` 之间拦截这个 bracketed paste，使用**第二条独立 tssh 连接**把文件上传到远端：

```text
/home/me/.cache/agentdrop/<session-id>/shot.png
```

然后把这个远端绝对路径写回当前 Agent 的输入框。当前 tmux pane、zsh 和 Agent TUI 不会被切换到 `trz`。

## 支持平台

| 本机平台 | 状态 | CI |
| --- | --- | --- |
| Windows | 正式支持 | `windows-latest` test + release build |
| macOS | 正式支持 | `macos-latest` test + release build |
| Linux | 代码兼容性测试 | `ubuntu-latest` test |

远端以 Linux/Ubuntu + `trz` 为主要目标环境。

## 数据流

```text
Explorer / Finder
      │ drag
      ▼
Alacritty
      │ bracketed paste: local absolute path
      ▼
agentdrop
      │
      ├──── tssh --upload-file ────► Ubuntu ~/.cache/agentdrop/...
      │
      ▼ rewrite
/home/me/.cache/agentdrop/.../shot.png
      │
      ▼
tssh → tmux → Codex / Claude / other TUI
```

## 前置条件

### Windows

- Alacritty
- `tssh`（trzsz-ssh）在 `PATH` 中
- Rust stable（仅从源码安装时需要）

`tssh` 可用 Scoop、winget 或 Chocolatey 安装，例如：

```powershell
winget install tssh
```

### macOS

- Alacritty
- `tssh`（Homebrew 包名为 `trzsz-ssh`）在 `PATH` 中
- Rust stable（仅从源码安装时需要）

```zsh
brew install trzsz-ssh
```

Homebrew 安装后实际命令仍然是：

```zsh
tssh --version
```

### Ubuntu 远端

远端需要安装 `trz`：

```zsh
trz --version
```

你的正常 `tssh dev` 登录需要已经可用。

由于文件上传使用额外的短连接，推荐使用 SSH key、agent，或者 tssh 已保存的认证信息，避免辅助连接需要重新交互输入密码。

## 安装 agentdrop

Windows PowerShell 或 macOS shell 都可以直接从 GitHub 编译安装：

```text
cargo install --git https://github.com/v0v0/alacritty-agent-drop.git
```

安装后确认：

```text
agentdrop --version
```

## 使用

原来：

```text
tssh dev
```

改为：

```text
agentdrop dev
```

然后远端照常：

```zsh
tmux attach
codex
```

此时可直接从 Windows Explorer 或 macOS Finder 把文件拖入 Codex/Claude 的输入区域。上传成功后，Agent 收到的是 Ubuntu 上的绝对路径，而不是本机路径。

### 额外 tssh 参数

放在 `--` 后面；`agentdrop` 会把它们插到 destination 之前：

```text
agentdrop dev -- -A
```

等价于交互连接：

```text
tssh -A dev
```

### 自定义 tssh 路径

Windows：

```powershell
agentdrop dev --tssh C:\Tools\tssh.exe
```

macOS：

```zsh
agentdrop dev --tssh /opt/homebrew/bin/tssh
```

Intel Mac 的 Homebrew 默认路径通常是 `/usr/local/bin/tssh`；如果 `tssh` 已在 `PATH` 中，不需要指定 `--tssh`。

## 与 tmux / zsh 的关系

`agentdrop` 位于本机：

```text
Alacritty → agentdrop → PTY → tssh → Ubuntu → tmux → zsh/Agent
```

因此不需要修改远端 `.zshrc`，也不需要修改 tmux 配置。

上传不会在当前 tmux pane 中执行 `trz`。它使用：

```text
connection A: agentdrop → tssh → tmux → Agent TUI
connection B: agentdrop → tssh --upload-file → trz → cache directory
```

这也是它能在 Agent CLI 正占用前台 PTY 时工作的原因。

## 路径识别规则

采用保守策略。只有同时满足以下条件才触发上传：

1. 输入是 terminal bracketed paste；
2. paste 内容只有一个路径；
3. 路径是本机绝对路径；
4. 路径指向一个真实的普通文件。

Windows 的 `C:\...` 和 macOS 的 `/Users/...` 都通过本机 `PathBuf` 原生解析，不做平台间路径转换。

Alacritty 的 dropped-file 事件会在路径后附加一个空格；`agentdrop` 会识别这个分隔符，并在替换成远端路径后保留它。

因此普通代码/文本粘贴不会被改写。

> 终端层无法区分“拖拽产生的 paste”和“用户手工粘贴一个本地文件路径”。所以手工粘贴一个真实的本地文件绝对路径也会触发上传，这是预期行为。

如果上传失败，原始 paste 会继续传给 Agent，不会吞掉输入，同时终端会输出一条 `[agentdrop] upload failed` 错误。

## 远端文件位置

每次启动 `agentdrop` 会创建独立目录：

```text
$HOME/.cache/agentdrop/<uuid>/
```

权限通过 `umask 077` 创建。当前不会自动删除这些文件，因为 Agent 可能在会话结束前后仍需要访问它们；可自行周期清理：

```zsh
find ~/.cache/agentdrop -mindepth 1 -maxdepth 1 -type d -mtime +7 -exec rm -rf -- {} +
```

## 当前限制

- 只自动上传普通文件，不上传目录。
- 依赖 Agent/TUI 开启 bracketed paste；Codex、Claude 等现代 TUI 通常会启用。
- 辅助上传连接需要能独立完成认证。
- 文件名需要能够表示为 UTF-8。
- 本项目不会解析 Agent 私有协议；它只把远端绝对路径插入当前输入流，因此可复用于不同 Agent CLI。

## 开发

```text
cargo test
cargo build --release
```

CI 在 Windows、macOS 和 Ubuntu 上执行 Rust 测试，并分别在 Windows/macOS job 构建 release binary artifact。

## License

MIT
