# Tessel 原型自动化测试

**只在你不用电脑时运行。**测试期间会弹出测试窗口、模拟按键，并点击一次鼠标，总共大约 30 秒。

安全措施：
- 每一步之前都会检查前台窗口，确认是测试窗口或弹窗，否则立即中止，不会把按键发给其他程序。
- 弹窗里的按键通过 `PostMessage` 直接投递给弹窗本身。原因是弹窗在前台时，`SendInput` 注入的按键会丢失（真实键盘不受影响），详见 `docs/m0-report.md`。

```powershell
# 1. 启动弹窗程序（--background：启动后不自动弹出）
Start-Process D:\cargo-target\mypaste\debug\tessel.exe -ArgumentList --background -RedirectStandardOutput $env:TEMP\tessel.log
# 2. 运行测试
powershell -ExecutionPolicy Bypass -File spikes\popup-paste\tests\harness.ps1 (Get-Process tessel).Id
# 3. 查看结果：测试窗口收到的文本，以及弹窗日志
Get-Content $env:TEMP\paste-target.txt; Get-Content $env:TEMP\tessel.log
```

测试场景：
1. `Alt+V` → `↓ ↓` → `Enter`：应该粘贴第 3 条（代码）。
2. `Alt+V` → 输入"上海" → `Enter`：应该粘贴收货地址。
3. `Alt+V` → `Tab Tab`（切到"链接"）→ `Enter`：应该粘贴链接。
4. 唤起后按 `Esc` 关闭，循环 10 次，统计唤起延迟。
