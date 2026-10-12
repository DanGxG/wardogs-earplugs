# WarDogs Earplugs

A tiny native Windows tray app written in Rust. **F2 sets only WarDogs' Windows mixer volume to the chosen percentage (40% by default), then restores its previous level on the next press.** An independent top-left desktop badge displays **Earplugs ON** while active. Discord, music, other games and Windows master volume stay at their existing levels.

## Use

1. Download the Windows x64 release EXE, or unzip the portable package. Requires Windows 10/11 x64. No Python, installer or administrator access is needed.
   When updating, **Exit the old version first**; launching a new copy while the old one is running reopens the old instance.
2. Run `wardogs-earplugs.exe` and start WarDogs so it creates a Windows audio session.
3. Enter a whole **Game mixer volume (%)** from **0 to 100**. Default is **40**. This is an exact target mixer percentage, not a relative reduction or a ceiling: choosing 60 sets matching sessions to 60%, even if they were quieter.
4. Leave **Game executable contains** at `wardogs`, or enter the actual name shown in Task Manager's **Details** tab. Matching ignores letter case. A complete name ending in `.exe` matches exactly; otherwise it matches that distinctive part of the filename. Click **Apply**. While earplugs are on, Apply updates the current sessions immediately, including changes from 40% to 60% or 80%, while keeping their original levels for restoration.
5. Press **F2** to turn earplugs on. Press **F2** again to restore the game and hide the badge. The Toggle button does the same thing.
6. **Hide** or the window's close button keeps the app in the system tray. Click the tray icon to reopen settings; right-click for the menu. **Exit** restores volume and stops the app.

`wardogs-earplugs.exe --background` starts directly in the tray. For optional startup with Windows, create a shortcut to the EXE in `shell:startup`, with `--background` in its Target. The app never enables startup itself.

If another app owns F2, Earplugs reports the conflict. Close that app and restart Earplugs, or use the Toggle button. F2 is registered globally, so remove any in-game F2 binding you need. Exclusive fullscreen/input modes may prevent Windows hotkeys from being delivered; try borderless mode. There is deliberately no low-level keyboard hook fallback.

The badge is a small, independent Windows window at the top-left of the primary screen. It stays above ordinary desktop and borderless/windowed game windows, lets clicks pass through, and does not take keyboard focus. It has no owner or attachment to the game window. Exclusive fullscreen may cover it. It appears only while earplugs are on and uses no timer or polling loop.

## Small by design

- One thread, native Win32 controls, native tray icon and a blocking Windows message loop.
- No polling, timers, worker threads, embedded browser, Python interpreter, telemetry, updates or network traffic.
- Audio/process enumeration happens when turning earplugs on. Applying a new target and restoring use the saved Windows audio-session interfaces.
- Ordinary unchanged F2 presses perform no settings-file writes.
- One running instance per Windows login session. Release builds optimize for size and link the MSVC runtime statically.

Memory and CPU depend on Windows, audio drivers and tray/UI state. See [VALIDATION.md](VALIDATION.md) for measured results and their limits.

## Windows integration and reliability

Earplugs uses Windows `RegisterHotKey`, a process-list snapshot containing filenames/PIDs, Core Audio mixer sessions and an independent native desktop badge. It does not open the game's process, read/write its memory, inject code, load a game plugin, hook the game window, intercept graphics or install a driver. The process snapshot is used solely to associate the named game's PIDs with its Windows audio sessions. It checks all active output devices, including games routed to a different output.

Anti-cheat classification belongs to the game's vendor; this implementation cannot promise approval or that a future policy will accept a global hotkey utility.

Previous mixer volume is kept in memory. Applying a new target while on keeps that original restoration level. Manual mixer changes made after the most recent Apply are preserved on restoration. A game restart or output-device change may create new sessions: press F2 to clear/restore the old sessions, then F2 again to set the new ones. Normal Exit and Windows shutdown attempt restoration. Forced termination, a crash or power loss cannot guarantee restoration; use Windows Volume Mixer if needed. The app does not change mute state.

Settings live at `%LOCALAPPDATA%\WarDogsEarplugs\settings.txt`. They are validated before activation, written to a same-directory temporary file, flushed, then atomically replaced using Windows `MoveFileExW` with write-through. A failed save keeps the previous active settings. Corrupt settings report an error and load safe defaults. This protects complete configuration writes under ordinary filesystem operation, but is not a database or a promise of full ACID transactions across Windows audio sessions. Multi-session volume changes are sequential and roll back completed changes if a later change fails.

To uninstall, Exit, delete the EXE and remove any startup shortcut you created. Optionally delete `%LOCALAPPDATA%\WarDogsEarplugs` to remove saved settings.

## Code signing policy

**Trusted code signing is pending service enrollment/identity validation.** Version 0.1.0 is publicly available as an explicitly unsigned EXE, and v0.1.1 is prepared for explicitly unsigned public distribution at the maintainer's request. The automated workflow keeps unsigned builds in drafts unless the maintainer chooses to publish them. Do not treat a checksum or a self-signed certificate as a trusted Authenticode signature.

The project includes a GitHub-hosted build and a SignPath signing stage. It publishes a signed release only after the returned EXE passes Windows Authenticode validation. See [SIGNING.md](SIGNING.md) for setup and certificate alternatives. The maintainer is [Daniel Grao / DanGxG](https://github.com/DanGxG). No signing provider is claimed to endorse or sign this project before acceptance.

Signed applications can still receive SmartScreen or antivirus reputation warnings, especially for a new publisher. Signing identifies the publisher and protects file integrity; it does not guarantee zero warnings. See [Microsoft's SmartScreen guidance](https://learn.microsoft.com/en-us/windows/apps/package-and-deploy/smartscreen-reputation).

**Privacy:** the application never transfers user information to a network service. It stores only the chosen percentage and executable-name filter locally. Its source and build workflows are public.

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

To test the real F2 hotkey and restore-on-exit against a particular compiled EXE, close Earplugs first and set `$env:EARPLUGS_TEST_EXE` to that EXE's absolute path before running the mixer test. The test sends actual F2 input through Windows while the app is hidden. To verify controls, persistence, tray hiding, duplicate launches and idle resource use:

```powershell
./scripts/smoke.ps1 -Executable ./target/release/wardogs-earplugs.exe
```

These tests use isolated settings and a silent fixture; they do not launch or inspect the game.

GitHub runners do not normally provide an audio output, so the real-device test is opt-in. It targets only `wardogs-test-tone.exe` and is never shipped. GNU builds can be used for development; the release workflow uses MSVC and embeds the app manifest and version resource.
