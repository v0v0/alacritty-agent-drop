# 不接管 tmux 的增强方案

## 使用目标

本机打开连接，远端继续用自己已有的 tmux session/window/pane，再正常启动 codex 或 claude。安装配置一次，以后只需连接、拖拽、截图粘贴。

本次已经实现：Linux 客户端环境被动路由、普通 tmux attach/reconnect、zsh 的可选自动 Agent 包装。未实现原版 tssh 的原生扩展，也不宣称仅安装 trzsz 就能增强正在前台运行的 Agent。

## 为什么不能直接照搬 trzsz 自动拖拽

trzsz-go 的 `uploadDragFiles` 先向输入流写 Ctrl-C（0x03），然后写入 `DragFileUploadCommand`（默认 trz）和回车。这适合 shell 提示符，不适合前台 Agent：可能中断任务，且输入的 trz 命令仍可能被 Agent 当成提示词。

来源：[trzsz-go/filter.go，核对提交 6650842](https://github.com/trzsz/trzsz-go/blob/665084211187c3034a819561a1b6650a45fed4c9/trzsz/filter.go)，[tssh 官方说明](https://github.com/trzsz/trzsz-ssh/blob/main/README.en.md)。因此只配置 `DragFileUploadCommand` 无法消除这个前台执行边界。

## 已采用的实现

1. 本机 `agentdrop connect HOST` 启动 bridge，并直接继承 stdio 启动原生 tssh。没有本机 PTY 代理。
2. 连接携带随机 socket:token，远端登录 shell 继承该连接的 `AGENTDROP_BRIDGE`。
3. 用户自己运行 `tmux attach` / new-session / new-window。agentdrop 不执行这些命令。
4. Agent 启动通过轻量 proxy。`agentdrop init zsh` 输出 shell 包装，在 `.zshrc` 末尾安装后，用户仍输入原来的 `codex` / `claude`。
5. 用户触发拖拽/截图时，proxy 在 Linux 上用 `tmux list-clients -t <pane> -F '#{client_pid}'` 查询唯一客户端，再读取 `/proc/<pid>/environ` 中的连接绑定。
6. 请求前再次查询客户端 PID，发现切换则拒绝本次请求。不要把 pane 环境或 session 最后一次 attach 的变量当成本机来源。
7. 文件通过既有 SSH 转发流上传，完整落盘后注入当前 Agent PTY；上传过程中不向 shell 注入 trz/rz，不发送 Ctrl-C，不切换或新建 tmux 窗口。

[tmux 官方手册](https://man.openbsd.org/tmux.1) 定义了 list-clients 和 client_pid。该路由适用于 Linux 远端：客户端进程是在本次 SSH shell 中启动的，即使 tmux server / pane 已存在，其进程环境也来自本次连接。

zsh 包装的子进程设置 `AGENTDROP_PROXY_ACTIVE=1`，加载 `.zshrc` 时跳过再次包装，让用户原有 function 正常执行。参数通过 positional arguments 传递。把初始化行放在原 function 定义之后；shell alias 不在保证范围内。

## 边界

- tmux session 只有一个附着客户端时自动路由；多个时拒绝，任何一个分离后重新查询剩余客户端。不会拿最后 attach 的 session 环境猜测。
- 客户端没有 bridge、无法读取 `/proc`、客户端切换或 binding 无效时失败，不扫描 socket、不回退其他电脑。
- 不要求修改 `.tmux.conf`、`update-environment` 或 session environment。无需 tmux hook、后台窗口或常驻远端 daemon。
- 已经运行且没有 proxy 的 Agent 不可热增强，需重新启动一次。已有 pane 只需加载一次 zsh 集成。
- 嵌套 tmux、链接/共享窗口、多用户进程隔离、sudo/su 切换账号不属于本次保证范围。
- 本机仍需读取剪贴板的 bridge。若要求界面命令也叫 `tssh HOST`，可以做显式的 shell 包装或后续向 tssh 上游集成；这不等于未修改的 tssh 已支持本协议，不能把包装冒充完全兼容 tssh 的所有 CLI 参数。
- 当前纯 shell 的自动拖拽不是本项目目标；本连接禁用 tssh 原生自动拖拽。普通 shell 传文件仍用手工 trz/tsz；Agent 内自动增强使用本项目 proxy。

## 验证

CI 除原有传输和 PTY 测试，还覆盖：普通 tmux attach、旧 pane 环境、两个原生客户端拒绝取图、分离后剩余客户端的正确来源、原生断开重连，以及已有 zsh function/参数/防递归。真实 Alacritty + 系统剪贴板 + SSH 的桌面验收仍待进行。
