# MyPaste 调研报告

> 调研日期：2026-11-23
> 目的：在立项前了解竞品格局、技术选型和平台限制，为 `plan.md` 提供依据。
> 说明：数据来自公开资料（GitHub README、博客实测、官方文档），标注了来源。凡是写着"待验证"的地方，都要在 M0 技术验证阶段亲手测过再下结论。

---

## 1. 参照对象：Paste（pasteapp.io）

- 闭源付费的剪贴板管理器，只支持 Mac、iPhone、iPad，按订阅收费。
- 核心能力：无限历史、搜索、Pinboards（分组固定）、iCloud 多设备同步、隐私规则（忽略密码、忽略指定应用）、团队共享、AI 上下文集成。
- iPhone 端**不能后台自动采集**，只能靠打开 App、分享扩展或键盘扩展。这是 iOS 的系统限制，不是 Paste 自己的问题（见第 4 节）。

**启示**：Paste 的价值不在"记录"，而在"多设备无缝同步 + 优雅的取回体验"。开源世界里，同时把这两件事做好、又真正轻量的产品还不多。

---

## 2. 开源竞品格局

| 项目 | 技术栈 | 平台 | 热度 | 特点 / 短板 |
|---|---|---|---|---|
| **Maccy** | Swift 原生 | 仅 macOS | ~21.9k★ | 极简、键盘优先，是"轻量"的标杆；只有 macOS，没有同步 |
| **CopyQ** | C++ / Qt | Win/Mac/Linux | 老牌 | 功能最强，支持脚本；界面陈旧，上手门槛高 |
| **Ditto** | C++ / Win32 | 仅 Windows | 老牌 | 稳定，支持局域网同步；界面老旧 |
| **EcoPaste** | Tauri 2 + React | Win/Mac | ~7.6k★ | 国内知名度最高；**重构后放弃 Linux**，没有同步 |
| **TieZ** | Tauri 2 + React | Win/Mac | ~3.1k★ | WebDAV / MQTT 同步、标签、Mica 效果；GPL-3.0 |
| **CrossPaste** | Kotlin Compose MP | Win/Mac/Linux | — | 局域网 E2E 同步是核心卖点，自带 OCR、CLI、MCP；JVM 内存占用高 |
| **PasteBar** | Tauri + React | Win/Mac | — | 片段、模板、表单功能多；协议是 CC BY-NC，**不是真正的开源协议** |
| **Ortu** | Tauri + Rust | Win/Mac/Linux | — | 粘贴栈（按顺序连续粘贴）、敏感信息加密 |
| **Ropy** | **Rust + GPUI (gpui-kit)** | Win/Mac/Linux | — | **和我们选的技术路线完全相同**；明确表示不做插件、不做云同步 |
| Clipboard Desktop (muutot) | Tauri 2 + Svelte 5 + Tantivy | 全平台 | ~90★ | README 里有一张非常详细的各平台能力矩阵，值得参考 |

来源：各项目 GitHub README；dev.to "Best Clipboard Managers in 2026"；SourceForge 剪贴板管理器分类。

### 观察

1. **Tauri 已是红海**：2025–2026 年新出的跨平台剪贴板工具，十个里有八九个是 Tauri + React/Svelte，功能高度雷同（历史、搜索、收藏、托盘、热键）。
2. **轻量做到极致的只有原生方案**：Maccy、Ditto 是单平台；跨平台又真正轻量的，目前只有 Ropy 一家，而且它刻意不做同步和扩展。
3. **同步做得好的都偏重**：CrossPaste（JVM）、TieZ（MQTT/WebDAV，依赖外部服务）。目前还没有"轻量原生 + 免服务器 E2E 同步"的组合。
4. **Linux 是"深坑"**：EcoPaste 直接放弃了 Linux；其他项目在 Wayland 上也普遍打折（见第 4 节）。
5. **键盘流普遍做得浅**：大多只有"热键唤起 → 方向键 → 回车"。粘贴队列只有 Ortu 做了；前缀过滤、快速编号粘贴、连续粘贴这些能力还没有形成体系。

