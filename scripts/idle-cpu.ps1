# Tessera idle CPU meter: how much CPU the app uses while nobody touches it.
#
# Counts the app process and every process it started (the WebView2 browser,
# renderer, GPU and utility processes), so the number matches what the whole app
# costs, not just the Rust side. Two phases of -Seconds each: window open in the
# background, then minimized (the script minimizes and restores it itself).
#
# Usage (from the folder that holds the exe):
#   powershell -ExecutionPolicy Bypass -File idle-cpu.ps1 -Exe tessera-app.exe [-Seconds 60] [-Tag before]
# If the app runs as administrator, run this from an elevated PowerShell too,
# otherwise Windows hides the app's CPU time from it.
# Writes results\<timestamp>\idle-cpu[-tag].txt next to the script.
param(
    [string]$Exe = 'tessera-app.exe',
    [int]$Seconds = 60,
    [string]$Tag = ''
)
$ErrorActionPreference = 'Stop'
Set-Location $PSScriptRoot

Add-Type -Namespace TesseraIdle -Name Win -MemberDefinition @'
[DllImport("user32.dll")] public static extern bool ShowWindow(System.IntPtr hWnd, int nCmdShow);
'@

$name = [IO.Path]::GetFileNameWithoutExtension($Exe)
$path = if ([IO.Path]::IsPathRooted($Exe)) { $Exe } else { Join-Path $PSScriptRoot $Exe }

function Find-App {
    Get-Process -Name $name -ErrorAction SilentlyContinue | Sort-Object StartTime -Descending | Select-Object -First 1
}

$app = Find-App
if (-not $app) {
    if (-not (Test-Path $path)) { Write-Host "找不到 $path"; exit 1 }
    Start-Process -FilePath $path | Out-Null
    Write-Host "已打开 $Exe。"
}
Write-Host ''
Write-Host '1. 在 Tessera 里点 C: 开始扫描，等扫描完成。'
Write-Host '2. 回到这个窗口按回车。之后别碰 Tessera，鼠标也别停在它上面。'
Write-Host ("   脚本先测 {0} 秒（窗口开着、在后台），再把它最小化测 {0} 秒，最后恢复窗口。" -f $Seconds)
[void](Read-Host)

$app = Find-App
if (-not $app) { Write-Host "$Exe 没在运行。"; exit 1 }
$appPid = $app.Id
$cpus = [Environment]::ProcessorCount

# the app and everything it started, with each process's role
function Get-AppTree {
    $all = @(Get-CimInstance Win32_Process | Select-Object ProcessId, ParentProcessId, Name, CommandLine)
    $ids = New-Object 'System.Collections.Generic.HashSet[int]'
    [void]$ids.Add($appPid)
    do {
        $grew = $false
        foreach ($p in $all) {
            if (-not $ids.Contains([int]$p.ProcessId) -and $ids.Contains([int]$p.ParentProcessId)) {
                [void]$ids.Add([int]$p.ProcessId)
                $grew = $true
            }
        }
    } while ($grew)
    foreach ($p in $all) {
        if (-not $ids.Contains([int]$p.ProcessId)) { continue }
        $role = 'WebView2 主进程'
        if ([int]$p.ProcessId -eq $appPid) { $role = 'Tessera（Rust）' }
        elseif ($p.CommandLine -match '--type=renderer') { $role = '渲染进程（界面 JS + 绘制）' }
        elseif ($p.CommandLine -match '--type=gpu-process') { $role = 'GPU 进程' }
        elseif ($p.CommandLine -match '--utility-sub-type=([^\s"]+)') { $role = '工具进程 ' + $Matches[1] }
        elseif ($p.CommandLine -match '--type=([^\s"]+)') { $role = $Matches[1] }
        [pscustomobject]@{ Id = [int]$p.ProcessId; Name = $p.Name; Role = $role }
    }
}

function Get-CpuMs($tree) {
    $h = @{}
    foreach ($p in $tree) {
        try { $h[$p.Id] = (Get-Process -Id $p.Id -ErrorAction Stop).TotalProcessorTime.TotalMilliseconds } catch { }
    }
    $h
}

function Measure-Phase([string]$label) {
    Start-Sleep -Seconds 5 # let the last animation and the minimize settle
    $t0 = Get-AppTree
    $c0 = Get-CpuMs $t0
    if ($c0.Count -eq 0) { throw '读不到 Tessera 的 CPU 时间（它以管理员身份运行时，这个脚本也要用管理员 PowerShell 运行）。' }
    $sw = [Diagnostics.Stopwatch]::StartNew()
    Start-Sleep -Seconds $Seconds
    $t1 = Get-AppTree
    $c1 = Get-CpuMs $t1
    $wall = $sw.Elapsed.TotalMilliseconds
    $rows = foreach ($p in $t1) {
        if (-not $c1.ContainsKey($p.Id)) { continue }
        $before = if ($c0.ContainsKey($p.Id)) { $c0[$p.Id] } else { 0 }
        $ms = $c1[$p.Id] - $before
        [pscustomobject]@{
            '进程'         = $p.Role
            'PID'          = $p.Id
            'CPU 毫秒'     = [math]::Round($ms)
            '占一个核 %'   = [math]::Round(100 * $ms / $wall, 3)
        }
    }
    $rows = @($rows | Sort-Object 'CPU 毫秒' -Descending)
    $total = ($rows | Measure-Object 'CPU 毫秒' -Sum).Sum
    $gone = @($t0 | Where-Object { -not $c1.ContainsKey($_.Id) }).Count
    $out = @()
    $out += ("== {0}，{1:N0} 秒" -f $label, ($wall / 1000))
    $out += ($rows | Format-Table -AutoSize | Out-String).TrimEnd()
    $out += ("合计 {0:N0} 毫秒 CPU = 一个核的 {1:N3}% = 整个 CPU（{2} 个逻辑核，任务管理器的口径）的 {3:N3}%" -f $total, (100 * $total / $wall), $cpus, (100 * $total / $wall / $cpus))
    if ($gone) { $out += "（测量期间有 $gone 个进程退出，它们的 CPU 没算进去）" }
    $out += ''
    $out
}

$elevated = ([Security.Principal.WindowsPrincipal][Security.Principal.WindowsIdentity]::GetCurrent()).IsInRole([Security.Principal.WindowsBuiltInRole]::Administrator)
$report = @()
$report += "Tessera 空闲 CPU  $(Get-Date -Format o)  exe=$Exe  pid=$appPid  脚本管理员=$elevated"
$report += ''
$report += Measure-Phase '窗口开着、在后台'
$hwnd = (Get-Process -Id $appPid).MainWindowHandle
if ($hwnd -ne [IntPtr]::Zero) {
    [void][TesseraIdle.Win]::ShowWindow($hwnd, 6) # SW_MINIMIZE
    $report += Measure-Phase '最小化'
    [void][TesseraIdle.Win]::ShowWindow($hwnd, 9) # SW_RESTORE
} else {
    $report += '（找不到 Tessera 的窗口，跳过最小化那一段）'
}

$dir = Join-Path $PSScriptRoot ('results\' + (Get-Date -Format 'yyyyMMdd-HHmmss'))
New-Item -ItemType Directory -Force $dir | Out-Null
$file = Join-Path $dir ('idle-cpu' + $(if ($Tag) { "-$Tag" } else { '' }) + '.txt')
$report | Out-File -FilePath $file -Encoding UTF8
$report | ForEach-Object { Write-Host $_ }
Write-Host "结果已保存到 $file"
