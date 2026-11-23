param([int]$seconds, [string]$out)
Add-Type -AssemblyName System.Windows.Forms, System.Drawing
$f = New-Object Windows.Forms.Form
$f.Text = "MyPaste Paste Target"; $f.TopMost = $true; $f.Width = 640; $f.Height = 220
$f.StartPosition = 'CenterScreen'
$tb = New-Object Windows.Forms.TextBox
$tb.Multiline = $true; $tb.Dock = 'Fill'
$tb.Font = New-Object System.Drawing.Font("Microsoft YaHei UI", 12)
$f.Controls.Add($tb)
$script:deadline = (Get-Date).AddSeconds($seconds)
$timer = New-Object Windows.Forms.Timer; $timer.Interval = 200
$timer.Add_Tick({
  if ((Get-Date) -ge $script:deadline) {
    $timer.Stop()
    [IO.File]::WriteAllText($out, $tb.Text, [Text.Encoding]::UTF8)
    $f.Close()
  }
})
$f.Add_Shown({ $f.Activate(); $tb.Focus() | Out-Null; $timer.Start(); "shown" | Out-Host })
[Windows.Forms.Application]::Run($f)
"closed" | Out-Host
