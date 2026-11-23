<p align="center">
  <img src="assets/brand/png/tessel-128.png" width="96" height="96" alt="Tessel 图标">
</p>

<h1 align="center">Tessel · 拼贴</h1>

<p align="center"><b>每一次复制，都拼进你的工作里。</b></p>

<p align="center">原生、轻量、键盘优先的开源剪贴板工具，支持多设备端到端加密直连同步。</p>

<p align="center">
  <img src="docs/images/popup-overview.png" width="720" alt="Tessel 主界面">
</p>

> 🚧 项目处于 **M1（Windows 本地版）开发中**：剪贴板采集、本地存储、搜索、粘贴已经可用；设置页、自动更新、安装包等还在路上。

## 为什么是 Tessel

- **真正轻量**：基于 GPUI（Zed 编辑器的 GPU 渲染 UI 框架），常驻内存约 **17–24MB**，大约是同等 WebView 方案的 1/4。
- **快**：按 `Alt+V` 到界面画出来约 40ms；10 万条历史中搜索的中位数耗时 0.27ms。
- **键盘优先**：唤起、搜索、选择、粘贴都不用碰鼠标；`Ctrl+1–9` 快速粘贴，`Tab` 切换分类。
- **尊重隐私**：本地优先；遵守密码管理器等应用设置的系统隐私标记，命中时连内容都不读。
- **多设备同步**（规划中）：局域网直连，端到端加密，不需要服务器。

名字的由来：Tessel 源自 *tessellation*（镶嵌）。每一次复制都是一块瓷砖，拼起来就是你的工作轨迹。图标里那块被取出来的瓷砖，就是你正要粘贴的那一条。

## 文档

- 调研报告：[docs/research.md](docs/research.md)
- 项目方案与路线图：[docs/plan.md](docs/plan.md)
- M0 技术验证记录：[docs/m0-report.md](docs/m0-report.md)

## 开发环境（Windows）

- Rust 工具链由 `rust-toolchain.toml` 锁定，需要 **MSVC** 目标（`x86_64-pc-windows-msvc`）
- Visual Studio 2022（或 Build Tools），勾选"使用 C++ 的桌面开发"，并安装 Windows SDK

```sh
cargo run -p tessel                                  # 运行（调试版，带控制台日志；TESSEL_DEBUG=1 输出更详细的日志）
cargo test -p tessel-core -p tessel-clipboard        # 单元测试
powershell -ExecutionPolicy Bypass -File scripts\install-local.ps1   # 编译 release 并安装到本机、创建桌面快捷方式
cargo run -p icon-gen --release                      # 修改 assets/brand/*.svg 后重新生成 PNG / ICO
```

代码结构：

| 目录 | 内容 |
|---|---|
| `crates/tessel-core` | 数据模型、内容识别、SQLite 存储与搜索（不依赖 UI 和平台，可单测） |
| `crates/tessel-clipboard` | 平台剪贴板：事件驱动监听、遵守隐私标记的读取、写回（目前是 Windows） |
| `apps/tessel` | 桌面应用（GPUI）：弹窗界面、全局热键、托盘、粘贴 |
| `spikes/` | M0 技术验证程序，保留作参考 |
| `tools/icon-gen` | 从 SVG 生成各尺寸 PNG 和 .ico |

数据存放在 `%LOCALAPPDATA%\Tessel\`（`tessel.db` + `images/`），可以用环境变量 `TESSEL_DATA_DIR` 换到别的目录。

## 许可证

可任选以下两种协议之一：

- Apache License 2.0（[LICENSE-APACHE](LICENSE-APACHE)）
- MIT License（[LICENSE-MIT](LICENSE-MIT)）

除非你明确声明，否则你有意提交并纳入本项目的任何贡献，都将按上述双协议授权，不附加任何额外条款或条件。
