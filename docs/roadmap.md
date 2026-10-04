# v2 后续完善项

这些是对现有实现的具体差距，不表示已经实现。

| 优先级 | 项目 | 当前证据 / 差距 | 完成标准 |
| --- | --- | --- | --- |
| P0 | 真实桌面与 SSH 验收 | tests/e2e.py 验证了 PTY、模拟 bridge、真实 tmux；尚未验证真实 tssh RemoteForward、Alacritty 拖拽、Windows/macOS 系统剪贴板和 Codex/Claude 识图 | 两个平台各完成单/多文件、截图、中文空格路径、快捷键、密码登录后重复上传、断网重连；记录工具版本 |
| P1 | tmux 重连竞态 | session::attach 的检查客户端、set-environment、attach 是分步调用；两个并发 attach 可能通过同一检查 | 以 tmux 锁/原子化流程实现唯一连接接管，并测试同时重连；当前多客户端拒绝上传仍保留 |
| P1 | 上传进度和取消 | proxy::forward_event 在输入处理线程同步等待传输，后续按键排队；无取消键 | 传输时显示进度，可取消，取消后无残留文件，粘贴与后续输入顺序正确 |
| P1 | doctor 诊断与配置 | 失败现在靠错误文本；没有统一检查本机 tssh、远端版本、转发、剪贴板权限和 Ctrl-V 映射 | 一条 doctor 命令给出检查结果和具体修复方法；配置可保存默认 host/session/allow-root |
| P2 | 缓存管理 | 远端文件不自动清理，本机强制结束可能遗留截图；多文件部分成功后失败会留下已完成的缓存 | cache list/prune 支持大小/时间过滤与 dry-run，不自动删除仍在使用的附件 |
| P2 | 安装与分发 | 已有四平台打包；签名、公证、包管理器安装和 Linux ARM64 未实现 | macOS 签名/公证、Windows 签名；按需求补充 Homebrew/winget、Linux ARM64 或 musl |
| P2 | 权限进一步收窄 | --allow-root 是可选配置，token 在同账号信任边界内；不是本机恶意并发进程的沙箱 | 可选保存分享策略、首次分享提示与审计；明确不宣称隔离同 Unix 账号的进程 |

推荐顺序：先完成 P0 桌面验收，再解决 tmux 竞态与传输取消，最后增加 doctor、配置和缓存工具。当前 Release 自动化不会将这些未完成项误标为已验收。
