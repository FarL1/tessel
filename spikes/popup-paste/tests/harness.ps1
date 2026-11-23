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
  [StructLayout(LayoutKind.Sequential)] public struct KEYBDINPUT { public ushort wVk; public ushort wScan; public uint dwFlags; public uint time; public IntPtr dwExtraInfo; }
  [StructLayout(LayoutKind.Explicit, Size = 40)] public struct INPUT { [FieldOffset(0)] public uint type; [FieldOffset(8)] public KEYBDINPUT ki; }
  [DllImport("user32.dll")] public static extern uint SendInput(uint n, INPUT[] inputs, int size);
  // KEYEVENTF_UNICODE：直接输入字符，绕过输入法组字
  public static void TypeUnicode(string text) {
    foreach (char c in text) {
      var down = new INPUT { type = 1, ki = new KEYBDINPUT { wScan = c, dwFlags = 0x0004 } };
      var up = new INPUT { type = 1, ki = new KEYBDINPUT { wScan = c, dwFlags = 0x0004 | 0x0002 } };
      SendInput(2, new[] { down, up }, Marshal.SizeOf(typeof(INPUT)));
    }
  }
  [StructLayout(LayoutKind.Sequential)] public struct GUITHREADINFO { public int cbSize; public int flags; public IntPtr hwndActive; public IntPtr hwndFocus; public IntPtr hwndCapture; public IntPtr hwndMenuOwner; public IntPtr hwndMoveSize; public IntPtr hwndCaret; public RECT rcCaret; }
  [DllImport("user32.dll")] public static extern bool GetGUIThreadInfo(uint tid, ref GUITHREADINFO info);
  [DllImport("user32.dll", CharSet=CharSet.Unicode)] public static extern int GetClassNameW(IntPtr h, System.Text.StringBuilder s, int n);
  static string Cls(IntPtr h) { var sb = new System.Text.StringBuilder(128); GetClassNameW(h, sb, 128); return h + "(" + sb + ")"; }
  public static string FocusInfo() {
    IntPtr fg = GetForegroundWindow(); uint p; uint tid = GetWindowThreadProcessId(fg, out p);
    var gi = new GUITHREADINFO(); gi.cbSize = Marshal.SizeOf(gi); GetGUIThreadInfo(tid, ref gi);
    return "foreground=" + Cls(fg) + " active=" + Cls(gi.hwndActive) + " focus=" + Cls(gi.hwndFocus) + " flags=" + gi.flags;
  }
  public delegate bool EnumCB(IntPtr h, IntPtr l);
  [DllImport("user32.dll")] public static extern bool EnumWindows(EnumCB cb, IntPtr l);
  [DllImport("user32.dll")] public static extern bool IsWindowVisible(IntPtr h);
  [DllImport("user32.dll")] public static extern bool PostMessageW(IntPtr h, uint m, IntPtr w, IntPtr l);
  public static IntPtr FindPopup(uint pid) { IntPtr found = IntPtr.Zero; EnumWindows((h, x) => { uint p; GetWindowThreadProcessId(h, out p); var sb = new System.Text.StringBuilder(64); GetClassNameW(h, sb, 64); if (p == pid && IsWindowVisible(h) && sb.ToString() == "Zed::Window") { found = h; return false; } return true; }, IntPtr.Zero); return found; }
  // 只投递给弹窗本身，不经过系统输入队列，不会误伤其他程序
  public static void PostKey(IntPtr h, int vk) { PostMessageW(h, 0x0100, (IntPtr)vk, (IntPtr)1); PostMessageW(h, 0x0101, (IntPtr)vk, unchecked((IntPtr)(int)0xC0000001)); }
  public static void PostText(IntPtr h, string s) { foreach (char c in s) PostMessageW(h, 0x0102, (IntPtr)c, (IntPtr)1); }
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
while ((Get-Date) -lt $deadline) { $form = [K]::FindWindowW([NullString]::Value, "MyPaste Paste Target"); if ($form -ne [IntPtr]::Zero) { break }; Start-Sleep -Milliseconds 100 }
"form hwnd=$form" 
Start-Sleep -Milliseconds 800
for ($i = 0; $i -lt 15 -and [K]::FgPid() -ne $t.Id; $i++) {
  $r = [K]::ClickToActivate($form, [uint32]$t.Id); if ($i -eq 0 -or $r -eq "clicked") { "click #${i}: $r" }
  Start-Sleep -Milliseconds 300
}
if ([K]::FgPid() -ne $t.Id) { "ABORT: 测试窗口没能成为前台（前台 pid=$([K]::FgPid())），不发送按键"; Stop-Process -Id $t.Id; exit 1 }
"OK: 测试窗口在前台"

