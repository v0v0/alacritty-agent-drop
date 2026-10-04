# v2：设计评审与实现决策

## 目标

用户在本机 Alacritty 中连接远端开发机，Agent 已运行在 tmux 前台。用户希望拖入文件或截图后直接在 Agent 中使用，不离开当前输入框、不打断 Agent、不重复输入 SSH 密码。键盘、tmux 快捷键和已有 shell function 继续工作。

审查基线：`main` 的 `d2d3d93`（0.3.0）。v2 基于该提交建立，未合并到 main。

## 现有实现的评价

| 环节 | 现有设计及代码事实 | 对用户的影响 | v2 决策 |
| --- | --- | --- | --- |
| 本机终端 | connect 继承 tssh 的 stdin/stdout，已去掉本机 PTY | 避开早期 ConPTY/raw mode 的键盘兼容性问题 | 保留 |
| 远端 Agent | proxy 为 Agent 创建 Unix PTY，解析 bracketed paste / Ctrl-V | 不依赖 Agent 私有接口，但必须维护终端语义 | 保留最小边界并加入真实 PTY 验证 |
| 上传 | connect::upload_local_file 每次调用第二条 `tssh --upload-file`，远端执行 `trz` | 再认证、再建连接、额外安装 trz，配置可能与主连接冲突 | 在已有反向转发流中直接返回文件 |
| 会话发现 | proxy 扫描 `/tmp/agentdrop-*.sock`，按修改时间尝试，业务错误也换 socket | 多电脑/连接时可能取错文件或剪贴板，显式 socket 设置繁琐 | 单一显式 socket:token 绑定；绝不扫描或跨连接重试 |
| 启动 | 先 connect，再 proxy；zsh function 需独立入口 | 每次多一步，难以放进固定终端启动命令 | 新增 `run HOST -- AGENT` 和 `--tmux NAME` |
| 文件输入 | 单路径识别，简单去首尾引号 | 多文件、Finder 转义和带空格文件名处理不完整 | 保守解析路径列表，输出路径单独引用 |
| 协议 | `read_line` 后才检查 64 KiB；响应不限制长度 | 超长请求已分配内存；不可控输入可拖垮进程 | 读取前限额、文件限额、连接限额、超时 |
| 缓存 | 上传端返回路径，proxy 不保证完整文件存在 | 传输和路径注入间缺乏远端落盘边界 | 接收端创建私有文件、校验长度、sync、rename 后注入 |
| 测试 | 大多是解析和构造测试，控制键测试仅验证 writer | 难以发现实际 PTY 或 tmux 生命周期问题 | 真正执行输入处理、TCP 传输、PTY 和 tmux 重连 |

## 备选方案

| 方案 | 优点 | 代价 / 适用边界 | 结论 |
| --- | --- | --- | --- |
| 换成集成远程文件上传的终端 | 应用统一处理本机文件与远端会话 | 更换现有终端、快捷键和运行习惯；能力取决于终端 | 可选产品路线，不作为本仓库实现 |
| 改 Alacritty / 写平台 GUI helper | 能准确知道拖拽事件和本机剪贴板来源 | 多平台分发、终端 fork 或额外常驻进程；维护面大 | 不选 |
| 在本机全量代理 PTY | 可拦截任意输入 | 重复此前 Windows 终端协议问题 | 不选 |
| 保留副 SSH/SFTP 上传，仅增加命令包装 | 改动小 | 仍依赖独立认证与第二条连接 | 未解决核心摩擦 |
| 已有 SSH 转发上传 + 远端最小 proxy + 一键启动 | 复用现有栈和认证，不增加桌面进程或文件传输服务 | 仍需两端安装 agentdrop，远端 TUI 依赖 PTY | **采用** |

