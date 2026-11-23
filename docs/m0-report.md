# M0 技术验证记录

> 目的：动手实测，排除 GPUI 技术路线的最大风险。每一项都要写清**测试环境、实测结果、结论**。
> 有任何一项**不达标**，就重新评估技术路线（见 plan.md 第 7 节）。

## 测试环境

| 项 | 值 |
|---|---|
| 系统 | Windows 11 Pro 10.0.26300 |
| Rust | 1.99.1（x86_64-pc-windows-msvc） |
| gpui-kit | 0.10.0（gpui-pre 0.3.12） |
| SQLite | 3.54.1（rusqlite 0.43，bundled） |
| 显示器 / 缩放 | 主屏 150%（其余待填） |
| 输入法 | 待填 |

## 总览

| # | 验证项 | 程序 | 状态 | 结论 |
|---|---|---|---|---|
| 1 | GPUI 中文输入法 / 高 DPI / 多显示器 | `spikes/ime-input` | 🟡 渲染和文本管线已通过；**输入法组字待人工测** | |
| 2 | 隐藏窗口常驻内存、热键唤起延迟、冷启动 | `spikes/popup-paste`、`spikes/startup-probe` | 🟡 唤起延迟 ✅；**内存、冷启动未达标** | 见下文分析 |
| 3 | 远程桌面、虚拟机下 D3D11 能否运行 | 复用 #1 | ⚪ 未开始 | |
| 4 | 剪贴板全格式采集 + 隐私标记 | `spikes/clip-capture` | ✅ 通过 | 事件驱动可行；**必须防抖合并通知** |
| 5 | 焦点还原 + SendInput 粘贴成功率 | `spikes/popup-paste` | 🔴 **Enter 没有触发粘贴**，待排查 | |
| 6 | 托盘 + 全局热键 + Mica | `spikes/popup-paste` | 🟡 全局热键 ✅；托盘、Mica 视觉效果未验证 | |
| 7 | SQLite FTS5 trigram 10 万条搜索耗时 | `spikes/search-bench` | ✅ 通过（方案 B） | **rowid 按时间排序 + 长短词分流**，P50 0.27ms |

---

## #1 GPUI 中文输入法

运行：`cargo run -p spike-ime-input`（release 模式：`cargo run -p spike-ime-input --release`）

### 已自动验证（2026-11-23）

- ✅ 150% 缩放下渲染清晰；中文、英文、日文、代码、全角标点混排都正确；输入框焦点环正常显示。
- ✅ 窗口打开后焦点直接落在搜索框（不用点击）。
- ✅ 通过 `WM_CHAR` 向窗口投递「验证码」三个字：正确上屏，Change 事件 3 次，过滤结果 1 条且正确。
  - 这只验证了**文本管线**（UTF-16 → 输入框 → 事件 → 过滤），**没有**经过输入法组字流程。
- ℹ️ debug 构建首次启动到窗口出现需要好几秒（依赖代码未做 LTO，着色器首次编译）。启动耗时以 release 构建为准，见 #2。

### 人工测试清单

每一项填 ✅ / ⚠️ / ❌，有问题的附截图或现象描述。

| 测试项 | 微软拼音 | 其他输入法：____ |
|---|---|---|
| 打开窗口后无需点击，直接输入中文 | | |
| 候选框出现在光标附近（不是跑到屏幕左上角） | | |
| 组字期间（拼音还没上屏）不触发过滤，上屏后才过滤 | | |
| 上屏后列表正确过滤（试试：剪贴板、同步、验证码） | | |
| 中英文混输、中英切换（Shift） | | |
| 全角 / 半角标点 | | |
| Emoji（Win + . 面板） | | |
| 光标左右移动、Home/End、Shift 选中、Ctrl+A | | |
| Ctrl+C / Ctrl+V / Ctrl+X / Ctrl+Z | | |
| Backspace 删除组字中的拼音、删除已上屏的汉字 | | |
| 多行输入区：换行、长文本自动折行、滚动 | | |
| 缩放 100% / 125% / 150% 下文字清晰、候选框位置正确 | | |
| 把窗口拖到另一台缩放比例不同的显示器 | | |
| 状态栏的 Change 事件次数是否合理（不应每敲一个拼音字母就触发） | | |

### 观察记录

（在这里记录问题、截图和 workaround）

### 结论

（待人工测试后填写）

---

## #4 剪贴板采集（✅ 通过）

