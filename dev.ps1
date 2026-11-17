#Requires -Version 7.0

# 本地前后端共用一个生命周期，数据库与 Redis 由现有服务提供
[CmdletBinding()]
param()

Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'

$projectDirectory = $PSScriptRoot
$frontendDirectory = Join-Path $projectDirectory 'frontend'
$viteEntry = Join-Path $frontendDirectory 'node_modules/vite/bin/vite.js'
$configPath = Join-Path $projectDirectory 'deploy/config.yaml'

if (-not (Test-Path -LiteralPath $configPath -PathType Leaf)) {
    throw '缺少 deploy/config.yaml，请按 docs/development.md 准备本地数据库与 Redis 配置'
}
if (-not (Test-Path -LiteralPath $viteEntry -PathType Leaf)) {
    throw '缺少前端依赖，请先执行 pnpm --dir frontend install --frozen-lockfile'
}

$cargoCommand = (Get-Command cargo -CommandType Application -ErrorAction Stop).Source
$nodeCommand = (Get-Command node -CommandType Application -ErrorAction Stop).Source

# 不接管已有进程，也不允许 Vite 换端口，否则页面可能连到另一版本的后端
$listeners = [System.Net.NetworkInformation.IPGlobalProperties]::GetIPGlobalProperties().GetActiveTcpListeners()
foreach ($port in @(5173, 8080)) {
    if ($listeners.Port -contains $port) {
        throw "端口 $port 已被占用，请先关闭原来的前端或后端，再重新运行本命令"
    }
}

function Start-DevelopmentProcess {
    param(
        [string] $Executable,
        [string] $Directory,
        [string[]] $Arguments,
        [hashtable] $Environment = @{}
    )

    $startInfo = [System.Diagnostics.ProcessStartInfo]::new()
    $startInfo.FileName = $Executable
    $startInfo.WorkingDirectory = $Directory
    $startInfo.UseShellExecute = $false
    # 共享当前终端；Windows 下禁用控制台且不重定向输出会隐藏编译错误与启动日志
    $startInfo.CreateNoWindow = $false
    foreach ($argument in $Arguments) {
        $startInfo.ArgumentList.Add($argument)
    }
    foreach ($entry in $Environment.GetEnumerator()) {
        $startInfo.Environment[$entry.Key] = $entry.Value
    }
    return [System.Diagnostics.Process]::Start($startInfo)
}

$developmentProcesses = [System.Collections.Generic.List[object]]::new()
try {
    # 沿用仓库根目录检查时的系统工具链与环境，不在启动时另行安装工具链
    $backendProcess = Start-DevelopmentProcess -Executable $cargoCommand -Directory $projectDirectory `
        -Arguments @('run', '--locked', '--manifest-path', 'backend/Cargo.toml', '-p', 'codex-proxy-rs') `
        -Environment @{ CPR_SERVER_HOST = '127.0.0.1'; CPR_SERVER_PORT = '8080'; VEIL_SKIP_UI_BUILD = '1' }
    $developmentProcesses.Add(@{ Name = '后端'; Process = $backendProcess })

    Write-Host '正在编译并启动后端，首次编译可能需要较长时间；按 Ctrl+C 停止'
    # 编译进程存在不代表 API 已就绪，提前打开前端会使开发代理返回 502
    $healthHandler = [System.Net.Http.HttpClientHandler]::new()
    $healthHandler.UseProxy = $false
    $healthClient = [System.Net.Http.HttpClient]::new($healthHandler)
    $healthClient.Timeout = [TimeSpan]::FromSeconds(1)
    try {
        while (-not $backendProcess.HasExited) {
            try {
                $healthResponse = $healthClient.GetAsync('http://127.0.0.1:8080/healthz').GetAwaiter().GetResult()
                try {
                    if ([int]$healthResponse.StatusCode -eq 204) {
                        break
                    }
                }
                finally {
                    $healthResponse.Dispose()
                }
            }
            catch [System.Net.Http.HttpRequestException], [System.Threading.Tasks.TaskCanceledException] {
                # 编译及初始化期间尚无可用监听，继续等待并保留终端中的后端错误
            }
            Start-Sleep -Milliseconds 500
        }
        if ($backendProcess.HasExited) {
            throw "后端在就绪前退出，退出码 $($backendProcess.ExitCode)，请查看上方编译或启动错误"
        }
    }
    finally {
        $healthClient.Dispose()
    }

    $frontendProcess = Start-DevelopmentProcess -Executable $nodeCommand -Directory $frontendDirectory `
        -Arguments @($viteEntry, '--host', '127.0.0.1', '--port', '5173', '--strictPort')
    $developmentProcesses.Add(@{ Name = '前端'; Process = $frontendProcess })

    Write-Host '后端已就绪，正在启动前端'
    Write-Host '前端地址：http://localhost:5173；后端地址：http://127.0.0.1:8080'
    Write-Host '等待各服务输出就绪信息；按 Ctrl+C 一起停止'

    while ($true) {
        foreach ($entry in $developmentProcesses) {
            if ($entry.Process.HasExited) {
                if ($entry.Process.ExitCode -ne 0) {
                    throw "$($entry.Name)已退出，退出码 $($entry.Process.ExitCode)，正在关闭另一服务"
                }
                Write-Host "$($entry.Name)已退出，正在关闭另一服务"
                return
            }
        }
        Start-Sleep -Milliseconds 250
    }
}
finally {
    # 只回收本次启动的进程树，包含 cargo 启动的网关；不停止 PostgreSQL 或 Redis
    foreach ($entry in $developmentProcesses) {
        try {
            if (-not $entry.Process.HasExited) {
                $entry.Process.Kill($true)
                $entry.Process.WaitForExit()
            }
        }
        finally {
            $entry.Process.Dispose()
        }
    }
}