**结论：差异化方向定为"原生轻量（GPUI）+ 免服务器 E2E 多设备同步 + 极致键盘流"，这个组合在开源世界里目前是空白。**

---

## 3. UI 技术选型

### 3.1 内存实测数据

剪贴板工具一直常驻后台，**空闲内存是最核心的轻量指标**。

| 来源 | 场景 | 结果 |
|---|---|---|
| lleg.dev（2026-11，Win11） | 空窗口 + 500 行文本 | **Tauri 2（WebView2）108MB ≈ Electron 103MB**。WebView2 自带浏览器、GPU、渲染等多个进程，Tauri 只是借用了系统里的 Chromium |
| wolinski.com（2026-11，macOS） | 空窗口 | Tauri 59.5MB，Electron 86MB；安装包 7.3MB 对比 268.7MB |
| zenn.dev（2026-08，M1，15 个框架） | 中等复杂度应用 | 原生 / Rust 框架（Slint、egui、iced、Qt、GTK）在 100–170MB 区间；WebView 类（Tauri、Wails、Electron）在 300–700MB 区间 |
| anomalyco/rune 调研（2026-07） | 小型 todo 应用 | 二进制大小：GPUI 4.3MB、Tauri 6.4MB、Iced 8.5MB、Slint 13.2MB；活跃状态 RSS：GPUI 79MB、Tauri 211MB（含辅助进程） |
| gpui-starter 博客（作者自测） | 空闲 | GPUI 15–40MB，Tauri 30–60MB（macOS） |

**结论**：在 Windows 上，Tauri 的"轻量"主要体现在安装包大小，常驻内存并不比 Electron 少。想做到真正轻量，需要原生渲染。

### 3.2 候选方案对比

| 维度 | Tauri 2 + Web 前端 | **GPUI (gpui-kit)** | Slint | 各平台原生 UI + Rust 核心 |
|---|---|---|---|---|
| 空闲内存（Windows） | ~100MB+ | **~15–40MB** | 很低 | 最低 |
| 渲染 / 动画 | 浏览器级，非常灵活 | GPU，120fps | 一般 | 原生 |
| 组件生态 | 最丰富 | gpui-kit 75+ 组件（含虚拟列表、Markdown、代码高亮） | 中等 | 每个平台都要写一遍 |
| 成熟度 | 稳定 | **1.0 之前，API 会变** | 稳定 | 稳定 |
| 移动端 | 支持 | **不支持** | 支持 | 支持 |
| 开发效率 | 高 | 中（UI 也要用 Rust 写） | 中 | 低 |
| 差异化 | 和竞品同质化 | **明显** | 一般 | 明显 |

**决策：桌面端用 GPUI（gpui-kit）。**（用户已确认，2026-11-23）

### 3.3 GPUI 生态现状（2026-11）

- **官方 `gpui` crate 自 0.2.2（2025-10）起没有更新**，社区分成了三条线，**彼此类型不兼容，不能混用**：
  - `gpui-kit`（Longbridge，Apache-2.0）：固定使用 `gpui-pre` 快照，有商业产品 Longbridge Pro 在生产环境使用，约 1.4 万星。**推荐选这条线**。
  - `gpui-ce`（社区版）：增加了模糊滤镜等能力，但已经开始做不兼容改动。
  - `gpui-unofficial` / `gpuikit`：规模较小。
- Windows 后端：Win32 + Direct3D 11 + DirectWrite，支持 IME、AccessKit 无障碍、Mica 背景、透明窗口。
- 窗口类型：`Normal / PopUp / Floating / Dialog / AnchoredPopup`；可以设置 `show(false)` 预先创建隐藏窗口，设置 `focus(false)` 打开时不抢焦点。
- 托盘：gpui-kit 本身没有托盘。可用 `gpui-tray`（支持 gpui-kit feature，Windows 走 `Shell_NotifyIconW`），也可以像 Ropy 那样用 `tray-icon`。
- 参考实现：**Ropy**（MIT 协议，gpui-kit 0.9 + clipboard-rs + redb + tray-icon），可以直接学习它的窗口、托盘、热键处理方式。

