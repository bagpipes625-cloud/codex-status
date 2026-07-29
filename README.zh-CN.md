<div align="center">

# CodexStatus

**在 Windows 托盘里一眼看清 Codex 剩余额度。**

[English](README.md) · [下载](https://github.com/bagpipes625-cloud/codex-status/releases/latest) · [反馈问题](https://github.com/bagpipes625-cloud/codex-status/issues)

</div>

| 浅色 | 深色 |
|:--:|:--:|
| ![CodexStatus 浅色额度卡片](assets/screenshots/codexstatus-light.png) | ![CodexStatus 深色额度卡片](assets/screenshots/codexstatus-dark.png) |

CodexStatus 是一个小巧的原生 Windows 工具。通知区域图标显示用户选择的 5 小时或周剩余额度 `0–100`；不可用时显示 `--`。两个周期均可用时，底部 2px 色条反映另一周期；如果 Codex 只返回一种有效额度，界面会自动切换为紧凑的单额度布局。

本仓库是基于上游
[mmm1h/codex-status](https://github.com/mmm1h/codex-status)
持续维护的社区衍生版本，保留上游 MIT 许可证与版权声明，同时采用独立的版本线和产品取舍。

## 主要特点

- 两种额度均可用时，5 小时和本周额度卡并列显示，可直接点击卡片或从托盘菜单选择；只返回一种额度时自动使用紧凑布局，不显示多余的切换入口。
- 每张卡的外环显示真实剩余额度，蓝灰色内环显示按周期时间推导的理论合理剩余额度，无需新增请求。
- 使用微软雅黑 UI 并适配明暗主题；额度状态采用绿色（≥50%）、琥珀色（20–49%）和红色（<20%）。
- 使用 Direct2D/DirectWrite 绘制抗锯齿圆环和一致文字，保留 GDI
  兜底，并适配浅色、深色、高对比度、多显示器 DPI 和 Windows 10 圆角。
- 可从托盘菜单选择跟随系统、浅色或深色界面主题。
- 只使用官方 Codex app-server RPC `account/rateLimits/read`，不读取 Token，不访问私有接口。
- 纯 Win32 事件驱动；没有 Electron、WebView、WPF、WinUI、本地 HTTP 服务或常驻异步运行时。
- 面板关闭后立即释放窗口尺寸相关的渲染目标；短暂宽限后继续释放
  Direct2D 资源并压缩空闲工作集。
- 默认 5 分钟刷新，支持手动刷新、失败退避、安全缓存过期和可选低额度提醒。
- 仅由用户主动触发的 GitHub Release 更新，区分安装版与便携版产物并校验 SHA-256。
- 单实例、Explorer 重启恢复、多屏定位和开机启动。
- 根据 Windows 自动选择英文或简体中文。

## 安装

需要 Windows 10/11 x64，并已安装且登录 [Codex CLI 或 Codex 应用](https://developers.openai.com/codex/cli/)。

1. 从 [Releases](https://github.com/bagpipes625-cloud/codex-status/releases/latest) 下载当前用户安装包。
2. 运行安装程序并确认安装目录。全新安装时，如果存在 `F:` 盘则默认使用 `F:\CodexStatus`，否则默认使用 `%LOCALAPPDATA%\Programs\CodexStatus`；升级时沿用已有安装目录。
3. 安装程序始终注册开机启动。
4. 如果 Windows 把新图标放进折叠区，请打开折叠区，把 CodexStatus 拖到可见托盘。图标是否常显由 Windows 和用户控制，应用无法强制固定。

当前安装包尚未代码签名，因此 Microsoft Defender SmartScreen 可能提示“无法识别的应用”。每个 Release 都提供 SHA-256 校验文件。便携 ZIP 默认不会修改开机启动，可从右键菜单自行开启。

## 使用

- **左键：** 打开或关闭额度卡片；两种额度均可用时，可点击对应卡片选择托盘数字展示哪个周期。
- **右键：** 立即刷新、打开 Codex 用量页、选择 1/5/15 分钟刷新、配置低额度提醒、选择主题、切换开机启动、检查更新或退出。
- **托盘数字：** 用户选择的 5 小时或周剩余百分比，四舍五入到整数。双额度模式下状态条跟随未选中的额度；单额度模式下数字和状态条都跟随唯一可用额度。

每次刷新会短暂启动本机 `codex app-server`，完成 `initialize → account/read → account/rateLimits/read` 后，使用 Windows Job Object 关闭整个子进程树。周窗口优先精确匹配 10,080 分钟；否则只接受 6–8 天窗口，绝不会把短窗口误标成周额度。

每张额度卡的外环显示真实剩余百分比；内环根据现有的重置时间和周期长度在本地计算剩余时间比例，不会增加任何网络请求。

## 隐私

CodexStatus 不读取或保存 OAuth Token、邮箱、项目内容、提示词和 app-server 原始响应，也不收集遥测。只有用户选择 **检查更新** 后才会访问 GitHub，不设置后台更新定时器。

`%LOCALAPPDATA%\CodexStatus` 下通常只有两个状态文件：

- `settings.json`：刷新间隔、界面语言、主题、提醒阈值、首次引导和提醒去重状态。
- `snapshot.json`：最近一次经过解析的非敏感额度快照；跨过重置时间后立即停止使用，并在下一次成功刷新时替换文件。

普通版本不写运行日志。只有 Windows 拒绝托盘图标操作时，CodexStatus 才会额外写入一份 `tray-errors.log`，其中仅包含操作名、发布通道、EXE 路径、托盘标识、PID 和 Win32 错误码。显式启用 Cargo `diagnostics` 特性时，还会记录生命周期阶段和过滤后的错误摘要。

## 性能

0.5.0 仍是事件驱动的原生 Win32 程序，在设定的刷新定时器之间不做持续
动画或轮询。面板隐藏后会释放与窗口尺寸相关的渲染目标，经过短暂宽限后
继续释放 Direct2D 资源并压缩空闲驻留页。具体工作集会随 Windows 版本、
DPI、显卡驱动以及近期是否打开过面板而变化。

刷新时只会短暂启动一个本机 `codex app-server` 进程树。它具有 Codex 自身
的瞬时占用，并在 RPC 完成或超时后作为整体关闭，不属于托盘程序的常驻占用。

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

GitHub Actions 会为版本标签分别构建 stable 与 portable 可执行文件、路径无关的便携 ZIP 和 stable 通道 Inno Setup 安装包。本地也可使用 gnullvm 开发工具链；此时 llvm-mingw 的 `libunwind.dll` 只是本地开发依赖，正式 MSVC Release 是单文件程序。

## 首版边界

本项目不做私有任务栏注入、多供应商、Token/成本历史或本地服务。Windows 没有受支持的接口可强制托盘图标常显，因此固定图标始终由用户决定。

## 致谢

本衍生版本基于并感谢上游
[mmm1h/codex-status](https://github.com/mmm1h/codex-status)，其 MIT
许可证与版权声明完整保留在 [LICENSE](LICENSE) 中。其他交互参考包括
[CodexBar](https://github.com/steipete/CodexBar)、
[TaskbarQuota](https://github.com/zioder/TaskbarQuota)、
[CodexQuotaTaskbar](https://github.com/zHysie/CodexQuotaTaskbar)、
[codex-win-widget](https://github.com/Mauriciog87/codex-win-widget) 和
[Claude & Codex Battery](https://github.com/dennykim123/claude-codex-battery)；
紧凑浮层也借鉴了 [Windows 应用设计指南](https://learn.microsoft.com/windows/apps/design/)、
[Twinkle Tray](https://github.com/xanderfrangos/twinkle-tray) 与
[EarTrumpet](https://github.com/File-New-Project/EarTrumpet) 的交互思路。

额度通信遵循官方 [Codex app-server 文档](https://learn.chatgpt.com/docs/app-server#6-rate-limits-chatgpt)，通知区域行为遵循 [Microsoft 指南](https://learn.microsoft.com/windows/win32/uxguide/winenv-notification)。

## 许可证

[MIT](LICENSE)
