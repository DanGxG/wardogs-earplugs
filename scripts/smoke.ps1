param([Parameter(Mandatory = $true)][string]$Executable)
$ErrorActionPreference = 'Stop'
Add-Type -TypeDefinition @'
using System;
using System.Text;
using System.Runtime.InteropServices;
public static class EarplugsSmoke {
  [DllImport("user32.dll", CharSet=CharSet.Unicode)] public static extern IntPtr FindWindow(string cls, string title);
  [DllImport("user32.dll")] public static extern IntPtr GetDlgItem(IntPtr window, int id);
  [DllImport("user32.dll", EntryPoint="SendMessageW", CharSet=CharSet.Unicode)] private static extern IntPtr SendText(IntPtr window, uint message, IntPtr wParam, string text);
  [DllImport("user32.dll", EntryPoint="SendMessageW", CharSet=CharSet.Unicode)] private static extern IntPtr ReadText(IntPtr window, uint message, IntPtr wParam, StringBuilder text);
  public static void SetText(IntPtr window, string text) { SendText(window, 0x000c, IntPtr.Zero, text); }
  [DllImport("user32.dll")] public static extern IntPtr SendMessage(IntPtr window, uint message, IntPtr wParam, IntPtr lParam);
  [DllImport("user32.dll")] public static extern bool PostMessage(IntPtr window, uint message, IntPtr wParam, IntPtr lParam);
  [DllImport("user32.dll")] public static extern bool IsWindowVisible(IntPtr window);
  [DllImport("user32.dll")] public static extern bool EnumChildWindows(IntPtr window, ChildCallback callback, IntPtr data);
  public delegate bool ChildCallback(IntPtr window, IntPtr data);
  public static string Text(IntPtr window) { var text = new StringBuilder(512); ReadText(window, 0x000d, (IntPtr)text.Capacity, text); return text.ToString(); }
  public static string ChildTexts(IntPtr window) {
    var all = new StringBuilder();
    EnumChildWindows(window, (child, data) => { all.AppendLine(Text(child)); return true; }, IntPtr.Zero);
    return all.ToString();
  }
}
'@
function Assert-True([bool]$Condition, [string]$Message) { if (-not $Condition) { throw $Message } }
Assert-True ([EarplugsSmoke]::FindWindow('WarDogsEarplugsWindow', 'WarDogs Earplugs') -eq [IntPtr]::Zero) 'Close the running Earplugs app before the isolated UI smoke test.'
$profileDirectory = Join-Path (Get-Location) 'test-output/smoke-profile'
New-Item -ItemType Directory -Path $profileDirectory -Force | Out-Null
$config = Join-Path $profileDirectory 'WarDogsEarplugs/settings.txt'
if (Test-Path -LiteralPath $config) { Remove-Item -LiteralPath $config }
$previousLocalAppData = $env:LOCALAPPDATA
$process = $null
try {
    $env:LOCALAPPDATA = $profileDirectory
    $process = Start-Process -FilePath (Resolve-Path -LiteralPath $Executable).Path -ArgumentList '--background' -PassThru -WindowStyle Hidden
    $env:LOCALAPPDATA = $previousLocalAppData
    $window = [IntPtr]::Zero
    for ($attempt = 0; $attempt -lt 50; $attempt++) {
        $window = [EarplugsSmoke]::FindWindow('WarDogsEarplugsWindow', 'WarDogs Earplugs')
        if ($window -ne [IntPtr]::Zero) { break }
        Start-Sleep -Milliseconds 100
    }
    Assert-True ($window -ne [IntPtr]::Zero) 'App did not create its window.'
    Assert-True (-not [EarplugsSmoke]::IsWindowVisible($window)) '--background did not start hidden (F2 conflict or tray failure).'
    $overlay = [EarplugsSmoke]::FindWindow('WarDogsEarplugsOverlay', 'Earplugs ON')
    Assert-True ($overlay -ne [IntPtr]::Zero) 'Independent overlay window did not initialize.'
    Assert-True (-not [EarplugsSmoke]::IsWindowVisible($overlay)) 'Overlay was visible while OFF.'
    $percent = [EarplugsSmoke]::GetDlgItem($window, 201)
    $name = [EarplugsSmoke]::GetDlgItem($window, 202)
    Assert-True ([EarplugsSmoke]::Text($percent) -eq '40') 'Default percent was not 40.'
    Assert-True ([EarplugsSmoke]::Text($name) -eq 'wardogs') 'Default executable name was wrong.'
    [EarplugsSmoke]::SetText($percent, '101')
    [void][EarplugsSmoke]::SendMessage($window, 0x111, [IntPtr]101, [IntPtr]::Zero)
    $invalidState = [EarplugsSmoke]::ChildTexts($window)
    Assert-True ($invalidState.Contains('Enter a whole percentage from 0 to 100.')) "Invalid percentage was accepted. Controls: $invalidState"
    Assert-True (-not (Test-Path -LiteralPath $config)) 'Invalid input wrote a settings file.'
    [EarplugsSmoke]::SetText($percent, '25')
    [EarplugsSmoke]::SetText($name, 'no-such-game-fixture.exe')
    [void][EarplugsSmoke]::SendMessage($window, 0x111, [IntPtr]101, [IntPtr]::Zero)
    Assert-True ((Get-Content -LiteralPath $config -Raw).Contains("25`nno-such-game-fixture.exe")) 'Settings did not persist.'
    [void][EarplugsSmoke]::SendMessage($window, 0x111, [IntPtr]102, [IntPtr]::Zero)
    Assert-True ([EarplugsSmoke]::ChildTexts($window).Contains('No matching game audio.')) 'Toggle did not report the missing game.'
    Assert-True (-not [EarplugsSmoke]::IsWindowVisible($overlay)) 'Missing game incorrectly showed the ON overlay.'
    [void][EarplugsSmoke]::SendMessage($window, 0x10, [IntPtr]::Zero, [IntPtr]::Zero)
    Assert-True (-not [EarplugsSmoke]::IsWindowVisible($window)) 'Close did not hide to the tray.'
    $second = Start-Process -FilePath (Resolve-Path -LiteralPath $Executable).Path -PassThru -WindowStyle Hidden
    Assert-True ($second.WaitForExit(5000)) 'A second long-lived app instance was created.'
    Assert-True ([EarplugsSmoke]::IsWindowVisible($window)) 'Second launch did not reopen the existing window.'
    [void][EarplugsSmoke]::SendMessage($window, 0x10, [IntPtr]::Zero, [IntPtr]::Zero)
    Start-Sleep -Seconds 1
    $process.Refresh()
    $cpuBefore = $process.TotalProcessorTime.TotalMilliseconds
    $workingSet = $process.WorkingSet64
    $privateBytes = $process.PrivateMemorySize64
    Start-Sleep -Seconds 15
    $process.Refresh()
    $cpuAfter = $process.TotalProcessorTime.TotalMilliseconds
    [pscustomobject]@{
        ExecutableBytes = (Get-Item -LiteralPath $Executable).Length
        WorkingSetBytes = $workingSet
        PrivateBytes = $privateBytes
        IdleWallSeconds = 15
        IdleCpuMilliseconds = $cpuAfter - $cpuBefore
        Threads = $process.Threads.Count
        Signature = (Get-AuthenticodeSignature -LiteralPath $Executable).Status.ToString()
        Result = 'PASS: defaults, validation, atomic persistence, toggle dispatch, close-to-tray, single instance, background idle, exit'
    } | ConvertTo-Json | Tee-Object -FilePath 'test-output/smoke-results.json'
    [void][EarplugsSmoke]::PostMessage($window, 0x111, [IntPtr]104, [IntPtr]::Zero)
    Assert-True ($process.WaitForExit(5000)) 'Exit did not stop the app.'
    Assert-True ([EarplugsSmoke]::FindWindow('WarDogsEarplugsOverlay', 'Earplugs ON') -eq [IntPtr]::Zero) 'Overlay survived Exit.'
} finally {
    $env:LOCALAPPDATA = $previousLocalAppData
    if ($process -and -not $process.HasExited) {
        if ($window -ne [IntPtr]::Zero) { [void][EarplugsSmoke]::PostMessage($window, 0x111, [IntPtr]104, [IntPtr]::Zero) }
        if (-not $process.WaitForExit(3000)) { $process.Kill() }
    }
}
