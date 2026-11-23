param([int]$popupPid)
Add-Type @"
using System; using System.Runtime.InteropServices;
public class K {
  [DllImport("user32.dll")] public static extern void keybd_event(byte vk, byte scan, uint flags, UIntPtr extra);
  [DllImport("user32.dll")] public static extern IntPtr GetForegroundWindow();
  [DllImport("user32.dll")] public static extern uint GetWindowThreadProcessId(IntPtr h, out uint pid);
  [DllImport("user32.dll")] public static extern bool SetForegroundWindow(IntPtr h);
  [DllImport("user32.dll")] public static extern bool AttachThreadInput(uint a, uint b, bool attach);
  [DllImport("kernel32.dll")] public static extern uint GetCurrentThreadId();
  [DllImport("user32.dll")] public static extern bool GetWindowRect(IntPtr h, out RECT r);
  [DllImport("user32.dll")] public static extern IntPtr WindowFromPoint(POINT p);
  [DllImport("user32.dll")] public static extern bool SetCursorPos(int x, int y);
  [DllImport("user32.dll")] public static extern bool GetCursorPos(out POINT p);
  [DllImport("user32.dll")] public static extern void mouse_event(uint f, int dx, int dy, uint d, UIntPtr e);
  [DllImport("user32.dll")] public static extern bool SetProcessDPIAware();
  [DllImport("user32.dll", CharSet=CharSet.Unicode)] public static extern IntPtr FindWindowW(string cls, string title);
  public struct RECT { public int L,T,R,B; }
  public struct POINT { public int X,Y; }
  // 只有当点击位置最上层的窗口属于目标进程时才点击
  public static string ClickToActivate(IntPtr h, uint targetPid) {
    SetProcessDPIAware();
    RECT r; GetWindowRect(h, out r);
    POINT c; c.X = (r.L + r.R) / 2; c.Y = (r.T + r.B) / 2;
    uint p; GetWindowThreadProcessId(WindowFromPoint(c), out p);
    if (p != targetPid) return "skip: 点击位置的窗口属于 pid " + p;
    POINT old; GetCursorPos(out old);
    SetCursorPos(c.X, c.Y); mouse_event(2,0,0,0,UIntPtr.Zero); mouse_event(4,0,0,0,UIntPtr.Zero);
    SetCursorPos(old.X, old.Y);
    return "clicked";
  }
  public static bool ForceForeground(IntPtr h) {
    uint p; uint fgThread = GetWindowThreadProcessId(GetForegroundWindow(), out p);
    uint me = GetCurrentThreadId();
    AttachThreadInput(me, fgThread, true);
    bool ok = SetForegroundWindow(h);
    AttachThreadInput(me, fgThread, false);
    return ok;
  }
  public static uint FgPid() { uint p; GetWindowThreadProcessId(GetForegroundWindow(), out p); return p; }
  public static void Tap(byte vk) { keybd_event(vk,0,0,UIntPtr.Zero); keybd_event(vk,0,2,UIntPtr.Zero); }
  public static void Chord(byte mod, byte vk) { keybd_event(mod,0,0,UIntPtr.Zero); keybd_event(vk,0,0,UIntPtr.Zero); keybd_event(vk,0,2,UIntPtr.Zero); keybd_event(mod,0,2,UIntPtr.Zero); }
}
"@
$out = "$env:TEMP\paste-target.txt"; Remove-Item $out -ErrorAction SilentlyContinue
$t = Start-Process powershell -ArgumentList "-NoProfile -STA -ExecutionPolicy Bypass -File `"$PSScriptRoot\target-window.ps1`" 25 `"$out`"" -PassThru -WindowStyle Hidden
$deadline = (Get-Date).AddSeconds(8)
$form = [IntPtr]::Zero
while ((Get-Date) -lt $deadline) { $form = [K]::FindWindowW($null, "MyPaste Paste Target"); if ($form -ne [IntPtr]::Zero) { break }; Start-Sleep -Milliseconds 100 }
"form hwnd=$form" 
Start-Sleep -Milliseconds 800
if ([K]::FgPid() -ne $t.Id) { [K]::ClickToActivate($form, [uint32]$t.Id); Start-Sleep -Milliseconds 300 }
if ([K]::FgPid() -ne $t.Id) { "ABORT: 测试窗口没能成为前台（前台 pid=$([K]::FgPid())），不发送按键"; Stop-Process -Id $t.Id; exit 1 }
"OK: 测试窗口在前台"

function Expect-Popup { Start-Sleep -Milliseconds 400; return ([K]::FgPid() -eq $popupPid) }

# 场景 1：粘贴第 3 条（验证码）
[K]::Chord(0x12, 0x56)                         # Alt+V
if (-not (Expect-Popup)) { "ABORT: 弹窗没有成为前台"; Stop-Process -Id $t.Id; exit 1 }
[K]::Tap(0x28); [K]::Tap(0x28); Start-Sleep -Milliseconds 100; [K]::Tap(0x0D)   # ↓ ↓ Enter
Start-Sleep -Milliseconds 600
if ([K]::FgPid() -ne $t.Id) { "ABORT: 粘贴后焦点没有回到测试窗口"; Stop-Process -Id $t.Id; exit 1 }

# 场景 2：搜索后粘贴 Emoji 条目（"emoji"）
[K]::Tap(0x0D)                                 # 测试窗口里换行
[K]::Chord(0x12, 0x56)
if (-not (Expect-Popup)) { "ABORT: 弹窗没有成为前台(2)"; Stop-Process -Id $t.Id; exit 1 }
foreach ($c in [byte[]](0x45,0x4D,0x4F,0x4A,0x49)) { [K]::Tap($c) }   # e m o j i
Start-Sleep -Milliseconds 150; [K]::Tap(0x0D)
Start-Sleep -Milliseconds 600

# 场景 3：唤起/Esc 关闭 循环 8 次，收集延迟
for ($i = 0; $i -lt 8; $i++) {
  if ([K]::FgPid() -ne $t.Id) { "ABORT: 循环中前台不是测试窗口"; break }
  [K]::Chord(0x12, 0x56)
  if (-not (Expect-Popup)) { "WARN: 第 $i 次弹窗未成为前台"; break }
  [K]::Tap(0x1B); Start-Sleep -Milliseconds 300
}
Stop-Process -Id $t.Id -ErrorAction SilentlyContinue
