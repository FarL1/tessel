# MyPaste（代号）

原生、轻量、键盘优先的开源剪贴板工具，支持多设备端到端加密直连同步。

> 🚧 项目处于 **M0 技术验证**阶段，还不能使用。

- 调研报告：[docs/research.md](docs/research.md)
- 项目方案与路线图：[docs/plan.md](docs/plan.md)
- M0 验证记录：[docs/m0-report.md](docs/m0-report.md)

## 开发环境（Windows）

- Rust 工具链由 `rust-toolchain.toml` 锁定，需要 **MSVC** 目标（`x86_64-pc-windows-msvc`）
- Visual Studio 2022（或 Build Tools），勾选"使用 C++ 的桌面开发"，并安装 Windows SDK

```sh
cargo run -p spike-ime-input
```

## 许可证

可任选以下两种协议之一：

- Apache License 2.0（[LICENSE-APACHE](LICENSE-APACHE)）
- MIT License（[LICENSE-MIT](LICENSE-MIT)）

除非你明确声明，否则你有意提交并纳入本项目的任何贡献，都将按上述双协议授权，不附加任何额外条款或条件。
