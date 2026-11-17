# 固定上游源码与本地补丁；不创建提交、不推送、不覆盖已有工作副本。
[CmdletBinding()]
param()
$ErrorActionPreference = 'Stop'
$root = [IO.Path]::GetFullPath((Join-Path $PSScriptRoot '..'))
$destination = Join-Path $root '.deps/veil'
$revision = '82f9290022144c82609aaad149d81693c4a563ec'
$patch = Join-Path $root 'scripts/patches/veil-library.patch'

if (-not (Test-Path -LiteralPath $destination)) {
    New-Item -ItemType Directory -Path $destination -Force | Out-Null
    & git -C $destination init --quiet
    if ($LASTEXITCODE -ne 0) { throw 'Cannot initialize veil checkout' }
    & git -C $destination remote add origin https://github.com/Mouseww/veil.git
    if ($LASTEXITCODE -ne 0) { throw 'Cannot configure veil origin' }
    & git -C $destination fetch --depth 1 origin $revision
    if ($LASTEXITCODE -ne 0) { throw 'Cannot fetch pinned veil revision' }
    & git -C $destination checkout --detach FETCH_HEAD
    if ($LASTEXITCODE -ne 0) { throw 'Cannot check out pinned veil revision' }
}
$actual = & git -C $destination rev-parse HEAD
if ($LASTEXITCODE -ne 0 -or $actual -ne $revision) { throw 'Existing veil checkout does not match the pinned revision; it has been preserved' }

& git -C $destination apply --reverse --check $patch 2>$null
if ($LASTEXITCODE -ne 0) {
    & git -C $destination apply --check $patch
    if ($LASTEXITCODE -ne 0) { throw 'Existing veil changes conflict with the patch; they have been preserved' }
    & git -C $destination apply $patch
    if ($LASTEXITCODE -ne 0) { throw 'Cannot apply veil library patch' }
}
Write-Output "Veil ready: $revision + scripts/patches/veil-library.patch"