运行：`cargo run -p spike-clip-capture`，然后在任意应用里复制内容。

测试方法：用 PowerShell（.NET `Clipboard`）依次写入 6 种内容，观察监听程序的输出。

| 场景 | 读到的格式 | 结果 |
|---|---|---|
| 纯文本（中英混合） | `CF_UNICODETEXT`、`CF_LOCALE`、`CF_TEXT`… | ✅ 24 字符，内容正确 |
| HTML + 文本 | `HTML Format` + `CF_UNICODETEXT` | ✅ 两种都读到了 |
| 图片（.NET Bitmap） | `CF_BITMAP`、`CF_DIB`、`CF_DIBV5`（**没有 PNG**） | ✅ DIBV5 250KB |
| 文件列表 | `CF_HDROP`、`FileNameW` | ✅ 2 个路径正确 |
| `ExcludeClipboardContentFromMonitorProcessing` | — | ✅ 识别并跳过，没有读取内容 |
| `CanIncludeInClipboardHistory = 0` | — | ✅ 识别并跳过 |
| 来源应用 | 剪贴板所有者窗口 → 进程路径 | ✅ 正确识别为 powershell.exe；所有者为空时退回前台窗口 |

### 关键发现

1. **每次复制会触发 2 次 `WM_CLIPBOARDUPDATE`**（序列号相差 2）。OLE 写剪贴板分两步（`OleSetClipboard` + `OleFlushClipboard`），很多应用都是这样。
   - 不做处理时：第一次通知到来时源应用还占着剪贴板，打开要重试 1–3 次，耗时 6–18ms，而且同一内容会被读两遍。
   - **加 40ms 防抖后**：12 次通知合并成 6 次，重试 0 次，读取耗时 **0.8–6ms**。
   - → **正式实现必须防抖**（`SetTimer` 重置计时），并且在管线层再按内容哈希去重，作为第二道保险。
2. **图片格式不统一**：.NET 只提供 DIB/DIBV5，浏览器通常提供 PNG。管线里需要把 DIBV5 转成 PNG 再存储（注意 alpha 通道和 BI_BITFIELDS）。
3. **只读需要的格式**：枚举格式名不会触发延迟渲染，但 `GetClipboardData` 会。对于不认识的格式只记名字、不读数据。
4. 隐私标记检查用 `IsClipboardFormatAvailable` 即可，不需要打开剪贴板，命中后连内容都不碰。

### 待补测

- 真实应用：Chrome（PNG + HTML）、Office（RTF + 大量私有格式）、微信、资源管理器复制文件、截图工具、1Password/KeePass。
- 大内容：10MB 文本、4K 截图的读取耗时（应放到后台线程处理）。

---

## #7 SQLite FTS5 trigram 搜索（✅ 通过，采用方案 B）

运行：`cargo run -p spike-search-bench --release [-- 条数]`

数据：10 万条合成数据，中英混合、链接、代码，平均 128 字符。合成词库很小，热门词的命中率约 20%，比真实数据**更悲观**。

### 基础数据

| 项 | 结果 |
|---|---|
| 批量写入 10 万条 | 16.4s（约 6 条/ms，只有导入时才会用到） |
| 库文件大小 | 73MB（原始文本约 13MB，trigram 索引大约是原文的 5 倍） |
| 单条增量写入（含 FTS） | P50 0.32ms · P95 1.5ms · max 68ms（WAL checkpoint）→ 必须放在后台线程 |

### 方案 A：常规写法（❌ 不达标）

| 查询方式 | P50 | P95 |
|---|---|---|
| FTS 按相关度排序 Top50 | 21ms | 37ms |
| FTS + JOIN 按时间倒序 Top50 | 38ms | **52ms** ❌（目标 30ms） |

问题：热门词命中 2 万条，SQLite 要先取出全部命中项再排序。

**还发现一个功能缺陷**：`同步 设备` 返回 **0 条**。trigram 至少要 3 个字符，**2 个字的中文词根本查不到**，而中文里 2 字词非常常见。

### 方案 B：rowid 即时间顺序 + 长短词分流（✅ 采用）

- 让 `rowid` 单调代表"最近使用"：再次使用某一条时，给它分配新的 rowid。
  FTS5 就能按 `ORDER BY rowid DESC` 遍历倒排表，凑够 50 条就停下。
- 关键词按空格拆开：≥3 字符的走 FTS 短语，1–2 字符的走 `LIKE`，然后取交集。