官方能力依据：[tssh](https://github.com/trzsz/trzsz-ssh) 支持 RemoteForward、Unix socket、StreamLocalBindMask；[tmux 手册](https://man.openbsd.org/tmux.1) 提供 session environment、new-session -e 和 attach-session。v2 不要求修改 sshd 的 AcceptEnv，通过受引用的远端启动命令传入绑定。

## 数据和生命周期

1. connect / run 生成随机 socket、token，绑定本机 loopback 端口。
2. 主 tssh 连接建立 0600 的远端 Unix socket 转发；失败则退出。tssh 独占本机终端输入。
3. 远端启动 shell / proxy，继承单一 `AGENTDROP_BRIDGE=socket:token`。所有命令参数按 POSIX shell 单引号规则传递，不 eval 用户输入。
4. Agent 的文件粘贴或 Ctrl-V 触发 proxy 请求。已存在的远端 Unix 路径优先保持原义。
5. proxy 在一个 socket 流中发送版本、token、操作。bridge 验证后打开普通文件或读取剪贴板图片。
6. bridge 返回有界 JSON 头（文件名和字节数），紧接原始二进制；不 base64，不整文件装入内存，不建立额外 SSH 连接。
7. proxy 创建私有请求目录，按声明长度接收，要求 EOF，sync 后 rename，再向 Agent 注入 bracketed paste 路径。
8. 无剪贴板图片：透传 Ctrl-V；请求失败：错误提示并保留原输入；中断：删除半成品。

头版本为 3。主 SSH 已提供传输加密和完整性；长度和 EOF 校验检测应用层截断/越界。文件传输使用有界 copy 缓冲；截图编码依然需要本机 RGBA 图像内存。文件必须在传输期间保持稳定；不提供可变文件快照语义。

## tmux 路由（更新）

默认推荐被动集成，完整说明见 [不接管 tmux 的方案](tmux-independent.md)。Linux proxy 每次只读查询客户端 PID 和该进程的 `/proc` 环境；普通 tmux attach、旧 pane 和重新连接均无需管理 session/window 或 set-environment。`agentdrop init zsh` 提供可选的自动 Agent 包装。

以下托管流程仅在用户明确使用 `--tmux` / `attach` 时适用：


`run --tmux NAME` 调用远端 `attach`：新建专用 session 或连接已有且分离的 session，设置该 session 的单一绑定，然后 attach。已有 Agent 不重启。

Linux proxy 每次从当前唯一客户端进程读取绑定；非 Linux 远端的旧托管路径仍读取 session environment。一个 session 只支持一个附着客户端；多个客户端时拒绝自动路由。不同电脑/任务使用不同 session 名。

`--bridge socket:token` 是高级显式覆盖选项，绕过自动 session 选择；调用者负责指定正确来源。Linux 支持普通手工 tmux attach，完全不需要写入 tmux 环境；非 Linux 的被动模式尚未实现。嵌套 SSH、嵌套 tmux、共享/链接窗口和多客户端协同不是本次支持目标。

## 权限与故障边界

- token 解决无绑定的请求和意外错连，不把同用户进程变成互不信任的主体。启动参数和环境中包含绑定，能够读取它们的本机/远端进程应视为可信。
- `--allow-root` 对 canonical path 做范围检查；`--no-clipboard` 独立关闭剪贴板。不是本机恶意并发进程/路径替换攻击的完整沙箱。
- 缓存目录拒绝 symlink、非当前用户所有或过宽权限；每次请求使用 UUID 和 create_new。文件名不允许路径分隔符或控制字符。
- 8 个并发请求、64 KiB 控制头、256 MiB 单文件、30 秒传输停顿超时；32 个路径、8 MiB 路径解析缓冲、64 个输入块队列。
- 大文件会让后续输入等待，保持粘贴与按键顺序；v2 未实现异步进度 UI、传输取消或断点续传。
- 本机截图临时文件在正常请求结束清理；强制终止可能留下文件。远端缓存由用户确认无引用后清理。
- 当前 CLI 仅传输 UTF-8 名称的普通文件，不读取剪贴板文字、不列举本机目录。

## 验收

自动检查：

- 参数保真与 shell 引用；单 / 多路径和 Unicode、空格、UNC、Finder 转义。
- 二进制内容、错误 token、受限根目录、超长头、文件大小、路径穿越、缓存 symlink、私有权限、截断/超长流清理。
- 普通按键、方向键、Ctrl-A/E/R、文本中的 Ctrl-V、跨 read 的 bracketed paste、未完成/超大文本粘贴透传。
- 真正的 PTY 中启动测试 Agent，确认收到的粘贴路径能读到原始二进制；无剪贴板图片时保留 Ctrl-V；失败时不留下半成品。
- 真正的 tmux 创建、拒绝接管已附着 session、分离后重连，同一存活 proxy 改向新 bridge。
- Windows、macOS Intel / Apple Silicon、Ubuntu 的 Rust 测试和构建。

桌面手工验收（CI 不能代替）：

1. Windows Explorer / macOS Finder 拖单文件和多文件到 Alacritty 中的 Codex / Claude；检查路径与图片理解结果。
2. 系统截图 → Ctrl-V，检查远端 PNG；无图片 → Ctrl-V 保留 Agent 行为。
3. 中文、空格、单引号路径；已有远端路径；普通多行代码文本；方向键、Esc、Ctrl-A/E/R、tmux 前缀键。
4. 密码认证主连接建立后多次上传，不再出现第二次 SSH 认证。
5. 网络断开后重连同一 tmux session，Agent 进程继续；两个不同 session 不串文件/剪贴板。
6. 大文件、传输中断、转发禁用、远端 agentdrop 版本不一致，给出错误且不发布半文件路径。

桌面验收尚不能由本次云端执行环境宣称完成。