function Expect-Popup { Start-Sleep -Milliseconds 400; return ([K]::FgPid() -eq $popupPid) }

function Popup { $p = [K]::FindPopup([uint32]$popupPid); if ($p -eq [IntPtr]::Zero) { "ABORT: 找不到可见的弹窗"; Stop-Process -Id $t.Id; exit 1 }; return $p }

# 场景 1：Alt+V → ↓ ↓ → Enter，粘贴第 3 条（代码）
[K]::Chord(0x12, 0x56)
if (-not (Expect-Popup)) { "ABORT: 弹窗没有成为前台"; Stop-Process -Id $t.Id; exit 1 }
$p = Popup; [K]::PostKey($p, 0x28); [K]::PostKey($p, 0x28); Start-Sleep -Milliseconds 150; [K]::PostKey($p, 0x0D)
Start-Sleep -Milliseconds 700
if ([K]::FgPid() -ne $t.Id) { "ABORT: 粘贴后焦点没有回到测试窗口"; Stop-Process -Id $t.Id; exit 1 }
"场景 1 完成"

# 场景 2：Alt+V → 搜索「上海」→ Enter
[K]::Tap(0x0D)
[K]::Chord(0x12, 0x56)
if (-not (Expect-Popup)) { "ABORT: 弹窗没有成为前台(2)"; Stop-Process -Id $t.Id; exit 1 }
$p = Popup; [K]::PostText($p, "上海"); Start-Sleep -Milliseconds 200; [K]::PostKey($p, 0x0D)
Start-Sleep -Milliseconds 700
if ([K]::FgPid() -ne $t.Id) { "ABORT: 粘贴后焦点没有回到测试窗口(2)"; Stop-Process -Id $t.Id; exit 1 }
"场景 2 完成"

# 场景 3：Alt+V → Tab 切到「链接」→ Ctrl+1 快速粘贴（Ctrl 状态用 PostMessage 无法表达，这里用 Enter 代替）
[K]::Tap(0x0D)
[K]::Chord(0x12, 0x56)
if (-not (Expect-Popup)) { "ABORT: 弹窗没有成为前台(3)"; Stop-Process -Id $t.Id; exit 1 }
$p = Popup; [K]::PostKey($p, 0x09); [K]::PostKey($p, 0x09); Start-Sleep -Milliseconds 150; [K]::PostKey($p, 0x0D)
Start-Sleep -Milliseconds 700
"场景 3 完成"

# 场景 4：唤起 / Esc 关闭 循环 10 次，收集唤起延迟
$ok = 0
for ($i = 0; $i -lt 10; $i++) {
  if ([K]::FgPid() -ne $t.Id) { "WARN: 第 $i 次循环前前台不是测试窗口"; break }
  [K]::Chord(0x12, 0x56)
  if (-not (Expect-Popup)) { "WARN: 第 $i 次弹窗未成为前台"; break }
  $p = Popup; [K]::PostKey($p, 0x1B); Start-Sleep -Milliseconds 400; $ok++
}
"场景 4：成功唤起并关闭 $ok / 10 次"
Start-Sleep -Milliseconds 500
Stop-Process -Id $t.Id -ErrorAction SilentlyContinue
