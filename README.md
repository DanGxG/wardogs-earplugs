# WarDogs Earplugs

A tiny native Windows tray app written in Rust. **F2 sets only WarDogs' Windows mixer volume to the chosen percentage of its original level (40% by default), then restores that original level on the next press.** An independent top-left desktop badge displays **Earplugs ON** while active. Discord, music, other games and Windows master volume stay at their existing levels.

## Use

1. Download the Windows x64 release EXE, or unzip the portable package. Requires Windows 10/11 x64. No Python, installer or administrator access is needed.
   For a manual update, **Exit the old version first**; launching a new copy while the old one is running reopens the old instance. Versions up to v0.1.3 cannot update themselves: install v0.1.5 manually once to get the startup update check. Version v0.1.4 can offer v0.1.5 when it starts.
2. Run `wardogs-earplugs.exe` and start WarDogs so it creates a Windows audio session.
3. Enter a whole **Original volume kept (%)** from **0 to 100**. Default is **40**. This is the percentage of each session's original Windows mixer volume to keep. Original levels are captured separately when you turn earplugs on. **0** silences the sessions; **100** keeps their original levels.
4. Leave **Game executable contains** at `wardogs`, or enter the actual name shown in Task Manager's **Details** tab. Matching ignores letter case. A complete name ending in `.exe` matches exactly; otherwise it matches that distinctive part of the filename. Click **Apply**. While earplugs are on, Apply updates the sessions immediately using the same captured originals: with an original volume of **80%**, choosing **40 / 60 / 80** gives **32% / 48% / 64%**. Repeated Apply changes do not accumulate reductions. It can refresh a restarted game's matching sessions; if restoration is already partly complete, finish recovery with F2 before applying again.
5. Press **F2** to turn earplugs on. Press **F2** again to restore the game; the badge hides when recovery completes. The Toggle button does the same thing.
6. **Hide** or the window's close button keeps the app in the system tray. Click the tray icon to reopen settings; right-click for the menu. **Exit** restores volume and stops the app only when recovery completes.

**Changed in v0.1.5:** saved numeric values are preserved, but now represent a percentage of the original volume. Versions v0.1.1–v0.1.4 used absolute mixer targets. A saved value of **40** with an original mixer level of **80%** now gives **32%**.

`wardogs-earplugs.exe --background` starts directly in the tray when startup checks succeed; configuration errors show settings instead. For optional startup with Windows, create a shortcut to the EXE in `shell:startup`, with `--background` in its Target. The app never enables startup itself.

F2 is observed through a Windows Raw Input background listener (`RIDEV_INPUTSINK`), independently of held movement keys or Ctrl, Shift, Alt and Windows modifiers. Press state is tracked per keyboard, allowing one toggle per F2 press even if modifiers change while it remains held. Removing a keyboard discards its held state; resume and session unlock clear stale state. It uses no keyboard hooks or polling.

F2 and modifier+F2 combinations are still delivered normally to the game and other apps. Remove conflicting bindings if you want only the earplugs action; other Windows shortcuts continue to work. The full live keyboard matrix and game-specific input behavior remain unverified; see the testing limits below and [VALIDATION.md](VALIDATION.md).

The badge is a small, independent Windows window at the top-left of the primary screen. It stays above ordinary desktop and borderless/windowed game windows, lets clicks pass through, and does not take keyboard focus. It has no owner or attachment to the game window. Exclusive fullscreen may cover it. It appears only while earplugs are on and uses no timer or polling loop.

## Updates

Version 0.1.5 checks GitHub's latest public `SHA256SUMS.txt` once at startup, including `--background`. A newer stable `major.minor.patch` version opens a Spanish **Sí / No** prompt with **No** selected by default. **No** keeps the current version and downloads no EXE. Offline checks, unavailable releases and an up-to-date copy stay silent. There is no periodic check or installed service.

**Sí** downloads the offered version using the asset name and SHA-256 hash captured during the check. Native Windows HTTPS and streaming hashing verify the download against that manifest and check the Windows x64 executable format. Assets advertised as signed also require Windows Authenticode trust validation; unsigned assets remain unsigned. A matching hash proves agreement with the release manifest, not a trusted publisher signature.

Before replacement, Earplugs restores the game volume. Failed recovery or handoff cancels the update and keeps the current app open. A temporary helper waits for its Earplugs parent to exit, backs up the old EXE, replaces it at the same path and restarts with the same settings and current tray/window mode. It removes the backup only after the new app's UI reports readiness; a failed startup restores the previous EXE. Updating requires write permission to the EXE folder and does not request administrator access. If that folder is protected, update manually from a writable location.

## Small by design