来源：gpui-kit README / docs.rs；64p.org GPUI Kit 笔记；gpui-tray README；ropy README 与 PR。

---

## 4. 平台限制（决定了能做什么、不能做什么）

### 4.1 Windows（首发平台）

| 能力 | 实现方式 | 备注 |
|---|---|---|
| 监听剪贴板变化 | `AddClipboardFormatListener` + `WM_CLIPBOARDUPDATE`（事件驱动） | 需要一个独立的消息窗口线程 |
| 防止重复记录 | `GetClipboardSequenceNumber` | 处理"一次复制写入多种格式"时的竞争 |
| 读取格式 | `CF_UNICODETEXT`、`HTML Format`、`Rich Text Format`、`CF_HDROP`（文件）、`PNG` / `CF_DIBV5`（图片） | |
| **尊重隐私标记** | 检查 `ExcludeClipboardContentFromMonitorProcessing`、`CanIncludeInClipboardHistory = 0` 等格式 | 1Password、KeePass 等密码管理器会写入这些标记，**必须遵守** |
| 识别来源应用 | `GetClipboardOwner` / 前台窗口 → 进程路径 → 图标 | 有时拿不到 |
| 粘贴到目标应用 | 记住唤起前的前台窗口 → 隐藏自身 → `SetForegroundWindow` → `SendInput(Ctrl+V)` | **M0 重点验证**：焦点还原的可靠性 |
| 全局热键 | `RegisterHotKey`（可用 `global-hotkey` crate） | `Win+V` 被系统占用 |
| 毛玻璃效果 | Mica / Acrylic（Win11） | GPUI 已支持 |

### 4.2 macOS / Linux（第二阶段）

- **macOS**：没有剪贴板变化通知，只能轮询 `NSPasteboard.changeCount`（常见间隔 250–500ms）；模拟粘贴需要辅助功能权限。
- **Linux X11**：用 XFixes 事件监听，没有问题。
- **Linux Wayland**：后台读取剪贴板依赖 `ext-data-control-v1` 协议。

  | 合成器 | 支持情况 |
  |---|---|
  | KWin（KDE）、Sway、Hyprland、niri、COSMIC | ✅ 支持 |
  | **Mutter（GNOME）** | ❌ **不支持**，也没有支持计划。只能做 Shell 扩展，或者通过 XWayland 绕过 |

  来源：clipmer.app Wayland 合成器对照表（2026-11 复核）；linuxstans.com。

### 4.3 移动端

| 平台 | 限制 | 可行形态 |
|---|---|---|
| **Android 10+** | 只有**当前默认输入法**或**前台应用**能读剪贴板；Android 12+ 读取时会弹提示；Android 13+ 会自动清空剪贴板 | ① 做成**输入法（IME）**，就能合法地自动采集 + 直接粘贴 ② 普通 App：打开时采集 + 分享菜单 + 通知栏快捷操作 |
| **iOS** | 不能后台监听；读取剪贴板会弹系统授权；自定义键盘在密码框等安全字段里会被系统键盘替换 | App 打开时采集 + **分享扩展** + **键盘扩展**（从同步历史里粘贴） |

来源：Android Developers "Secure Clipboard Handling"；clipboardhistory.app；copiedpro.com（Paste、Copied 的 iOS 行为说明）。

**结论**：移动端定位为"**同步伙伴**"，而不是"后台采集器"。移动端的价值在于：在手机上查看、搜索、粘贴电脑上复制过的内容，并把手机上的内容主动推送到电脑。GPUI 不支持移动端，所以移动端要用原生 UI，共享 Rust 核心（见 plan.md）。

---

## 5. 关键依赖调研

### 5.1 剪贴板读写

