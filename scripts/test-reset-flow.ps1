param([string]$Channel = 'Development')
$ErrorActionPreference='Stop'
$repo = Split-Path $PSScriptRoot -Parent
$exe = Join-Path $repo 'target/x86_64-pc-windows-msvc/release/codex-status.exe'
$fake = Join-Path $repo 'target/x86_64-pc-windows-msvc/release/examples/fake-codex.exe'
$fixture = Join-Path $repo ('dist/reset-flow-' + [Guid]::NewGuid().ToString('N'))
New-Item -ItemType Directory -Path $fixture | Out-Null
Add-Type @'
using System;
using System.Runtime.InteropServices;
public static class ResetFlowNative {
 [DllImport("user32.dll",CharSet=CharSet.Unicode)] public static extern IntPtr FindWindow(string cls,string title);
 [DllImport("user32.dll")] public static extern uint GetWindowThreadProcessId(IntPtr hwnd,out uint pid);
 [DllImport("user32.dll")] public static extern IntPtr SendMessage(IntPtr hwnd,uint msg,IntPtr w,IntPtr l);
 [DllImport("user32.dll")] public static extern bool PostMessage(IntPtr hwnd,uint msg,IntPtr w,IntPtr l);
 [DllImport("user32.dll")] public static extern uint GetDpiForWindow(IntPtr hwnd);
}
'@
$info=[Diagnostics.ProcessStartInfo]::new($exe)
$info.UseShellExecute=$false
$info.WindowStyle=[Diagnostics.ProcessWindowStyle]::Hidden
$info.Environment['CODEX_STATUS_CODEX']=$fake
$info.Environment['CODEX_STATUS_FIXTURE_DIR']=$fixture
$info.Environment['LOCALAPPDATA']=$fixture
$process=[Diagnostics.Process]::Start($info)
try {
 Start-Sleep -Seconds 3
 $main=[ResetFlowNative]::FindWindow("CodexStatus.$Channel.MainWindow.v1",'CodexStatus')
 $flyout=[ResetFlowNative]::FindWindow("CodexStatus.$Channel.FlyoutWindow.v1",'CodexStatus')
 [uint32]$windowPid=0
 [void][ResetFlowNative]::GetWindowThreadProcessId($main,[ref]$windowPid)
 if ($windowPid -ne $process.Id) { throw 'Fixture window PID mismatch; refusing UI interaction' }
 [void][ResetFlowNative]::GetWindowThreadProcessId($flyout,[ref]$windowPid)
 if ($windowPid -ne $process.Id) { throw 'Flyout PID mismatch' }
 [void][ResetFlowNative]::SendMessage($main,0x8003,0,0)
 Start-Sleep -Milliseconds 400
 function Click-Fixture([int]$x,[int]$y) {
  $dpi=[ResetFlowNative]::GetDpiForWindow($flyout)
  $px=[int]($x*$dpi/96); $py=[int]($y*$dpi/96)
  $point=[IntPtr](($py -shl 16) -bor $px)
  [void][ResetFlowNative]::SendMessage($flyout,0x201,1,$point)
  [void][ResetFlowNative]::SendMessage($flyout,0x202,0,$point)
  Start-Sleep -Milliseconds 150
 }
 Click-Fixture 220 315
 Click-Fixture 290 100
 if (Test-Path (Join-Path $fixture 'spent.json')) { throw 'Consume before confirmation' }
 Click-Fixture 95 228
 if (Test-Path (Join-Path $fixture 'spent.json')) { throw 'Cancel consumed credit' }
 Click-Fixture 290 100
 Click-Fixture 270 228
 Start-Sleep -Seconds 3
 if (!(Test-Path (Join-Path $fixture 'spent.json'))) { throw 'Confirmed fixture request missing' }
 $record=Get-Content (Join-Path $fixture 'spent.json') -Raw | ConvertFrom-Json
 if ($record.params.creditId -ne 'fixture-1') { throw 'Wrong fixture credit' }
 $pending=Join-Path $fixture ('CodexStatus/channels/' + $Channel.ToLowerInvariant() + '/reset-attempt.json')
 if ((Get-Content $pending -Raw).Trim() -ne 'null') { throw 'Pending record not cleared' }
 # Only main-page hit coordinates open the list again. Successful refresh must show one credit.
 Click-Fixture 220 315
 if (!(Test-Path (Join-Path $fixture 'post-reset-refresh.json'))) { throw 'Post-redemption quota refresh absent' }
 Write-Output "PASS: real AppState + isolated fake RPC; cancel sends nothing, explicit confirm consumes one fixture, pending cleared, automatic refresh. $fixture"
} finally {
 if (!$process.HasExited) {
  $main=[ResetFlowNative]::FindWindow("CodexStatus.$Channel.MainWindow.v1",'CodexStatus')
  [uint32]$windowPid=0
  [void][ResetFlowNative]::GetWindowThreadProcessId($main,[ref]$windowPid)
  if ($windowPid -eq $process.Id) { [void][ResetFlowNative]::PostMessage($main,0x10,0,0) }
  if (!$process.WaitForExit(3000)) { $process.Kill(); $process.WaitForExit() }
 }
 $process.Dispose()
}
