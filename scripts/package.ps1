# 生成 Windows 安装包（Velopack）：dist\Tessel.App-win-Setup.exe
#
# 前置：dotnet tool install -g vpk --version 1.2.161（版本必须和 apps/tessel/Cargo.toml 里的 velopack 一致）
#
# 用法（在仓库根目录）：
#   powershell -ExecutionPolicy Bypass -File scripts\package.ps1
#   powershell -ExecutionPolicy Bypass -File scripts\package.ps1 -NoBuild   # 跳过编译
#
# 安装包的行为：
# - 按当前用户安装到 %LOCALAPPDATA%\Tessel.App，不需要管理员权限
# - 创建桌面和开始菜单快捷方式，在"设置 → 应用"里登记卸载入口
# - 卸载时删除程序和快捷方式、关闭开机自启；用户数据（%LOCALAPPDATA%\Tessel）保留
#   注意：packId 不能叫 "Tessel"，否则安装目录会和数据目录重合，卸载时会把历史一起删掉
param([switch]$NoBuild)
$ErrorActionPreference = 'Stop'
$repo = Split-Path -Parent $PSScriptRoot
$env:PATH += ";$env:USERPROFILE\.dotnet\tools"

if (-not (Get-Command vpk -ErrorAction SilentlyContinue)) {
    throw '找不到 vpk，请先运行：dotnet tool install -g vpk --version 1.2.161'
}

Push-Location $repo
try {
    if (-not $NoBuild) {
        Write-Host '==> 编译 release 版'
        cargo build -p tessel -p tessel-cli --release
        if ($LASTEXITCODE -ne 0) { throw '编译失败' }
    }

    $meta = cargo metadata --format-version 1 --no-deps | ConvertFrom-Json
    $version = ($meta.packages | Where-Object name -eq 'tessel').version
    $built = Join-Path $meta.target_directory 'release\tessel.exe'
    $cli = Join-Path $meta.target_directory 'release\tessel-cli.exe'
    foreach ($f in $built, $cli) {
        if (-not (Test-Path $f)) { throw "找不到编译产物：$f" }
    }

    # 发布目录里只放需要分发的文件（桌面端 + 命令行工具 + 许可证）
    $publish = Join-Path $meta.target_directory 'package\publish'
    Remove-Item $publish -Recurse -Force -ErrorAction SilentlyContinue
    New-Item -ItemType Directory -Force -Path $publish | Out-Null
    Copy-Item $built, $cli $publish
    Copy-Item LICENSE-MIT, LICENSE-APACHE $publish

    Write-Host "==> 打包 Tessel $version"
    vpk pack `
        --packId Tessel.App `
        --packVersion $version `
        --packDir $publish `
        --mainExe tessel.exe `
        --packTitle 'Tessel 拼贴' `
        --packAuthors 'Tessel contributors' `
        --icon assets\brand\tessel.ico `
        --shortcuts Desktop,StartMenuRoot `
        --outputDir dist
    if ($LASTEXITCODE -ne 0) { throw 'vpk pack 失败' }

    Write-Host '==> 产物：'
    Get-ChildItem dist | Format-Table Name, @{ n = '大小'; e = { '{0:N1} MB' -f ($_.Length / 1MB) } } -AutoSize
}
finally { Pop-Location }
