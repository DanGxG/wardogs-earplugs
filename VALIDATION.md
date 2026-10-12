# Validation

Tested on Windows x64 on 12 October 2026. Official release builds use Rust 1.90.0, MSVC, a static CRT, native Win32 UI and embedded version/manifest resources. Historical v0.1.0 measurements are labeled separately below.

## v0.1.0 observed resource use

Baseline measured from the GitHub-hosted MSVC build of commit `3624d6a` using `scripts/smoke.ps1`, after exercising its controls and hiding it in the tray:

| Measurement | Result |
| --- | ---: |
| EXE size | 269,312 bytes (263 KiB) |
| Resident working set | 11,509,760 bytes (about 11 MiB) |
| Private committed memory | 2,220,032 bytes (about 2.1 MiB) |
| CPU time during 15 seconds idle | 0 ms recorded |
| OS-observed threads | 4 |

The app creates one UI thread; Windows/COM may create internal threads. Working set includes shared Windows code. CPU counters are finite-resolution measurements, so 0 ms recorded does not establish literally zero CPU usage forever. Measurements vary by machine, driver, UI state and interactions; these are observed values, not maximum guarantees.

## v0.1.0 passed checks

- Formatting, Clippy with warnings denied, five ordinary unit/filesystem tests, release compilation and GitHub-hosted Windows build.
- A real Windows mixer roundtrip using a silent fixture: 80% -> 40% -> 80%; a 0% target; preserving a manual change to 25%; never raising a session already at 20% (the v0.1.0 ceiling behavior, superseded by an exact target in v0.1.1); missing-game reporting.
- Actual global F2 input sent through Windows against the GitHub-built release EXE while hidden: lowering, restoring, lowering again and restore-on-exit.
- Isolated UI smoke test: 40% default, numeric validation, invalid input never saved, settings persistence, hotkey dispatch, close-to-tray, duplicate-instance handling and normal exit.
- Visual inspection of the actual native window: numeric fields, labels, buttons and status fit without clipping.
- Microsoft Defender custom scan of the GitHub-built EXE completed with no matching threat detection reported on this computer. This does not establish a universal antivirus result.

The final tagged build repeats CI validation. The signing stage requires a `Valid` trusted Authenticode signature before publishing through that path; the maintainer can explicitly publish unsigned assets. SHA-256 checksums are generated from the actual release assets.

## v0.1.1 checks

Passed against the local GNU build and the GitHub-hosted MSVC build of production code at commit `48f6520`:

- Formatting, Clippy with warnings denied, five unit/filesystem tests and release compilation.
- Real mixer reproduction: turn on at 40%, Apply 60%, Apply 80%, then restore the original volume; an initially quieter session reaches the chosen exact target; 0% and 100% targets; manual mixer changes and missing-game reporting.
- Actual F2 delivery, repeated toggling and restore-on-exit against the release EXE.
- A deterministic tray-menu regression first failed because a delivered hotkey was dropped while application state was borrowed. The counter-based deferral fix passed the same case: dismissing the menu restores the original volume and hides the badge.
- UI validation and persistence, Apply while active, tray hiding, duplicate-instance handling and normal exit.
- Overlay visibility only while on, primary-screen placement at (12, 12), no focus activation, transparent hit testing, topmost state, matching the Earplugs process ID, no owner window, and destruction on Exit.
- Visual inspection of the actual onscreen badge: mint text on a small translucent black background, no clipping.
- Mouse input reached a separate test process's window beneath the visible badge without changing focus.
- GitHub-hosted Windows formatting/lint/unit/build checks passed. The MSVC EXE reports version 0.1.1 and contains no imported process-memory APIs, low-level keyboard hook, input injection or polling timer.
- Microsoft Defender custom scan of the MSVC EXE completed with no matching detection reported on this computer; this is not a universal antivirus result.
- Local GNU sample: OFF working set 10,055,680 bytes, private memory 2,002,944 bytes; 0 ms CPU recorded over 15 idle seconds. ON with badge visible: working set 11,808,768 bytes, private memory 2,383,872 bytes; 0 ms CPU recorded over 5 idle seconds. These are measurements on this computer, not universal limits.