- One UI thread, native Win32 controls, native tray icon and a blocking Windows message loop. Update checks/downloads use temporary workers; accepted updates use a temporary helper process.
- No periodic polling, timer-based monitors, persistent worker threads, embedded browser, Python interpreter or telemetry. Only startup update checks and accepted downloads use the network.
- The keyboard listener wakes on Windows keyboard events, reads a fixed stack buffer and ignores non-F2 keys. It records no keyboard events or typed text.
- Audio/process enumeration happens only when a volume change or restoration needs it. Live sessions use saved Windows audio interfaces; recovery of ended sessions searches their original output endpoint. There is no restart monitor.
- Ordinary unchanged F2 presses perform no settings-file writes.
- One running instance per Windows login session. Release builds optimize for size and link the MSVC runtime statically.

A startup readiness event lets a second launch wait for the existing instance to finish initialization before opening its settings. It adds no background polling.

Memory and CPU depend on Windows, audio drivers and tray/UI state. See [VALIDATION.md](VALIDATION.md) for measured results and their limits.

## Windows integration and reliability

Earplugs uses Windows Raw Input with `RegisterRawInputDevices` and `RIDEV_INPUTSINK`, a process-list snapshot containing filenames/PIDs, Core Audio mixer sessions and an independent native desktop badge. It does not open the game's process, read/write its memory, inject code, load a game plugin, hook input or the game window, intercept graphics or install a driver. The process snapshot is used solely to associate the named game's PIDs with its Windows audio sessions. It checks all active output devices, including games routed to a different output.

The updater uses Windows WinHTTP, BCrypt and WinVerifyTrust without new Rust libraries. Its process handles target only its own Earplugs parent/helper for replacement and readiness, never the game.

Anti-cheat classification belongs to the game's vendor; this implementation cannot promise approval or that a future policy will accept a background keyboard utility.

Version 0.1.5 retains the device-state, startup and recovery behavior described here. Completed checks and remaining limits are recorded in [VALIDATION.md](VALIDATION.md). The full live keyboard matrix remains unverified.

Actions received during a tray menu or a nested Core Audio call are queued and processed after the current handler releases application state. Close and Exit cancel an open menu so the request can proceed; a nested close keeps its normal hide-to-tray behavior. Startup and shutdown use shared resource cleanup.

Previous mixer volume and recovery records are kept in memory. Each session's applied level is its captured original multiplied by the chosen percentage divided by 100. Apply uses that same original while earplugs remain on, and F2 captures fresh originals the next time you activate them. Restore writes only when the current mixer level still matches the last value Earplugs applied, preserving newer manual mixer edits. The app does not change mute state.

**Turn earplugs OFF, or Exit, before closing the game when possible.** Writing to an expired session does not repair the mixer level inherited by a new game process. If the game has already closed or its output disappeared, keep Earplugs running, restart the game or reconnect its original output, then press F2 to finish recovery. Normal Exit refuses to stop while recovery remains pending. Windows shutdown attempts restoration, but a crash, power loss or force quit loses the in-memory recovery data; use Windows Volume Mixer to correct the level if needed.

During F2, Apply or an Exit/shutdown restoration attempt, an ended session can be replaced only by a unique new live session on the same Windows output endpoint with the same opaque persistent audio-session identifier. All originally captured instances are excluded, including ones already restored, so one old record cannot claim another captured instance. If the match is missing or ambiguous, recovery remains pending. If ended instances had conflicting original levels, choose the correct Windows mixer level manually, then press F2 to preserve it; Apply will not choose one original level for you. An incomplete restoration blocks further Apply changes until F2 finishes. Recovery checks happen only when these volume actions run, with no polling or automatic restart detection.

Settings live at `%LOCALAPPDATA%\WarDogsEarplugs\settings.txt`. They are validated before activation, written to a uniquely named same-directory temporary file created exclusively, flushed, then atomically replaced using Windows `MoveFileExW` with write-through. Separate writers use separate staging files; failed saves attempt to remove their own staging file. A failed file save keeps the previous active settings. Preferences can be saved even when their audio Apply fails: the status explicitly says **Preferences saved**, and the active filter stays unchanged until successful recovery/application. Corrupt settings report an error, use safe defaults and show the settings window even with `--background`. This protects complete configuration writes under ordinary filesystem operation, but is not a database or a promise of full ACID transactions across Windows audio sessions. Multi-session volume changes are sequential and roll back completed changes if a later change fails. Rollback preserves newer manual mixer edits, and an incomplete rollback retains its recovery target for the next F2 restoration attempt.

To uninstall, Exit, delete the EXE and remove any startup shortcut you created. Optionally delete `%LOCALAPPDATA%\WarDogsEarplugs` to remove saved settings.

## Code signing policy

**Trusted code signing is pending service enrollment/identity validation.** Version 0.1.5 is prepared for explicitly unsigned public distribution at the maintainer's request. The update prompt identifies unsigned downloads. The automated workflow keeps unsigned builds in drafts unless the maintainer chooses to publish them. Do not treat a checksum or a self-signed certificate as a trusted Authenticode signature.

