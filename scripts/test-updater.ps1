param(
    [Parameter(Mandatory = $true)][string]$TestHarness,
    [Parameter(Mandatory = $true)][string]$NewExecutable
)
$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest
$harnessPath = (Resolve-Path -LiteralPath $TestHarness).ProviderPath
$candidatePath = (Resolve-Path -LiteralPath $NewExecutable).ProviderPath
if ([IO.Path]::GetFileName($harnessPath) -notlike 'wardogs_earplugs-*.exe' -or
    [IO.Path]::GetFileName($candidatePath) -notlike 'wardogs-earplugs*.exe') {
    throw 'Select the compiled Earplugs test harness and real Earplugs GUI EXE; never select the game.'
}
$candidateHash = (Get-FileHash -LiteralPath $candidatePath -Algorithm SHA256).Hash
$harnessHash = (Get-FileHash -LiteralPath $harnessPath -Algorithm SHA256).Hash
$expectedSettings = @('earplugs-v1', '55', 'no-such-game.exe', '') -join [char]10
$temporaryPrefix = 'earplugs-updater-ui-'
$temporaryBase = [IO.Path]::GetFullPath([IO.Path]::GetTempPath()).TrimEnd('\')
$temporaryDirectory = Join-Path $temporaryBase ($temporaryPrefix + [guid]::NewGuid().ToString('N') + ' with spaces')
$ownedTargets = [Collections.Generic.List[string]]::new()
$ownedProcesses = [Collections.Generic.List[object]]::new()
$caseDirectories = [Collections.Generic.List[string]]::new()
$results = [Collections.Generic.List[object]]::new()

if (-not ('EarplugsUpdaterUi' -as [type])) {
    Add-Type -TypeDefinition @'
using System;
using System.ComponentModel;
using System.Runtime.InteropServices;
using System.Text;
public static class EarplugsUpdaterUi {
    public const string RootClass = "WarDogsEarplugsWindow";
    [DllImport("user32.dll", CharSet=CharSet.Unicode, ExactSpelling=true)] private static extern IntPtr FindWindowW(string cls, string title);
    [DllImport("user32.dll", CharSet=CharSet.Unicode, ExactSpelling=true)] private static extern IntPtr FindWindowExW(IntPtr parent, IntPtr previous, string cls, string title);
    [DllImport("user32.dll")] private static extern uint GetWindowThreadProcessId(IntPtr hwnd, out uint process);
    [DllImport("user32.dll")] public static extern bool IsWindowVisible(IntPtr hwnd);
    [DllImport("user32.dll")] public static extern IntPtr GetDlgItem(IntPtr hwnd, int id);
    [DllImport("user32.dll", EntryPoint="GetWindowLongPtrW", ExactSpelling=true)] private static extern IntPtr GetWindowLongPtrW(IntPtr hwnd, int index);
    [DllImport("user32.dll", EntryPoint="GetWindowLongW", ExactSpelling=true)] private static extern int GetWindowLongW(IntPtr hwnd, int index);
    [DllImport("user32.dll", EntryPoint="SendMessageTimeoutW", CharSet=CharSet.Unicode, ExactSpelling=true, SetLastError=true)]
    private static extern IntPtr SendCommand(IntPtr hwnd, uint message, UIntPtr wParam, IntPtr lParam, uint flags, uint timeout, out IntPtr result);
    [DllImport("user32.dll", EntryPoint="SendMessageTimeoutW", CharSet=CharSet.Unicode, ExactSpelling=true, SetLastError=true)]
    private static extern IntPtr ReadText(IntPtr hwnd, uint message, UIntPtr wParam, StringBuilder text, uint flags, uint timeout, out IntPtr result);
    private delegate bool ChildCallback(IntPtr hwnd, IntPtr data);
    [DllImport("user32.dll")] private static extern bool EnumChildWindows(IntPtr hwnd, ChildCallback callback, IntPtr data);
    [DllImport("kernel32.dll", CharSet=CharSet.Unicode, ExactSpelling=true, SetLastError=true)] private static extern IntPtr OpenMutexW(uint access, bool inherit, string name);
    [DllImport("kernel32.dll")] private static extern bool CloseHandle(IntPtr handle);
    public static uint Owner(IntPtr hwnd) { uint process; GetWindowThreadProcessId(hwnd, out process); return process; }
    public static IntPtr Root() { return FindWindowW(RootClass, null); }
    public static bool Ready(IntPtr hwnd) {
        return (IntPtr.Size == 8 ? GetWindowLongPtrW(hwnd, -21) : (IntPtr)GetWindowLongW(hwnd, -21)) != IntPtr.Zero;
    }
    public static void Preflight() {
        if (Root() != IntPtr.Zero) throw new InvalidOperationException("Close the real Earplugs app before the updater UI test.");
        IntPtr handle = OpenMutexW(0x00100000, false, @"Local\WarDogsEarplugs");
        int error = Marshal.GetLastWin32Error();
        if (handle != IntPtr.Zero) {
            CloseHandle(handle);
            throw new InvalidOperationException("An Earplugs instance is starting or stopping; wait before the updater UI test.");
        }
        if (error != 2) throw new Win32Exception(error, "Cannot establish that the Earplugs mutex is absent.");
    }
    public static IntPtr Dialog(uint process, string title) {
        IntPtr previous = IntPtr.Zero;
        while (true) {
            previous = FindWindowExW(IntPtr.Zero, previous, "#32770", title);
            if (previous == IntPtr.Zero || Owner(previous) == process) return previous;
        }
    }
    public static void Command(IntPtr hwnd, uint process, uint id) {
        if (hwnd == IntPtr.Zero || Owner(hwnd) != process)
            throw new InvalidOperationException("Refusing to send a command to a window outside the owned test process.");
        IntPtr result;
        if (SendCommand(hwnd, 0x0111, new UIntPtr(id), IntPtr.Zero, 3, 2000, out result) == IntPtr.Zero)
            throw new Win32Exception(Marshal.GetLastWin32Error(), "Owned window did not process the control command.");
    }
    public static string Text(IntPtr hwnd, uint process) {
        if (hwnd == IntPtr.Zero || Owner(hwnd) != process)
            throw new InvalidOperationException("Refusing to read a control outside the owned test process.");
        var text = new StringBuilder(2048);
        IntPtr result;
        if (ReadText(hwnd, 0x000D, new UIntPtr((uint)text.Capacity), text, 3, 2000, out result) == IntPtr.Zero)
            throw new Win32Exception(Marshal.GetLastWin32Error(), "Owned control did not respond.");
        return text.ToString();
    }
    public static string Children(IntPtr hwnd, uint process) {
        var text = new StringBuilder();
        EnumChildWindows(hwnd, (child, data) => { text.AppendLine(Text(child, process)); return true; }, IntPtr.Zero);
        return text.ToString();
    }
}
'@
}

function Assert-True([bool]$Condition, [string]$Message) {
    if (-not $Condition) { throw $Message }
}
function Assert-OwnedPath([Diagnostics.Process]$Process, [string]$ExpectedPath) {
    $Process.Refresh()
    if (-not $Process.HasExited) {
        $actualPath = [IO.Path]::GetFullPath($Process.MainModule.FileName)
        Assert-True ($actualPath.Equals([IO.Path]::GetFullPath($ExpectedPath), [StringComparison]::OrdinalIgnoreCase)) "Refusing an unrelated process $($Process.Id): $actualPath"
    }
}
function Wait-Root([int]$ProcessId, [int]$TimeoutMilliseconds = 10000) {
    $deadline = [Diagnostics.Stopwatch]::StartNew()
    while ($deadline.ElapsedMilliseconds -lt $TimeoutMilliseconds) {
        $root = [EarplugsUpdaterUi]::Root()
        if ($root -ne [IntPtr]::Zero -and [EarplugsUpdaterUi]::Owner($root) -eq $ProcessId -and [EarplugsUpdaterUi]::Ready($root)) {
            return $root
        }
        Start-Sleep -Milliseconds 100
    }
    throw "Owned Earplugs UI $ProcessId did not become ready."
}
function Wait-Dialog([int]$ProcessId, [string]$Title) {
    $deadline = [Diagnostics.Stopwatch]::StartNew()
    while ($deadline.ElapsedMilliseconds -lt 10000) {
        $dialog = [EarplugsUpdaterUi]::Dialog($ProcessId, $Title)
        if ($dialog -ne [IntPtr]::Zero) { return $dialog }
        Start-Sleep -Milliseconds 100
    }
    throw "Owned process $ProcessId did not show '$Title'."
}
function Assert-Preferences([IntPtr]$Root, [int]$ProcessId, [string]$Configuration) {
    Assert-True ([EarplugsUpdaterUi]::Text([EarplugsUpdaterUi]::GetDlgItem($Root, 201), $ProcessId) -eq '55') 'Updater did not preserve the 55% preference.'
    Assert-True ([EarplugsUpdaterUi]::Text([EarplugsUpdaterUi]::GetDlgItem($Root, 202), $ProcessId) -eq 'no-such-game.exe') 'Updater did not preserve the isolated executable filter.'
    Assert-True ([IO.File]::ReadAllText($Configuration) -eq $expectedSettings) 'Updater changed the saved configuration.'
}
function Wait-NoStaging([string]$Directory) {
    $deadline = [Diagnostics.Stopwatch]::StartNew()
    do {
        $files = @(Get-ChildItem -LiteralPath $Directory -Filter '.wardogs-earplugs-update-*' -Force)
        if ($files.Count -eq 0) { return }
        Start-Sleep -Milliseconds 100
    } while ($deadline.ElapsedMilliseconds -lt 10000)
    throw "Updater staging/backup files remain in the owned case directory: $($files.Name -join ', ')"
}
function Close-OwnedProcess($Record) {
    if ($Record.Closed) { return }
    $process = $Record.Process
    $process.Refresh()
    if (-not $process.HasExited) {
        Assert-OwnedPath $process $Record.Path
        try {
            foreach ($dialogChoice in @(
                @('Actualización disponible', 7),
                @('No se pudo actualizar', 1)
            )) {
                $dialog = [EarplugsUpdaterUi]::Dialog($process.Id, $dialogChoice[0])
                if ($dialog -ne [IntPtr]::Zero) { [EarplugsUpdaterUi]::Command($dialog, $process.Id, $dialogChoice[1]) }
            }
            $root = [EarplugsUpdaterUi]::Root()
            if ($root -ne [IntPtr]::Zero -and [EarplugsUpdaterUi]::Owner($root) -eq $process.Id) {
                [EarplugsUpdaterUi]::Command($root, $process.Id, 104)
            }
        } catch {
            Write-Warning "Owned test UI did not exit gracefully: $($_.Exception.Message)"
        }
        if (-not $process.WaitForExit(5000)) {
            Assert-OwnedPath $process $Record.Path
            $process.Kill()
            Assert-True ($process.WaitForExit(5000)) "Owned process $($process.Id) did not exit."
        }
    }
    $Record.Closed = $true
    $process.Dispose()
}
function Register-Replacement([IntPtr]$Root, [string]$Target, [int]$OldProcessId) {
    $newProcessId = [int][EarplugsUpdaterUi]::Owner($Root)
    Assert-True ($newProcessId -ne 0 -and $newProcessId -ne $OldProcessId) 'Update did not create a different UI process.'
    $process = [Diagnostics.Process]::GetProcessById($newProcessId)
    try {
        Assert-OwnedPath $process $Target
        Assert-True (-not $process.HasExited) 'Replacement exited before its UI was verified.'
        $record = [pscustomobject]@{ Process = $process; Path = $Target; Closed = $false }
        $ownedProcesses.Add($record)
        return $record
    } catch {
        $process.Dispose()
        throw
    }
}
function Run-Case([string]$Mode) {
    [EarplugsUpdaterUi]::Preflight()
    $directory = Join-Path $temporaryDirectory ($Mode + ' case with spaces')
    $profileDirectory = Join-Path $directory 'local profile'
    $configuration = Join-Path $profileDirectory 'WarDogsEarplugs\settings.txt'
    $target = Join-Path $directory 'Earplugs portable.exe'
    New-Item -ItemType Directory -Path ([IO.Path]::GetDirectoryName($configuration)) | Out-Null
    $caseDirectories.Add($directory)
    Copy-Item -LiteralPath $harnessPath -Destination $target
    [IO.File]::SetAttributes($target, [IO.FileAttributes]::Normal)
    $ownedTargets.Add([IO.Path]::GetFullPath($target))
    [IO.File]::WriteAllText($configuration, $expectedSettings, [Text.UTF8Encoding]::new($false))
    $start = [Diagnostics.ProcessStartInfo]::new()
    $start.FileName = $target
    $start.WorkingDirectory = $directory
    $start.UseShellExecute = $false
    $start.CreateNoWindow = $true
    $start.RedirectStandardOutput = $true
    $start.RedirectStandardError = $true
    $arguments = @('--exact', 'update::tests::updater_ui_fixture', '--ignored', '--nocapture')
    if ($start.PSObject.Properties['ArgumentList']) {
        foreach ($argument in $arguments) { $start.ArgumentList.Add($argument) }
    } else {
        # Only constant, space-free test arguments; no user path is interpolated.
        $start.Arguments = $arguments -join ' '
    }
    $start.EnvironmentVariables['LOCALAPPDATA'] = $profileDirectory
    $start.EnvironmentVariables['EARPLUGS_TEST_UPDATE_SOURCE'] = $candidatePath
    [void]$start.EnvironmentVariables.Remove('EARPLUGS_TEST_BAD_UPDATE_HASH')
    if ($Mode -eq 'bad hash') { $start.EnvironmentVariables['EARPLUGS_TEST_BAD_UPDATE_HASH'] = '1' }
    $process = [Diagnostics.Process]::Start($start)
    $stdout = $process.StandardOutput.ReadToEndAsync()
    $stderr = $process.StandardError.ReadToEndAsync()
    $record = [pscustomobject]@{ Process = $process; Path = $target; Closed = $false; Stdout = $stdout; Stderr = $stderr }
    $ownedProcesses.Add($record)
    $replacement = $null
    try {
        $root = Wait-Root $process.Id
        Assert-Preferences $root $process.Id $configuration
        $initiallyVisible = [EarplugsUpdaterUi]::IsWindowVisible($root)
        Assert-True $initiallyVisible 'The test fixture must start in window mode; --background is not a Rust test-runner argument.'
        $dialog = Wait-Dialog $process.Id 'Actualización disponible'
        [EarplugsUpdaterUi]::Command($dialog, $process.Id, $(if ($Mode -eq 'decline') { 7 } else { 6 }))
        if ($Mode -eq 'decline') {
            Start-Sleep -Milliseconds 300
            $process.Refresh()
            Assert-True (-not $process.HasExited) 'Declining the update stopped the original app.'
            Assert-True ((Get-FileHash -LiteralPath $target -Algorithm SHA256).Hash -eq $harnessHash) 'Declining changed the original EXE.'
            Wait-NoStaging $directory
        } elseif ($Mode -eq 'bad hash') {
            $errorDialog = Wait-Dialog $process.Id 'No se pudo actualizar'
            $errorText = [EarplugsUpdaterUi]::Children($errorDialog, $process.Id)
            Assert-True ($errorText.Contains('SHA-256')) "Bad hash did not produce a checksum error: $errorText"
            [EarplugsUpdaterUi]::Command($errorDialog, $process.Id, 1)
            $process.Refresh()
            Assert-True (-not $process.HasExited) 'Checksum rejection stopped the original app.'
            Assert-True ((Get-FileHash -LiteralPath $target -Algorithm SHA256).Hash -eq $harnessHash) 'Checksum rejection replaced the original EXE.'
            Wait-NoStaging $directory
        } else {
            Assert-True ($process.WaitForExit(15000)) 'The accepted update did not stop its original parent.'
            Assert-True ($process.ExitCode -eq 0) "Original fixture exited with $($process.ExitCode)."
            $deadline = [Diagnostics.Stopwatch]::StartNew()
            $newRoot = [IntPtr]::Zero
            while ($deadline.ElapsedMilliseconds -lt 15000) {
                $found = [EarplugsUpdaterUi]::Root()
                if ($found -ne [IntPtr]::Zero -and [EarplugsUpdaterUi]::Owner($found) -ne $process.Id -and [EarplugsUpdaterUi]::Ready($found)) {
                    $replacement = Register-Replacement $found $target $process.Id
                    $newRoot = $found
                    break
                }
                Start-Sleep -Milliseconds 100
            }
            Assert-True ($newRoot -ne [IntPtr]::Zero) 'No owned replacement UI became ready within fifteen seconds.'
            Assert-True ((Get-FileHash -LiteralPath $target -Algorithm SHA256).Hash -eq $candidateHash) 'Installed EXE does not match the selected candidate.'
            Assert-Preferences $newRoot $replacement.Process.Id $configuration
            Assert-True ([EarplugsUpdaterUi]::IsWindowVisible($newRoot) -eq $initiallyVisible) 'Update changed the initial window/tray mode.'
            Wait-NoStaging $directory
        }
        Assert-True ((Get-FileHash -LiteralPath $candidatePath -Algorithm SHA256).Hash -eq $candidateHash) 'The selected candidate source was modified.'
        Assert-True ([IO.File]::ReadAllText($configuration) -eq $expectedSettings) 'Saved settings changed.'
        $results.Add([pscustomobject]@{ Case = $Mode; Result = 'PASS'; InitialMode = 'window'; KeyboardInjection = $false })
    } finally {
        try {
            if ($replacement) { Close-OwnedProcess $replacement }
        } finally {
            Close-OwnedProcess $record
        }
    }
}
function Cleanup-OwnedProcesses {
    foreach ($record in $ownedProcesses) {
        if (-not $record.Closed) { Close-OwnedProcess $record }
    }
    # A handoff can finish just after an assertion fails. Stop only helper/portable
    # process names whose image paths belong to this run, including pre-UI launches.
    for ($attempt = 0; $attempt -lt 5; $attempt++) {
        foreach ($directory in $caseDirectories) {
            foreach ($file in @(Get-ChildItem -LiteralPath $directory -Filter '.wardogs-earplugs-update-*.exe' -Force)) {
                foreach ($process in [Diagnostics.Process]::GetProcessesByName([IO.Path]::GetFileNameWithoutExtension($file.Name))) {
                    try {
                        if (-not $process.HasExited -and
                            [IO.Path]::GetFullPath($process.MainModule.FileName).Equals($file.FullName, [StringComparison]::OrdinalIgnoreCase)) {
                            $process.Kill()
                            Assert-True ($process.WaitForExit(5000)) 'Owned temporary helper did not exit.'
                        }
                    } catch {
                        $process.Refresh()
                        if (-not $process.HasExited) { throw }
                    } finally {
                        $process.Dispose()
                    }
                }
            }
        }
        foreach ($process in [Diagnostics.Process]::GetProcessesByName('Earplugs portable')) {
            try {
                if ($process.HasExited) { continue }
                $path = [IO.Path]::GetFullPath($process.MainModule.FileName)
                $matched = @($ownedTargets | Where-Object { $_.Equals($path, [StringComparison]::OrdinalIgnoreCase) })
                if ($matched.Count -gt 0) {
                    $record = [pscustomobject]@{ Process = $process; Path = $matched[0]; Closed = $false }
                    $ownedProcesses.Add($record)
                    Close-OwnedProcess $record
                }
            } catch {
                $process.Refresh()
                if (-not $process.HasExited) { throw }
            } finally {
                $process.Dispose()
            }
        }
        Start-Sleep -Milliseconds 100
    }
}

[EarplugsUpdaterUi]::Preflight()
New-Item -ItemType Directory -Path $temporaryDirectory | Out-Null
try {
    foreach ($mode in @('decline', 'bad hash', 'accept')) { Run-Case $mode }
} finally {
    Cleanup-OwnedProcesses
    $resolvedTemporary = (Resolve-Path -LiteralPath $temporaryDirectory).ProviderPath
    $resolvedBase = (Resolve-Path -LiteralPath $temporaryBase).ProviderPath.TrimEnd('\') + '\'
    $resolvedName = [IO.Path]::GetFileName($resolvedTemporary.TrimEnd('\'))
    Assert-True ($resolvedTemporary.StartsWith($resolvedBase, [StringComparison]::OrdinalIgnoreCase) -and
        $resolvedName.StartsWith($temporaryPrefix, [StringComparison]::Ordinal) -and
        [IO.Path]::GetFullPath($resolvedTemporary).Equals([IO.Path]::GetFullPath($temporaryDirectory), [StringComparison]::OrdinalIgnoreCase)) 'Refusing to delete a directory outside this owned TEMP test run.'
    Assert-True (-not ((Get-Item -LiteralPath $resolvedTemporary -Force).Attributes -band [IO.FileAttributes]::ReparsePoint)) 'Refusing recursive cleanup through a reparse point.'
    Remove-Item -LiteralPath $resolvedTemporary -Recurse -Force
}
$results | ConvertTo-Json