Representative MSVC v0.1.1 measurements on this computer:

| Measurement | Result |
| --- | ---: |
| EXE size | 274,432 bytes (268 KiB) |
| OFF working set / private memory | 11,636,736 / 2,121,728 bytes |
| ON working set / private memory | 12,161,024 / 2,412,544 bytes |
| Idle CPU OFF, 15 seconds | 0 ms recorded |
| Idle CPU ON, 5 seconds | 0 ms recorded |

Repeated live synthetic-key testing had an intermittent delivery failure (`Real F2 did not set expected mixer volume 0.4; actual=0.8, overlay_on=false`). Other native F2 runs and deterministic dispatched-hotkey tests passed. Synthetic input shares the desktop's keyboard state; these results do not verify every desktop/input context or the actual game's fullscreen behavior.

## v0.1.2 checks

The previous release was reproduced failing on real input with Shift+W held: `F2 with held keys [a0, 57] failed; expected=0.4, actual=0.8`. `RegisterHotKey` matches modifiers exactly. Registering additional modifier variants also failed the held-F2 case when a modifier changed: `Held F2 repeated while adding modifier a0`.

The final implementation uses Windows Raw Input keyboard events in background input-sink mode, with a press/release latch. It does not reserve shortcuts, suppress key delivery, install a keyboard hook, or poll. Non-F2 events are discarded without logging or storage. Windows/game F2 bindings continue to receive the key normally.

Passed locally:

- Formatting, Clippy with warnings denied, six unit/filesystem tests and release compilation. The new input-state regression covers movement and modifier changes during a held F2, repeated key-down events, release, and a fresh press; exactly two fresh presses are accepted.
- Actual Shift+W/F2 mixer transitions passed during initial live raw-input runs. The complete multi-key matrix did **not** pass: WarDogs reclaimed focus from the isolated keyboard fixture (`Focus changed for held keys [57, 41]`). The maintainer requested keeping the game open, so remaining checks disable synthetic keyboard input rather than sending gameplay/shortcut keys to an uncontrolled window.
- `EARPLUGS_TEST_NO_INPUT=1` real Windows mixer and release UI checks passed: exact 40/60/80/0/100% targets, restoration, manual-change preservation, missing-game handling, Apply while active, badge state/placement/independence, and restore on Exit. This mode uses the Toggle button and does not verify global keyboard delivery or tray-menu input deferral.
- Isolated smoke checks passed: numeric validation, persistence, missing-game response, close-to-tray, single-instance handling and Exit. GNU build sample: working set 10,031,104 bytes; private memory 1,990,656 bytes; 15.625 ms CPU recorded over 15 seconds while the game remained running.
- Local release imports include `RegisterRawInputDevices` and `GetRawInputData`; no imported game-process memory APIs, low-level keyboard hook, input injection or polling timer.

The full live modifier matrix and held-key Windows delivery remain unverified until a desktop is available without competing game focus/input. Unit input-state tests are not a substitute for those live checks.

## Not verified / external prerequisites

- Real WarDogs executable name, game-specific audio routing, exclusive fullscreen hotkey delivery/overlay visibility and vendor anti-cheat acceptance: not tested with the actual game. Exclusive fullscreen may cover the independent desktop badge. The default `wardogs` filename filter is editable.
- Trusted Authenticode signing: pending an approved provider/account and identity validation. Releases are explicitly unsigned by maintainer request; no release is claimed to have a trusted Authenticode signature.
- Zero SmartScreen/antivirus warnings on friends' PCs: cannot be guaranteed by signing or by the local scan.
- Hard crashes/power failures cannot guarantee volume restoration. Settings replacement is atomic under normal same-volume filesystem operation; Windows audio sessions are not an ACID database.