| crate | 监听变化 | 支持格式 | 备注 |
|---|---|---|---|
| `clipboard-rs` | ✅（Windows 事件驱动 / macOS 轮询 / X11 XFixes） | 文本、HTML、RTF、PNG、文件列表、自定义格式 | 国内作者维护，Ropy 在用；Wayland 不支持；iOS 处于 Beta |
| `arboard` | ❌ | 文本、HTML、图片 | 不能监听；Windows 上多线程并发访问容易冲突 |
| `windows` / `windows-sys` | 自己实现 | 全部 | Windows 上**最终建议自己封装**：隐私标记检查、序列号、来源应用这些细节，通用库都不管 |

**建议**：采集层定义成 `trait`，Windows 实现直接调用 `windows` crate（Win32 API），其他平台先用 `clipboard-rs` 兜底，以后再逐步替换成原生实现。

### 5.2 存储与搜索

- **SQLite**（`rusqlite`，bundled，WAL 模式）+ **FTS5 trigram 分词器**：可以做中文子串搜索。trigram 要求查询至少 3 个字符，1–2 个字符的查询退回到 `LIKE` 并只扫描最近的记录。
- 模糊排序：`nucleo`（Helix 编辑器使用的模糊匹配库），对候选结果做二次打分。
- 备选：`tantivy`（muutot 用了它，8 万条记录 P95 < 0.1ms）。但它要额外维护一份索引，**M1 先不用**，等 SQLite 方案在数据量上出现瓶颈再考虑。
- 大对象（图片、大段文本）：按内容哈希（blake3）存成文件，数据库里只存引用和缩略图。

### 5.3 同步：iroh

- **iroh 1.0 已于 2026-06-15 发布**，线协议和 API 都承诺稳定。
- 按公钥寻址；QUIC + TLS 1.3 端到端加密；自带 NAT 穿透，穿透失败时走中继兜底，中继只转发密文；支持局域网发现（不联网也能连）。
- **官方提供 Swift / Kotlin 绑定**，移动端可以直接使用。
- 中继服务器开源，可以自己部署；官方公共中继有限速。

**结论**：同步层采用 iroh。局域网直连免费且零配置，跨网络时可以选用公共中继或自建中继。不需要我们自己运营服务器。

### 5.4 扩展性

| 方案 | 适用场景 |
|---|---|
| 内置 Action / Transformer 管线 | M1 就做好架构，是所有扩展能力的基础 |
| CLI + 本地 IPC（命名管道） | 让外部脚本、AutoHotkey、启动器能够集成 |
| **Extism（WASM 插件）** | 插件可以用 Rust、JS、Go、C# 等语言编写，运行在沙箱里，能力需要显式授权。放到 v2 |
| gpui-kit 的 `gpui-shell`（JS 扩展运行时） | 如果将来需要插件自带 UI 面板，可以评估 |
| MCP Server | 把剪贴板历史作为上下文提供给 AI 工具（CrossPaste、Paste 都做了），放到后期 |

---

## 6. 风险清单

| 风险 | 影响 | 应对 |
|---|---|---|
| GPUI 在 1.0 之前 API 频繁变动；gpui-kit 固定使用 `gpui-pre` 快照 | 升级会有痛苦 | 锁定版本；**核心逻辑完全不依赖 UI 层**；UI 只是一层薄壳 |
| GPUI 在 Windows 上的**中文输入法**、高 DPI、多显示器表现 | 搜索框没法用就是致命问题 | **M0 第一项验证** |
| GPUI 依赖 D3D11，远程桌面、虚拟机、老显卡可能有兼容问题 | 部分用户打不开 | M0 验证 RDP 场景；必要时记录为已知限制 |
| `SendInput` + 全局热键 + 未签名的二进制 | 被杀毒软件误报 | 申请代码签名（如 SignPath 的开源免费签名，待确认）；避免使用低级键盘钩子 |
| 焦点还原失败，粘贴到了错误的窗口 | 体验很差 | M0 重点验证；失败时退化为"只复制到剪贴板 + 提示" |
| 移动端系统限制 | 无法和桌面端功能对等 | 产品定位明确为"同步伙伴" |
| 同步冲突、数据损坏 | 丢失数据 | 采用只追加的操作日志 + 墓碑；先做单向推送再做双向 |
