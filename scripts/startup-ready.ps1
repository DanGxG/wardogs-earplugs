param([Parameter(Mandatory = $true)][string]$Executable)
$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest
$resolvedExecutable = (Resolve-Path -LiteralPath $Executable).Path
if (-not (Test-Path -LiteralPath $resolvedExecutable -PathType Leaf) -or
    [IO.Path]::GetExtension($resolvedExecutable) -ne '.exe' -or
    [IO.Path]::GetFileName($resolvedExecutable) -notlike 'wardogs-earplugs*.exe') {
    throw 'Select a WarDogs Earplugs EXE; this test must not launch the game or another program.'
}

if (-not ('EarplugsStartupReadyFixture' -as [type])) {
    Add-Type -TypeDefinition @'
using System;
using System.ComponentModel;
using System.Runtime.InteropServices;
using System.Threading;

public sealed class EarplugsStartupReadyFixture : IDisposable {
    private const string WindowClass = "WarDogsEarplugsWindow";
    private const string WindowTitle = "WarDogs Earplugs";
    private const string MutexName = @"Local\WarDogsEarplugs";
    private const string EventName = @"Local\WarDogsEarplugsReady";
    private const int ErrorAlreadyExists = 183;
    private const int ErrorFileNotFound = 2;
    private const uint WmQuit = 0x0012;
    private readonly ManualResetEventSlim started = new ManualResetEventSlim(false);
    private readonly WindowProcedure procedure;
    private Thread guiThread;
    private uint guiThreadId;
    private IntPtr window;
    private IntPtr mutex;
    private IntPtr ready;
    private Exception startupFailure;
    private volatile bool stopRequested;
    private int disposed;

    public EarplugsStartupReadyFixture() {
        procedure = WindowProc;
        try {
            if (FindWindowW(WindowClass, null) != IntPtr.Zero)
                throw new InvalidOperationException("Close the real Earplugs app before this isolated startup test.");
            AssertNamedObjectAbsent(MutexName, true);
            AssertNamedObjectAbsent(EventName, false);

            mutex = CreateMutexW(IntPtr.Zero, false, MutexName);
            int error = Marshal.GetLastWin32Error();
            if (mutex == IntPtr.Zero) throw new Win32Exception(error, "Cannot create test mutex.");
            if (error == ErrorAlreadyExists)
                throw new InvalidOperationException("An Earplugs instance started during fixture setup; refusing the test.");

            ready = CreateEventW(IntPtr.Zero, true, false, EventName);
            error = Marshal.GetLastWin32Error();
            if (ready == IntPtr.Zero) throw new Win32Exception(error, "Cannot create test ready event.");
            if (error == ErrorAlreadyExists)
                throw new InvalidOperationException("A ready event appeared during setup; refusing to modify it.");

            guiThread = new Thread(Pump);
            guiThread.IsBackground = true;
            guiThread.SetApartmentState(ApartmentState.STA);
            guiThread.Start();
            if (!started.Wait(5000))
                throw new TimeoutException("The private startup fixture did not initialize.");
            if (startupFailure != null)
                throw new InvalidOperationException("The private startup fixture failed.", startupFailure);
            if (GetUserData(window) != IntPtr.Zero || IsWindowVisible(window))
                throw new InvalidOperationException("Startup fixture must be hidden with zero GWLP_USERDATA.");
        } catch {
            Dispose();
            throw;
        }
    }

    private static void AssertNamedObjectAbsent(string name, bool isMutex) {
        IntPtr handle = isMutex
            ? OpenMutexW(0x00100000, false, name)
            : OpenEventW(0x00100000, false, name);
        int error = Marshal.GetLastWin32Error();
        if (handle != IntPtr.Zero) {
            CloseHandle(handle);
            throw new InvalidOperationException("A real instance or another test owns " + name + "; close it first.");
        }
        if (error != ErrorFileNotFound)
            throw new Win32Exception(error, "Cannot establish that " + name + " is absent.");
    }

    private void Pump() {
        IntPtr instance = GetModuleHandleW(null);
        bool registered = false;
        try {
            guiThreadId = GetCurrentThreadId();
            WndClass cls = new WndClass();
            cls.procedure = procedure;
            cls.instance = instance;
            cls.className = WindowClass;
            if (RegisterClassW(ref cls) == 0)
                throw new Win32Exception(Marshal.GetLastWin32Error(), "Cannot register fixture class.");
            registered = true;
            // Ordinary top-level window, deliberately hidden and not initialized.
            window = CreateWindowExW(0, WindowClass, WindowTitle, 0x80000000u | 0x00C00000u,
                100, 100, 320, 80, IntPtr.Zero, IntPtr.Zero, instance, IntPtr.Zero);
            if (window == IntPtr.Zero)
                throw new Win32Exception(Marshal.GetLastWin32Error(), "Cannot create fixture window.");
            started.Set();
            Msg message;
            while (!stopRequested) {
                int received = GetMessageW(out message, IntPtr.Zero, 0, 0);
                if (received == 0) break;
                if (received == -1)
                    throw new Win32Exception(Marshal.GetLastWin32Error(), "Fixture GetMessage failed.");
                TranslateMessage(ref message);
                DispatchMessageW(ref message);
            }
        } catch (Exception error) {
            startupFailure = error;
        } finally {
            started.Set();
            if (window != IntPtr.Zero && IsWindow(window)) DestroyWindow(window);
            if (registered) UnregisterClassW(WindowClass, instance);
        }
    }

    private static IntPtr WindowProc(IntPtr hwnd, uint message, UIntPtr wParam, IntPtr lParam) {
        if (message == 0x0002) { PostQuitMessage(0); return IntPtr.Zero; }
        return DefWindowProcW(hwnd, message, wParam, lParam);
    }

    private static IntPtr GetUserData(IntPtr hwnd) {
        return IntPtr.Size == 8 ? GetWindowLongPtrW(hwnd, -21) : (IntPtr)GetWindowLongW(hwnd, -21);
    }

    public bool Visible { get { return IsWindowVisible(window); } }

    public void SignalReady() {
        if (startupFailure != null)
            throw new InvalidOperationException("Fixture message pump failed.", startupFailure);
        if (!SetEvent(ready))
            throw new Win32Exception(Marshal.GetLastWin32Error(), "Cannot signal test readiness.");
    }

    public void Dispose() {
        if (Interlocked.Exchange(ref disposed, 1) != 0) return;
        stopRequested = true;
        bool stopped = true;
        if (guiThread != null && guiThread.IsAlive) {
            PostThreadMessageW(guiThreadId, WmQuit, UIntPtr.Zero, IntPtr.Zero);
            stopped = guiThread.Join(5000);
        }
        if (ready != IntPtr.Zero) { CloseHandle(ready); ready = IntPtr.Zero; }
        if (mutex != IntPtr.Zero) { CloseHandle(mutex); mutex = IntPtr.Zero; }
        if (stopped) started.Dispose();
        GC.KeepAlive(procedure);
        if (!stopped)
            throw new TimeoutException("Owned fixture thread did not stop; no unrelated thread was terminated.");
    }

    [UnmanagedFunctionPointer(CallingConvention.Winapi)]
    private delegate IntPtr WindowProcedure(IntPtr window, uint message, UIntPtr wParam, IntPtr lParam);
    [StructLayout(LayoutKind.Sequential, CharSet = CharSet.Unicode)]
    private struct WndClass {
        public uint style;
        public WindowProcedure procedure;
        public int classExtra, windowExtra;
        public IntPtr instance, icon, cursor, background;
        public string menuName, className;
    }
    [StructLayout(LayoutKind.Sequential)]
    private struct Point { public int x, y; }
    [StructLayout(LayoutKind.Sequential)]
    private struct Msg {
        public IntPtr window;
        public uint message;
        public UIntPtr wParam;
        public IntPtr lParam;
        public uint time;
        public Point point;
        public uint privateValue;
    }

    [DllImport("kernel32.dll", CharSet = CharSet.Unicode, SetLastError = true)] private static extern IntPtr CreateMutexW(IntPtr attributes, bool initialOwner, string name);
    [DllImport("kernel32.dll", CharSet = CharSet.Unicode, SetLastError = true)] private static extern IntPtr OpenMutexW(uint access, bool inherit, string name);
    [DllImport("kernel32.dll", CharSet = CharSet.Unicode, SetLastError = true)] private static extern IntPtr CreateEventW(IntPtr attributes, bool manualReset, bool initialState, string name);
    [DllImport("kernel32.dll", CharSet = CharSet.Unicode, SetLastError = true)] private static extern IntPtr OpenEventW(uint access, bool inherit, string name);
    [DllImport("kernel32.dll", SetLastError = true)] private static extern bool SetEvent(IntPtr handle);
    [DllImport("kernel32.dll")] private static extern bool CloseHandle(IntPtr handle);
    [DllImport("kernel32.dll")] private static extern uint GetCurrentThreadId();
    [DllImport("kernel32.dll", CharSet = CharSet.Unicode)] private static extern IntPtr GetModuleHandleW(string name);
    [DllImport("user32.dll", CharSet = CharSet.Unicode)] private static extern IntPtr FindWindowW(string cls, string title);
    [DllImport("user32.dll", CharSet = CharSet.Unicode, SetLastError = true)] private static extern ushort RegisterClassW(ref WndClass cls);
    [DllImport("user32.dll", CharSet = CharSet.Unicode)] private static extern bool UnregisterClassW(string cls, IntPtr instance);
    [DllImport("user32.dll", CharSet = CharSet.Unicode, SetLastError = true)] private static extern IntPtr CreateWindowExW(uint exStyle, string cls, string title, uint style, int x, int y, int width, int height, IntPtr parent, IntPtr menu, IntPtr instance, IntPtr data);
    [DllImport("user32.dll", SetLastError = true)] private static extern bool DestroyWindow(IntPtr hwnd);
    [DllImport("user32.dll")] private static extern bool IsWindow(IntPtr hwnd);
    [DllImport("user32.dll")] private static extern bool IsWindowVisible(IntPtr hwnd);
    [DllImport("user32.dll", EntryPoint = "GetWindowLongPtrW")] private static extern IntPtr GetWindowLongPtrW(IntPtr hwnd, int index);
    [DllImport("user32.dll", EntryPoint = "GetWindowLongW")] private static extern int GetWindowLongW(IntPtr hwnd, int index);
    [DllImport("user32.dll", CharSet = CharSet.Unicode, SetLastError = true)] private static extern int GetMessageW(out Msg message, IntPtr hwnd, uint minimum, uint maximum);
    [DllImport("user32.dll")] private static extern bool TranslateMessage(ref Msg message);
    [DllImport("user32.dll", CharSet = CharSet.Unicode)] private static extern IntPtr DispatchMessageW(ref Msg message);
    [DllImport("user32.dll", CharSet = CharSet.Unicode)] private static extern IntPtr DefWindowProcW(IntPtr hwnd, uint message, UIntPtr wParam, IntPtr lParam);
    [DllImport("user32.dll", SetLastError = true)] private static extern bool PostThreadMessageW(uint thread, uint message, UIntPtr wParam, IntPtr lParam);
    [DllImport("user32.dll")] private static extern void PostQuitMessage(int code);
}
'@
}

