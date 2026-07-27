<div align="center">

# CodexStatus

**在 Windows 托盘里一眼看清 Codex 周剩余额度。**

[English](README.md) · [下载](https://github.com/bagpipes625-cloud/codex-status/releases/latest) · [反馈问题](https://github.com/bagpipes625-cloud/codex-status/issues)

</div>

| 浅色 | 深色 |
|:--:|:--:|
| ![CodexStatus 浅色额度卡片](assets/screenshots/codexstatus-light.png) | ![CodexStatus 深色额度卡片](assets/screenshots/codexstatus-dark.png) |

CodexStatus 是一个小巧的原生 Windows 工具。通知区域图标本身就是 `0–100` 的周剩余额度数字；暂时没有可信数据时显示 `--`。左键点击可查看重置倒计时、可选的 5 小时窗口、套餐和刷新状态。

## 主要特点

- 在标准系统托盘图标内直接绘制周剩余额度。
- 透明背景、随任务栏明暗切换的 Segoe UI 数字；底部仅用一条细线标记绿色（≥50%）、琥珀色（20–49%）、红色（<20%）或缓存状态。
- 原生圆角卡片，自动适配浅色、深色、高对比度和多显示器 DPI。
- 可从托盘菜单选择跟随系统、浅色或深色界面主题。
- 根据当前周周期已经过时间和已用额度，预测额度是否会提前耗尽。
- 只使用官方 Codex app-server RPC `account/rateLimits/read`，不读取 Token，不访问私有接口。
- 纯 Win32 事件驱动；没有 Electron、WebView、WPF、WinUI、本地 HTTP 服务或常驻异步运行时。
- 默认 5 分钟刷新，支持手动刷新、失败退避、安全缓存过期和可选低额度提醒。
- 单实例、Explorer 重启恢复、多屏定位和开机启动。
- 根据 Windows 自动选择英文或简体中文。

## 安装

需要 Windows 10/11 x64，并已安装且登录 [Codex CLI 或 Codex 应用](https://developers.openai.com/codex/cli/)。

1. 从 [Releases](https://github.com/bagpipes625-cloud/codex-status/releases/latest) 下载当前用户安装包。
2. 运行安装程序。此私有版本固定安装到 `F:\CodexStatus`，并始终注册开机启动。
3. 如果 Windows 把新图标放进折叠区，请打开折叠区，把 CodexStatus 拖到可见托盘。图标是否常显由 Windows 和用户控制，应用无法强制固定。

当前安装包尚未代码签名，因此 Microsoft Defender SmartScreen 可能提示“无法识别的应用”。每个 Release 都提供 SHA-256 校验文件。便携 ZIP 默认不会修改开机启动，可从右键菜单自行开启。

## 使用

- **左键：** 打开或关闭额度卡片。
- **右键：** 立即刷新、打开 Codex 用量页、选择 1/5/15 分钟刷新、配置低额度提醒、选择主题、切换开机启动或退出。
- **托盘数字：** 周剩余百分比四舍五入到整数。

每次刷新会短暂启动本机 `codex app-server`，完成 `initialize → account/read → account/rateLimits/read` 后，使用 Windows Job Object 关闭整个子进程树。周窗口优先精确匹配 10,080 分钟；否则只接受 6–8 天窗口，绝不会把短窗口误标成周额度。

面板底部会根据本周周期开始以来的平均消耗速度进行线性推算。预计耗尽时间不早于重置时间时，以绿色粗体显示 **用量充裕**；预计提前耗尽时，以红色粗体显示距离耗尽的天数和小时数。

## 隐私

CodexStatus 不读取或保存 OAuth Token、邮箱、项目内容、提示词和 app-server 原始响应，也不收集遥测或发起后台网络请求。

`%LOCALAPPDATA%\CodexStatus` 下通常只有两个状态文件：

- `settings.json`：刷新间隔、界面语言、主题、提醒阈值、首次引导和提醒去重状态。
- `snapshot.json`：最近一次经过解析的非敏感额度快照；一旦跨过重置时间立即失效。

普通版本不写运行日志。只有 Windows 拒绝托盘图标操作时，CodexStatus 才会额外写入一份 `tray-errors.log`，其中仅包含操作名、发布通道、EXE 路径、托盘标识、PID 和 Win32 错误码。显式启用 Cargo `diagnostics` 特性时，还会记录生命周期阶段和过滤后的错误摘要。

## 性能

在 Windows 11 24H2 x64、v0.3.0 Release 版上的实测：

| 状态 | CodexStatus 工作集 | CPU | 子进程 |
|---|---:|---:|---:|
| 刷新完成后空闲 | 约 12 MB | 平均 ≤0.1% | 0 |
| 刷新中 | 托盘主进程 <15 MB | 短暂活动 | 1 个临时 `codex app-server` 进程树 |

app-server 是 Codex 本身，刷新时会有更大的瞬时占用；完成两个账户查询后立刻退出，不属于常驻托盘进程。

## 构建

正式发布目标是稳定版 Rust 的 `x86_64-pc-windows-msvc`：

```powershell
cargo fmt --check
cargo clippy --all-targets -- -D warnings
cargo test --all-targets
$env:CODEX_STATUS_CHANNEL = "stable"
cargo build --release --locked
Remove-Item Env:CODEX_STATUS_CHANNEL
```

开发构建默认使用隔离的开发版托盘 GUID。只有打包相应安装通道时才把 `CODEX_STATUS_CHANNEL` 设为 `beta` 或 `stable`；各通道的窗口类和单实例互斥量也彼此隔离。不要直接从构建或暂存目录运行 stable 通道 EXE，应先安装到固定目录，让该 GUID 首次从正式路径完成注册。便携包使用 `CODEX_STATUS_CHANNEL=portable` 和受支持的 `HWND + uID` 标识，因为它的 EXE 路径本来就不固定。

GitHub Actions 会为版本标签构建 stable 通道的便携 ZIP 和 Inno Setup 安装包。本地也可使用 gnullvm 开发工具链；此时 llvm-mingw 的 `libunwind.dll` 只是本地开发依赖，正式 MSVC Release 是单文件程序。

## 首版边界

本项目不做私有任务栏注入、多供应商、Token/成本历史或本地服务。Windows 没有受支持的接口可强制托盘图标常显，因此固定图标始终由用户决定。

## 致谢

交互和信息层级参考了 [CodexBar](https://github.com/steipete/CodexBar)、[TaskbarQuota](https://github.com/zioder/TaskbarQuota)、[CodexQuotaTaskbar](https://github.com/zHysie/CodexQuotaTaskbar)、[codex-win-widget](https://github.com/Mauriciog87/codex-win-widget) 和 [Claude & Codex Battery](https://github.com/dennykim123/claude-codex-battery)；紧凑浮层也借鉴了 [Windows 应用设计指南](https://learn.microsoft.com/windows/apps/design/)、[Twinkle Tray](https://github.com/xanderfrangos/twinkle-tray) 与 [EarTrumpet](https://github.com/File-New-Project/EarTrumpet) 的交互思路。本项目独立实现，没有复制这些项目的源代码。

额度通信遵循官方 [Codex app-server 文档](https://learn.chatgpt.com/docs/app-server#6-rate-limits-chatgpt)，通知区域行为遵循 [Microsoft 指南](https://learn.microsoft.com/windows/win32/uxguide/winenv-notification)。

## 许可证

[MIT](LICENSE)
