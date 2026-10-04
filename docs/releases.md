# Release 发布与安装

## 自动发布规则

推送 `v` 开头的版本 tag，例如 `v0.4.0`，会执行 `.github/workflows/release.yml`：

1. 要求 tag 与该提交的 `Cargo.toml` 版本严格一致；不匹配立即失败。
2. 调用同一份 CI：四平台测试、release 构建、可执行文件冒烟测试、PTY/tmux 集成测试、发布脚本测试和校验文件验证。
3. 收集四个压缩包和校验文件；检查数量、名称和 SHA-256。
4. 创建 GitHub Release 草稿，上传全部附件，再发布。发布说明由 GitHub 自动生成。

`v0.5.0-rc.1` 对应 Cargo 版本 `0.5.0-rc.1`，自动标记为 Pre-release，不设为 Latest。稳定版 Latest 由 GitHub 按日期和版本自动处理。仅支持不带 `+build` 元数据的 SemVer。

工作流只在 tag 事件发布；普通分支提交/PR 会构建验证并保存 Actions artifacts，不会生成公开 Release。发布 job 使用自带的 `GITHUB_TOKEN`，只有它声明 `contents: write`，不需要额外 PAT。

## 发布当前 v2

先确认 tag 所在的提交含有 `release.yml`，且 CI 成功。当前工作流位于 `v2`；给未包含该工作流的旧 main 提交打 tag 不会触发它。

```sh
git switch v2
git pull --ff-only
# 当前 Cargo.toml 版本为 0.4.0
python3 scripts/release.py validate --tag v0.4.0
git tag -a v0.4.0 -m "agentdrop 0.4.0"
git push origin v0.4.0
```

下一个版本先修改 `Cargo.toml`，运行 `cargo check` 更新 `Cargo.lock`，将二者一同提交，再打对应 tag。每次只推一个版本 tag，不要移动已经发布的 tag。

发布结果：[Releases](https://github.com/v0v0/alacritty-agent-drop/releases)。构建日志：[Actions](https://github.com/v0v0/alacritty-agent-drop/actions)。

## 生成的附件

| 平台 | 文件名（以 v0.4.0 为例） | 构建基线 |
| --- | --- | --- |
| Windows x64 | `agentdrop-v0.4.0-x86_64-pc-windows-msvc.zip` | Windows runner，MSVC |
| macOS Apple Silicon | `agentdrop-v0.4.0-aarch64-apple-darwin.tar.gz` | macOS，deployment target 11.0 |
| macOS Intel | `agentdrop-v0.4.0-x86_64-apple-darwin.tar.gz` | macOS，deployment target 10.15 |
| Linux x64 | `agentdrop-v0.4.0-x86_64-unknown-linux-gnu.tar.gz` | Ubuntu 22.04 / glibc 2.35 |

每包内含 `agentdrop`（Windows 为 `.exe`）、README、MIT LICENSE 和 v2 设计说明。每个包有独立 `.sha256` 文件，Release 另附统一的 `SHA256SUMS`。

macOS 最低部署目标是链接配置，并不代表已经在这些旧版系统实机验收。二进制尚未做 Apple Developer 签名/公证或 Windows Authenticode 签名。Linux 包不是 musl 静态包，其他发行版需兼容的 glibc 和系统库；ARM Linux 暂不提供。

## 用户安装

本机和远端分别下载对应架构的同一版本，校验后解压。这样不需要安装 Rust。

Linux（下载包及同名 `.sha256` 后）：

```sh
sha256sum -c agentdrop-v0.4.0-x86_64-unknown-linux-gnu.tar.gz.sha256
tar -xzf agentdrop-v0.4.0-x86_64-unknown-linux-gnu.tar.gz
mkdir -p ~/.local/bin
install -m 755 agentdrop-v0.4.0-x86_64-unknown-linux-gnu/agentdrop ~/.local/bin/agentdrop
~/.local/bin/agentdrop --version
```

macOS 用 `shasum -a 256 -c <压缩包>.sha256` 校验，解压后将 `agentdrop` 放入 PATH。Windows 用 PowerShell `Get-FileHash <zip文件> -Algorithm SHA256` 与 `.sha256` 比较，解压并把 `agentdrop.exe` 所在目录加入用户 PATH。

完成两端安装后，在本机执行：

```sh
agentdrop connect dev
# 远端一次性设置 init zsh 后，正常 tmux attach 和 codex 即可
```

## 失败与重试

- 版本不一致：修正 Cargo 版本和 tag；尚未发布的错误 tag 应按仓库维护规范处理。
- 任一测试、构建或校验失败：不会进入发布 job，先查看失败 job。
- 上传失败：保留草稿，可在 Actions 重跑失败 job。草稿附件允许覆盖重传。
- 已发布：工作流拒绝覆盖附件，修改应使用新版本号。
- tag 被移到其他提交：发布前比对 tag 与构建提交，不一致则拒绝发布。
- 如果组织禁止写入 Release，需要仓库管理员允许该工作流的 `contents: write`；不要在日志中粘贴凭证。

## 验证范围

普通 CI 也运行完整原生构建和压缩包校验，发布脚本通过 CLI 替身测试创建顺序、草稿重试、上传失败、已发布拒绝覆盖和 tag 移动保护。真正向 GitHub 发布的 job 只在推送版本 tag 后执行。