$fixture = $null
$child = $null
$result = $null
try {
    $fixture = [EarplugsStartupReadyFixture]::new()
    $child = Start-Process -FilePath $resolvedExecutable -ArgumentList '--background' -PassThru -WindowStyle Hidden
    Start-Sleep -Milliseconds 200
    $child.Refresh()
    $prematureExit = $child.HasExited
    $prematureVisibility = $fixture.Visible
    if ($prematureExit -or $prematureVisibility) {
        throw "Second instance did not wait for startup readiness: exited=$prematureExit; fixtureVisible=$prematureVisibility."
    }
    $fixture.SignalReady()
    if (-not $child.WaitForExit(5000)) {
        throw 'Second instance did not exit within five seconds after startup readiness was signaled.'
    }
    if ($child.ExitCode -ne 0) {
        throw "Second instance returned failure exit code $($child.ExitCode)."
    }
    if (-not $fixture.Visible) {
        throw 'Second instance did not show the now-ready fixture window.'
    }
    $result = [pscustomobject]@{
        Executable = $resolvedExecutable
        Result = 'PASS: second launch stayed hidden and alive until readiness, then showed the fixture and exited'
        KeyboardInjection = $false
        AudioAccess = $false
    }
} finally {
    try {
        if ($child) {
            $child.Refresh()
            if (-not $child.HasExited) {
                # This process object is only the selected child launched by this test.
                $child.Kill()
                if (-not $child.WaitForExit(5000)) { throw 'Owned test child did not terminate.' }
            }
            $child.Dispose()
        }
    } finally {
        if ($fixture) { $fixture.Dispose() }
    }
}
$result | ConvertTo-Json