| 查询 | 耗时 |
|---|---|
| 剪贴板 / 验证码 / 历史记录 | 0.07–0.16ms |
| clipboard / release / token | 0.25–0.73ms |
| 同步 设备（两个 2 字词） | 0.28ms ✅ 能查到了 |
| 剪贴板 同步 / rust 内存（长短混合） | 0.24–0.32ms |
| 密码 / 图（只有短词） | 0.06–0.09ms |
| **汇总** | **P50 0.27ms**；P95 被下面的最坏情况拉高到 84ms |

**唯一的最坏情况**：只有短词并且完全没有命中（如「嗯嗯」），需要扫描全表，大约 80ms。

**应对**：先同步扫描最近 2 万条（约 15ms），剩下的放到后台分批继续扫描，下一次按键时取消。用户看到的是"先出最近的结果，再补全"。

### 对 M1 设计的影响

1. **数据模型**：`clip_items` 用整数 `seq`（rowid，表示最近使用顺序）+ `uuid`（UUIDv7，用于同步的稳定 ID）。再次使用某一条时，在一个事务里删除旧行、用新的 `seq` 重新插入，FTS 通过触发器同步更新。
2. **搜索规划器**放在 `mypaste-search` 中：负责分词、长短词分流、转义、分批扫描和取消。
3. 索引体积大约是原文的 5 倍：10 万条约 70MB，可以接受。如果以后需要优化，可以评估 `detail=none` 或者只索引前 N KB 的文本。

---

## #2 冷启动、唤起延迟、常驻内存（🟡 部分达标）

测试机：RTX 3070 Ti Laptop（NVIDIA 驱动 32.0.16.2124）；系统字体 1087 个 + 用户字体 506 个（702 个字体族）。

### 热键唤起延迟 ✅

| 指标 | 结果 | 目标 |
|---|---|---|
| 热键 → `ShowWindow` 完成 | 45.2ms | — |
| 热键 → 首帧渲染 | **45.4ms**（第一次唤起，窗口事先创建但隐藏） | < 50ms ✅ |

只测到 1 次，样本还不够，需要补测多次唤起的 P50/P95。

### 冷启动 ❌（目标 < 300ms，实测 1.6–3.3s）

release 构建（14.6MB），3 次：窗口出现分别用了 3290 / 2548 / 2375ms。

用启动埋点（`MYPASTE_TRACE=1`）拆分：

| 阶段 | 耗时 |
|---|---|
| `gpui_kit::application()` 创建平台对象 | 650–750ms |
| → 进入 `run` 回调 | 480–600ms |
| `gpui_kit::init`（组件库、主题） | ~10ms |
| `open_window` 创建原生窗口 | 460–520ms |
| 构建视图 + 首帧 | ~50ms |

用 `spikes/startup-probe` 单独测量系统调用，找到两个根因：

| 系统调用 | 耗时 | 说明 |
|---|---|---|
| `GetSystemFontCollection(checkForUpdates=true)` | **470–840ms，每次都要付出** | GPUI 在 `DirectWriteTextSystem::new` 里写死了 `true`。这台机器用户字体多，所以特别慢 |
| `GetSystemFontCollection(checkForUpdates=false)` | ≈0ms | |
| `D3D11CreateDevice` | **365–500ms / 次** | NVIDIA 驱动初始化；GPUI 启动时大概创建了 2 次 |

**结论与对策**：
1. 冷启动慢是 GPUI 在 Windows 上的平台开销，和我们的代码无关（我们的代码只占约 60ms）。
2. 常驻托盘应用只在开机自启时冷启动一次，**对日常体验影响不大**，但值得优化：
   - 字体：给 GPUI 提 issue/PR，把 `checkForUpdates` 改成 `false`，或者在后台线程异步检查。在上游合并前，可以用 `[patch.crates-io]` 本地打补丁，**预计省下 0.5–0.8s**。
   - D3D：确认 GPUI 是否重复创建了设备。如果是，同样走上游修复。
3. 把冷启动目标从"< 300ms"调整为"**< 1s（开机自启场景）**"，并在报告中注明测试机器的显卡型号和字体数量。

### 常驻内存 ❌（目标 < 40MB）

| 状态 | 工作集（含共享） | 提交的私有内存 |
|---|---|---|
| ime-input 窗口可见，启动后 5s | 61MB | 88MB |
| popup-paste 隐藏，启动后 3s | 64–73MB | 155–165MB |
| popup-paste 唤起一次再隐藏 | 79MB | 94MB |
| 线程数 | 30–45 | |

