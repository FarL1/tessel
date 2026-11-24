# 工具链评估（2026-11）

结论先说：**整体是现代的，Velopack 是目前 Windows 桌面应用"安装 + 自动更新"的最佳选择之一，建议保留。**需要补的是代码签名和依赖更新的自动化，其中有一项（Velopack 依赖的 `zip` 漏洞）必须在 M2 开启自动更新前处理。

## 现状一览

| 方面 | 现在用的 | 评价 | 说明 / 后续 |
|---|---|---|---|
| 语言 | Rust 2024 edition，工具链锁定 1.99.1（`rust-toolchain.toml`） | ✅ 现代 | 锁定版本保证 CI 和本地一致，升级时单独提交 |
| UI | GPUI（gpui-kit 0.10，精确锁版本） | ✅ 现代，⚠️ 1.0 前 | API 变动频繁，升级要单独提交并回归测试 |
| 存储 / 搜索 | SQLite（rusqlite bundled）+ FTS5 trigram | ✅ | 零外部依赖，单文件，备份方便 |
| 配置 | TOML + toml_edit（保留注释）+ 热加载 | ✅ | "配置即文件"，界面与文本编辑共用一条路径 |
| 安装 / 更新 | **Velopack 1.2.161** | ✅ 现代 | 见下文 |
| 代码签名 | 无 | ❌ 缺 | M2 公开发布前必须解决，见下文 |
| CI | GitHub Actions：fmt、clippy（`-D warnings`）、test、cargo-deny；打标签自动打包 | ✅ | |
| 供应链检查 | cargo-deny（许可证、RustSec 漏洞库、来源） | ✅ 新增 | 已发现 Velopack 依赖的 `zip` 漏洞 |
| 依赖更新 | 手动 | ⚠️ | 建议开启 Dependabot 或 Renovate（Velopack 自己用的就是 Renovate） |
| 提交规范 | Conventional Commits（`feat:` / `fix:` …） | ✅ | 可以直接用 git-cliff 生成 CHANGELOG |
| 编辑器一致性 | `.editorconfig` + `.gitattributes`（LF） | ✅ 新增 | |

## 安装与更新：为什么是 Velopack

| 方案 | 安装体验 | 自动更新 | 增量更新 | 需要管理员 | 维护状况 | 适合我们吗 |
|---|---|---|---|---|---|---|
| **Velopack** | 一键安装，装完即运行 | ✅ 内置（Rust crate） | ✅ | 不需要（按用户安装） | 活跃：1.2.161（2026-09），1.3 预览（2026-10） | ✅ **首选** |
| cargo-packager（CrabNebula） | NSIS / MSI | ✅ 有配套 updater | ❌ | 视配置 | 活跃，跨平台打包强 | 可选；增量更新和"装完即运行"不如 Velopack |
| cargo-dist（axo） | MSI / shell 安装脚本 | 有限 | ❌ | — | 主要面向命令行工具 | ❌ 不适合 GUI 托盘应用 |
| MSIX | 商店风格 | 系统负责 | ✅ | 不需要 | 微软主推 | 必须签名，沙箱限制多（全局热键、剪贴板监听要额外声明），留到上架 Microsoft Store 时再评估 |
| WiX 6 / NSIS / Inno Setup | 传统安装向导 | ❌ 要自己做 | ❌ | 通常需要 | 成熟 | ❌ 更新要从零写 |

Velopack 的优点：

- 一条命令（`vpk pack`）生成安装包、完整更新包、**增量更新包**和便携版；更新只下载变化的部分，约 2 秒内完成并重启，不弹 UAC。
- 核心用 Rust 写，有官方 Rust crate；安装、更新、卸载钩子直接写在 `main()` 里（我们用它在卸载前清理开机自启）。
- 更新源就是一组静态文件，放在 GitHub Releases 上就行，不需要服务器。
- 是 Squirrel.Windows 的继任者，社区下载量大，维护活跃。

需要注意的地方：

1. **`vpk` 依赖 .NET 8 SDK**（只在打包的机器上需要，用户不需要）。CI 里用 `actions/setup-dotnet` 安装。
2. **`vpk` 和 `velopack` crate 的版本必须一致**，目前都锁在 1.2.161（`apps/tessel/Cargo.toml`、`scripts/package.ps1`、`.github/workflows/release.yml`）。
3. **安全公告**：velopack 1.2.161 依赖的 `zip` 8.6 有 8 条已知漏洞（RUSTSEC-2026-0367 等，主要是解压时的路径穿越和符号链接问题）。Tessel 目前只用安装 / 卸载钩子，不调用 `UpdateManager`，走不到解压代码，所以暂时在 `deny.toml` 里逐条忽略并写明了原因。**开启自动更新前必须升级到 velopack 1.3**（依赖 `zip` ^9，可解析到已修复的 9.0.5+），并删除这些忽略项。
4. 安装包里带有 Velopack 的更新程序（约 7MB），所以安装包是 16.9MB，程序本体压缩后只有 9.7MB。
5. **packId 必须是 `Tessel.App`**：Velopack 安装到 `%LOCALAPPDATA%\{packId}`，卸载时整个删除；数据目录是 `%LOCALAPPDATA%\Tessel`，两者不能重合。

## 代码签名（M2 公开发布前）

没有签名的安装包会被 SmartScreen 拦一下（"已保护你的电脑"），对新用户不友好。可选方案：

| 方案 | 费用 | 用户看到的发布者 | 适用条件 |
|---|---|---|---|
| **SignPath Foundation** | 免费 | SignPath Foundation | 开源项目（OSI 许可证，无商业双授权）；签名在它们的 HSM 里完成，和 GitHub Actions 集成 |
| Certum Open Source 证书 | 约 €49 起 | "Open Source Developer" + 你的名字 | 有公开仓库的个人开发者 |
| Azure Artifact Signing（原 Trusted Signing） | $9.99/月 | 你的名字 | 个人只限美国、加拿大，**国内个人用不了** |
| 传统 OV 证书（DigiCert、Sectigo 等） | $150–300/年 | 你的名字 | 全球可用，但私钥通常在 USB 硬件里，不方便 CI |

**建议**：开源后申请 SignPath Foundation（免费，和 CI 集成好）。如果希望发布者显示自己的名字，再考虑 Certum 开源证书。Velopack 的 `vpk pack` 支持签名参数，接入只需要改打包脚本。

另外，无论哪种签名，SmartScreen 信誉都要靠下载量慢慢积累，刚开始仍可能有提示。

## 建议的后续动作

| 时间 | 动作 |
|---|---|
| 创建 GitHub 仓库后 | 开启 Dependabot（`cargo`、`github-actions`），或者用 Renovate |
| M2 开启自动更新前 | 升级 velopack 到 1.3（`vpk` 同步升级），删除 `deny.toml` 里的 `zip` 忽略项；用 `vpk upload github` 发布更新源。2026-11-24 复查：1.3.39 在 GitHub 上仍标为预发布版，暂不升级（未调用 `UpdateManager`，走不到有漏洞的解压代码） |
| M2 公开发布前 | 申请 SignPath Foundation 签名；用 git-cliff 从提交记录生成 CHANGELOG |
| M2 公开发布后 | 提交到 winget（用 Komac 或 wingetcreate 自动生成清单）和 Scoop |
| 可选 | 用 cargo-nextest 跑测试（更快，输出更清楚）；加一个冷启动和内存的性能基准，防止退化 |