The project includes a GitHub-hosted build and a SignPath signing stage. It publishes a signed release only after the returned EXE passes Windows Authenticode validation. See [SIGNING.md](SIGNING.md) for setup and certificate alternatives. The maintainer is [Daniel Grao / DanGxG](https://github.com/DanGxG). No signing provider is claimed to endorse or sign this project before acceptance.

Signed applications can still receive SmartScreen or antivirus reputation warnings, especially for a new publisher. Signing identifies the publisher and protects file integrity; it does not guarantee zero warnings. See [Microsoft's SmartScreen guidance](https://learn.microsoft.com/en-us/windows/apps/package-and-deploy/smartscreen-reputation).

**Privacy:** update checks and accepted downloads connect to GitHub and its public download CDN over HTTPS. Those services receive ordinary HTTP request metadata; Earplugs sends no game data, keyboard events, typed text or saved configuration and has no telemetry. Preferences remain local. Its source and build workflows are public.

## Build and test

On Windows with Rust and the Visual Studio C++ Build Tools/Windows SDK installed:

```powershell
cargo fmt --all -- --check
cargo clippy --locked --all-targets -- -D warnings
cargo test --locked
cargo build --locked --release
```

Output: `target\release\wardogs-earplugs.exe`. The pinned Rust toolchain and `Cargo.lock` are checked in. `windows` supplies Microsoft's Win32 bindings; `winresource` is a build-only dependency for the manifest and version metadata. The final app uses Windows system DLLs.

On a Windows machine with an active audio output, test the real mixer with a silent fixture (not the game):

```powershell
cargo build --locked --example wardogs-test-tone
cargo test --locked real_windows_mixer_roundtrip -- --ignored --nocapture
```

Opt-in real-device tests for ended-session recovery and duplicate-instance exclusion use only silent fixture processes, with no keyboard injection:

```powershell
cargo test --locked expired_session_restores_persisted_volume -- --ignored --nocapture
cargo test --locked restored_instance_is_not_reused_for_another_session -- --ignored --nocapture
```

Opt-in network validation downloads the public GitHub manifest and its selected EXE, verifies it and deletes the staged file without installing or executing it:

```powershell
cargo test --locked update::tests::real_github_manifest_and_verified_download -- --ignored --nocapture
```

The download check does not exercise installation or rollback. To exercise declining an update, rejecting an incorrect checksum, and replacing the portable EXE while preserving settings and window mode, close Earplugs and run the isolated UI harness:

```powershell
cargo test --locked --no-run
$testHarness = (Get-ChildItem ./target/debug/deps/wardogs_earplugs-*.exe | Sort-Object LastWriteTime -Descending | Select-Object -First 1).FullName
./scripts/test-updater.ps1 -TestHarness $testHarness -NewExecutable ./target/release/wardogs-earplugs.exe
```

The test uses a local candidate, private settings and temporary folders with spaces. It sends commands only to its own UI processes and does not inject keyboard input or inspect the game. Local update overrides exist only in the test harness; the replacement production EXE performs its normal startup update check. [VALIDATION.md](VALIDATION.md) records executed checks and their limits.

To test real F2 delivery and restore-on-exit against a particular compiled EXE, close Earplugs first and set `$env:EARPLUGS_TEST_EXE` to that EXE's absolute path before running the mixer test. The normal test injects keyboard input through Windows and needs a quiet desktop with other games closed so its harmless keyboard fixture keeps focus.

To leave a game or other app open, disable keyboard injection while checking the release's mixer changes, Apply controls, overlay and restore-on-exit:

```powershell
$env:EARPLUGS_TEST_NO_INPUT = '1'
cargo test --locked real_windows_mixer_roundtrip -- --ignored --nocapture
Remove-Item Env:EARPLUGS_TEST_NO_INPUT
```

This mode uses direct control messages and does not verify physical F2 delivery, held-key combinations or input deferral inside a tray menu. Ordinary tests include movement-key, modifier-change, repeated-F2, multiple-keyboard and stale-state sequences. The old Shift+W failure was reproduced in v0.1.1; an initial Raw Input live Shift+W check reached the expected mixer levels, but the full keyboard run failed because WarDogs reclaimed focus from the test fixture. The complete live keyboard matrix is still unverified.

To verify controls, persistence, tray hiding, duplicate launches and idle resource use:

```powershell
./scripts/smoke.ps1 -Executable ./target/release/wardogs-earplugs.exe
./scripts/startup-ready.ps1 -Executable ./target/release/wardogs-earplugs.exe
```

The startup-ready test uses a private native window and named event to check that a second launch waits for initialization; it requires the real Earplugs app to be closed. These tests use isolated settings, private windows or silent fixtures; they do not launch or inspect the game.

GitHub runners do not normally provide an audio output, so the real-device test is opt-in. It targets only `wardogs-test-tone.exe` and is never shipped. GNU builds can be used for development; the release workflow uses MSVC and embeds the app manifest and version resource.