**注意口径**：`WorkingSet64` 包含共享 DLL 页面；`PrivateMemorySize64` 是**提交量**，包含显卡驱动预留的大量未驻留内存，所以并不等于任务管理器显示的数字。之前调研里 Tauri 108MB 的数据用的是"**私有工作集**"（也就是任务管理器"内存"列的数字），要用相同口径重新测量才能比较。

**待做**：
- 用相同口径测量私有工作集，同时测一个空的 Tauri / WebView2 窗口做对照。
- 定位内存去向：D3D 驱动、字体缓存、gpui-kit 的资源（图标、主题）、30+ 个线程的栈。
- 试验 `GPUI_DISABLE_DIRECT_COMPOSITION=1` 对内存的影响。
- 如果 GPU 驱动本身就要占约 50MB，那么 **< 40MB 的目标对任何 GPU 渲染的 UI 都不现实**，需要重新设定目标（比如"私有工作集 < Tauri 的一半"）。

---

## #5 / #6 热键 + 粘贴闭环（🔴 发现问题，待排查）

程序：`spikes/popup-paste`。启动后窗口隐藏，`Alt+V` 唤起，`↑↓` 选择，`Enter` 粘贴回之前的窗口，`Esc` 关闭。

### 已验证

- ✅ `global-hotkey` 在 GPUI 主线程上能正常工作（它的隐藏窗口由 GPUI 的消息循环派发消息）。
- ✅ `WindowKind::PopUp` → `WS_EX_TOOLWINDOW | WS_EX_TOPMOST`：不出现在任务栏，始终置顶。
- ✅ 收到热键后弹窗能拿到前台（`WM_HOTKEY` 会赋予前台权限），正确记录了来源窗口。
- ⚠️ GPUI 在 Windows 上的 `App::hide()` 是**空实现**，显示和隐藏都必须拿到原生 HWND，直接调用 `ShowWindow`。

### 问题：Enter 没有触发粘贴

自动化测试（测试窗口 → `Alt+V` → `↓ ↓ Enter`）：弹窗出现并拿到了前台，但日志里没有 `PressEnter`/`[paste]`，剪贴板为空，焦点也没有回到测试窗口。

**根因分析（读源码，待复测确认）**：输入框处理 `Enter` 的逻辑本身没问题（单行模式下收到 `Enter` action 就会发出 `PressEnter`），问题出在窗口的显示和激活流程上。GPUI 0.3.12 在 Windows 上的 `Window::activate_window()`（`gpui-pre-windows/src/window.rs`）：

1. **是异步执行的**（`executor.spawn`），会在我们同步的显示逻辑之后才执行。
2. 窗口以 `show: false` 创建时，GPUI 会把 `WINDOWPLACEMENT` 存到 `initial_placement`，**第一次 activate 时再异步调用 `SetWindowPlacement`**，这会打乱我们用 `ShowWindow` 设置好的显示状态。
3. 它会**通过 `SendInput` 注入一次 Alt 键的按下和松开**来绕过前台锁。这个假 Alt 会发给当时的前台窗口，可能激活其他程序的菜单栏。

**修复（已实现，待复测）**：
- 窗口以 `show: true, focus: false` 创建，在构建闭包里立刻用 `ShowWindow(SW_HIDE)` 隐藏（首帧之前，不会闪烁），这样 GPUI 就不会保存延迟应用的位置状态。
- **弹出面板绝不调用 `window.activate_window()`**。收到 `WM_HOTKEY` 时进程本来就有前台权限，直接 `ShowWindow` + `SetForegroundWindow`，再用 GPUI 的 `focus()` 设置输入框焦点。

→ **M1 设计约束**：弹出窗口的显示、隐藏、激活、定位全部由我们自己的 Win32 层管理，GPUI 只负责渲染和输入。

已经在程序里加了按键、激活状态变化、`PressEnter` 三处日志。测试脚本放在 `spikes/popup-paste/tests/`，在**你不用电脑的时候**运行即可复测。

> 自动化测试中断的原因：检测到你在前台运行游戏，为了不干扰你，停止了所有会弹窗或模拟按键的测试。测试脚本每一步都会检查前台窗口，不是测试窗口就中止，不会把按键发给你正在使用的程序。
