param(
    [Parameter(Mandatory = $true)]
    [int] $ProcessId,
    [int] $DurationSeconds = 600,
    [Parameter(Mandatory = $true)]
    [string] $OutputPath
)

$ErrorActionPreference = 'Stop'
Add-Type @'
using System;
using System.Runtime.InteropServices;
public static class GuiResourceMeter {
    [DllImport("user32.dll")]
    public static extern uint GetGuiResources(IntPtr process, uint flags);
}
'@

$process = Get-Process -Id $ProcessId
$startedAt = [DateTimeOffset]::UtcNow
$initialCpu = $process.TotalProcessorTime.TotalSeconds
$workingSets = [Collections.Generic.List[double]]::new()
$handles = [Collections.Generic.List[int]]::new()
$gdi = [Collections.Generic.List[uint32]]::new()
$user = [Collections.Generic.List[uint32]]::new()
$deadline = [DateTime]::UtcNow.AddSeconds($DurationSeconds)

while ([DateTime]::UtcNow -lt $deadline) {
    $process.Refresh()
    if ($process.HasExited) {
        throw "Process $ProcessId exited during the residency test"
    }
    $workingSets.Add($process.WorkingSet64 / 1MB)
    $handles.Add($process.HandleCount)
    $gdi.Add([GuiResourceMeter]::GetGuiResources($process.Handle, 0))
    $user.Add([GuiResourceMeter]::GetGuiResources($process.Handle, 1))
    Start-Sleep -Seconds 2
}

$process.Refresh()
$elapsed = ([DateTimeOffset]::UtcNow - $startedAt).TotalSeconds
$cpuDelta = $process.TotalProcessorTime.TotalSeconds - $initialCpu
$result = [ordered]@{
    processId = $ProcessId
    durationSeconds = [math]::Round($elapsed, 1)
    samples = $workingSets.Count
    averageWorkingSetMB = [math]::Round(($workingSets | Measure-Object -Average).Average, 2)
    maximumWorkingSetMB = [math]::Round(($workingSets | Measure-Object -Maximum).Maximum, 2)
    averageCpuPercent = [math]::Round(100 * $cpuDelta / $elapsed / [Environment]::ProcessorCount, 4)
    initialHandles = $handles[0]
    finalHandles = $handles[$handles.Count - 1]
    initialGdiObjects = $gdi[0]
    finalGdiObjects = $gdi[$gdi.Count - 1]
    initialUserObjects = $user[0]
    finalUserObjects = $user[$user.Count - 1]
}

$result | ConvertTo-Json | Set-Content -LiteralPath $OutputPath -Encoding utf8
