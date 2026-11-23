# 本地安装 Tessel 原型，方便日常测试：
#   编译 release → 关闭正在运行的 Tessel → 复制到 %LOCALAPPDATA%\Programs\Tessel → 创建/更新桌面快捷方式 → 启动
#
# 用法（在仓库根目录）：
#   powershell -ExecutionPolicy Bypass -File scripts\install-local.ps1            # 编译并安装
#   powershell -ExecutionPolicy Bypass -File scripts\install-local.ps1 -NoBuild   # 跳过编译，直接安装现有产物
#   powershell -ExecutionPolicy Bypass -File scripts\install-local.ps1 -NoLaunch  # 安装后不启动
param(
    [switch]$NoBuild,
    [switch]$NoLaunch
)
$ErrorActionPreference = 'Stop'
$repo = Split-Path -Parent $PSScriptRoot

if (-not $NoBuild) {
    Write-Host '==> 编译 release 版（首次需要约 10 分钟）'
    Push-Location $repo
    try { cargo build -p tessel --release; if ($LASTEXITCODE -ne 0) { throw '编译失败' } }
    finally { Pop-Location }
}

# 编译产物的位置由 cargo 决定（本机把 target-dir 配到了 D 盘），这里直接问 cargo
$targetDir = (cargo metadata --format-version 1 --no-deps --manifest-path "$repo\Cargo.toml" | ConvertFrom-Json).target_directory
$built = Join-Path $targetDir 'release\tessel.exe'
if (-not (Test-Path $built)) { throw "找不到编译产物：$built" }

Write-Host '==> 关闭正在运行的 Tessel'
Get-Process tessel -ErrorAction SilentlyContinue | Stop-Process -Force
Start-Sleep -Milliseconds 300

$installDir = Join-Path $env:LOCALAPPDATA 'Programs\Tessel'
New-Item -ItemType Directory -Force -Path $installDir | Out-Null
$exe = Join-Path $installDir 'tessel.exe'
Copy-Item $built $exe -Force
Write-Host "==> 已安装到 $exe"

$desktop = [Environment]::GetFolderPath('Desktop')
$lnkPath = Join-Path $desktop 'Tessel 拼贴.lnk'
$shell = New-Object -ComObject WScript.Shell
$lnk = $shell.CreateShortcut($lnkPath)
$lnk.TargetPath = $exe
$lnk.WorkingDirectory = $installDir
$lnk.IconLocation = "$exe,0"
$lnk.Description = 'Tessel 拼贴 · 剪贴板工具（按 Alt+V 唤起）'
$lnk.Save()
Write-Host "==> 桌面快捷方式：$lnkPath"

if (-not $NoLaunch) {
    Start-Process $exe
    Write-Host '==> 已启动（之后按 Alt+V 唤起，托盘图标右键可以退出）'
}
