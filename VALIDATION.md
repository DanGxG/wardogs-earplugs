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

The final tagged build repeats CI validation. The optional signing stage requires a `Valid` Authenticode result before publishing through that path. SHA-256 checksums are generated from the actual release assets.

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

## v0.1.3 bug fixes and checks

The original failing cases were reproduced before the corresponding fixes:

- Two keyboards could release each other's F2 latch, making a held key toggle again. Input state now belongs to each keyboard; removal and power/session notifications clear stale state.
- A close delivered inside the native tray menu could destroy the root window and strand the process. Close, Exit, Apply, toggle, Explorer restart and shutdown actions now wait until application state is available. Close/Exit dismiss the menu and discard queued menu reopenings.
- A second launch could show a partially initialized window and exit before startup completed. An initialization event now provides a bounded startup wait. `scripts/startup-ready.ps1` reproduces this with an isolated window and named objects, without keyboard injection or audio access.
- A competing settings writer could modify the published configuration through a shared staging handle. Each save now exclusively creates its own staging file; a failed replacement retains the old settings and removes its staging file.
- Apply rollback could overwrite a newer manual mixer change, or lose a pre-Apply manual level when rollback failed. Rollback now checks ownership and retains a recovery target. Partial restoration blocks further Apply updates until recovery completes.
- Corrupt settings during `--background` startup could hide the warning. The settings window now appears with defaults and the load error; Apply repairs the file.
- Dropping expired sessions lost the original volume while Windows persisted the lowered level for the next game process. Setting an expired interface's volume also failed to repair the level inherited by the next process. Earplugs now retains recovery records and, on F2, finds a unique live replacement on the original endpoint using Windows' opaque session identifiers. Apply can refresh a restarted session while ON. Existing original instances remain excluded throughout recovery; conflicting original targets cannot be applied to one replacement.

Passed locally on Windows, using the GNU development build:

- Formatting, Clippy with warnings denied, 14 ordinary unit/filesystem/fault-injection tests and optimized compilation.
- All three opt-in real-device tests with silent fixtures: mixer roundtrip; expiration/restart and inherited restoration; multiple-instance recovery. The restart test confirms 80% -> 40% -> process exit -> pending recovery -> new process -> 80%, then a further restart inherits 80%. It also checks Apply to a restarted session and preservation of a manual 25% change.
- The multiple-instance test first reproduces the erroneous reuse of an already-restored original. Recovery now waits for a genuinely new session. Two ended instances with different originals also cannot overwrite a manual replacement through Apply; F2 preserves its manual level.
- Actual release controls tested with `EARPLUGS_TEST_NO_INPUT=1`: exact 40/60/80/0/100% targets, restore, manual-edit preservation, missing-game handling, overlay visibility/placement/independence, close during a tray menu, queued menu cancellation and restore on Exit.
- Isolated smoke checks: validation, persistence, tray hiding, duplicate launches, normal Exit and visible/repairable corrupt background settings. The startup-readiness fixture fails against the old release and passes against the fixed build.

WarDogs remained open at the maintainer's request. These checks send no synthetic keyboard input; the full live global F2/modifier matrix remains unverified. Physical device unplug/replug, suspend/resume, session lock/unlock, output-device removal and actual Windows shutdown were implemented but not executed. Input reset sequences and partial recovery were checked through unit tests; those are not substitutes for live OS lifecycle tests.

Recovery is held in memory. Turn OFF before closing the game when possible. If the game already ended or its original output disappeared, keep Earplugs running, reopen the game or reconnect that output, then press F2. Normal Exit refuses to discard pending recovery; a crash, forced termination or shutdown can lose it. No background polling or automatic restart monitor was added.

## v0.1.4 startup updates

The app now checks GitHub's latest public checksum manifest once when its primary instance starts, including background startup. It offers a newer stable version through a native Spanish update dialog with No as the default. Declining performs no executable download. Accepted downloads keep the offered version/name/hash fixed, use Windows HTTPS and streaming SHA-256, reject unsupported executable formats, and apply the manifest's Authenticode requirement.

The updater adds Windows WinHTTP, BCrypt and WinVerifyTrust APIs, with no new Rust libraries. A temporary Rust worker performs each check/download and finishes afterward; normal Exit cancels and joins it so its staging-file cleanup completes. Windows HTTP/crypto/COM may retain internal threads and caches. The helper validates its actual parent PID and initialized Earplugs window before opening that parent, prepares and verifies the replacement before handoff, waits for the old process, replaces with a backup, and confirms the new UI's PID/readiness before deleting the backup. Recovery restarts the original with a failure notice and helper cleanup.

Passed locally on Windows with the GNU development build:

- Formatting and Clippy with warnings denied; 23 ordinary tests including numeric version ordering, invalid/ambiguous manifest rejection, HTTPS/redirect host restrictions, the known SHA-256 vector, normal-Exit worker cleanup, corrupt payload/preparation rejection, a locked destination, missing-target recovery and rollback after failed initialization.
- Native Windows HTTPS against the public GitHub release: checksum manifest retrieval, streaming EXE download, digest and x64 validation, mismatch and Authenticode-policy rejection, and staging cleanup. No GitHub token is used by the app.
- `scripts/test-updater.ps1` tests a simulated newer offer in the test-only UI harness against a real compiled app. No keeps the old EXE with no staging; a bad hash shows the checksum error and preserves the old EXE; Yes replaces and restarts into the real new app, preserving saved 55%/filter settings and window mode, then removes helper/backup files. Test injection is compiled only into the test harness and is absent from the production EXE. Controls are targeted by owned PID, with no keyboard injection or game interaction.
- Release mixer/UI checks with `EARPLUGS_TEST_NO_INPUT=1`, isolated settings/tray/single-instance/corrupt-startup smoke checks, and normal Exit remained green.

Representative GNU sample after the startup network check: EXE 459,264 bytes; resident working set 15,486,976 bytes; private memory 3,551,232 bytes; 0 ms CPU recorded over 15 idle seconds; 10 OS-observed threads. These are samples, not limits; native Windows networking increases the resident Windows code/caches compared with v0.1.3.

Not exercised end to end: a valid signed update, an actual newer public release offer, interruption during replacement, restricted-folder ACL variants, or audio-restoration failure within the updater UI. Signed rejection, replacement fault/rollback behavior and core audio partial-recovery behavior were tested separately. The entire live global F2/modifier matrix remains unverified because the maintainer kept WarDogs open. Crash/forced termination/power loss can interrupt file cleanup or installation and cannot guarantee recovery.

Versions up to v0.1.3 have no startup updater: v0.1.4 must be downloaded manually once. The feature requires write access to the EXE folder and never requests administrator access.

## v0.1.5 relative volume

The numeric setting now keeps a percentage of each session's original volume, captured on activation. An original 80% with 40 / 60 / 80 produces 32% / 48% / 64%. Apply always uses the captured original rather than the current or last-applied level. A fresh OFF/ON cycle captures the user's current mixer level. Saved settings retain their numbers and file format; their interpretation changes from the absolute targets used in v0.1.1–v0.1.4. The historical checks above describe those earlier releases.

The original failures were reproduced before the fix: `Apply 60% must use the original session volume; expected=0.48, actual=0.6` and, against real Windows Core Audio, `40% of the original 80% must be 32%; actual=0.4`. Both original reproductions then passed.

Passed locally on Windows with the GNU development build:

- Formatting, Clippy with warnings denied, 24 ordinary tests and optimized compilation. Per-session originals 80% and 20% remain fixed during repeated Apply 60 / 80 / 40, and restore separately. Existing rollback/manual-edit and partial-restoration tests use the relative targets.
- Real Windows mixer roundtrip, restart/inherited-volume recovery and multiple-instance recovery with silent fixtures. The roundtrip covers 80% -> 32 / 48 / 64 / 0 / 80 -> 80%; an original 20% becomes 8%, then restores to 20%. Manual 25% mixer changes remain preserved.
- Five opt-in audio tests passed. Two additional regressions first failed: unchanged ended sessions (100% retained, or an original 0%) wrongly required a live replacement; and two ended originals at 40%/80%, applied as 16%/32%, could misclassify the replacement's owned 32% as a manual edit for the other record (`Restoration 1 discarded a conflicting original as a manual edit belonging to another record`). Unchanged records now complete without a recovery write. Conflicting replacements remain pending while matching any owned level, then preserve a genuinely different manual 25% level without requiring another restart.
- Actual release controls, Apply, overlay ownership/placement/visibility, nested tray Close, Exit restoration, isolated settings/tray/single-instance/corrupt-background-startup checks and readiness checks. Synthetic keyboard input stays disabled so WarDogs can remain open.
- The three simulated-offer updater UI cases still pass against the compiled app: No, bad hash and accepted replacement with saved preferences preserved.

Representative GNU sample: EXE 462,336 bytes; working set 15,536,128 bytes; private memory 3,600,384 bytes; 0 ms CPU recorded over 15 idle seconds; 10 OS-observed threads. These are observations on this computer, not maximum guarantees.

## v0.1.6 release presentation and update format

Release resources define Daniel Grao as the individual author in CompanyName, with product/file version information, internal/original filename, copyright, MIT license, website and public source references. The app window shows only its version and MIT license. Dedicated icons are embedded for Explorer/window and the small titlebar/tray view. The ICO sources and a build-only regeneration script are included; no image generator runs with the app.

Downloads use the canonical `wardogs-earplugs-v0.1.6-windows-x64.exe` / `.zip` names. The checksum manifest starts with `# earplugs-release-v2`; its optional `# authenticode-required` policy retains Windows trust verification when required. Unknown, missing, repeated or displaced policies, duplicate/ambiguous EXEs and invalid hashes/paths remain rejected. Versions up to v0.1.5 need a manual v0.1.6 installation once, because their update parser uses the earlier filename-based policy.

Passed locally with the GNU development build: formatting, Clippy with warnings denied, 24 ordinary tests, optimized compilation, mixer/Apply/overlay/Exit checks with keyboard injection disabled, three simulated-offer updater UI cases, settings/tray/single-instance/corrupt-background-startup checks and readiness checks. The new-format manifest regression first failed with `Invalid SHA-256 manifest.`, then passed after the parser change. Canonical packaging was executed and the generated EXE/ZIP checksums matched its manifest. MSVC release resources are provided by the GitHub-hosted build; the GNU development build does not embed those resources.

## Not verified / external prerequisites

- Real WarDogs executable name, game-specific audio routing, exclusive fullscreen hotkey delivery/overlay visibility and vendor anti-cheat acceptance: not tested with the actual game. Exclusive fullscreen may cover the independent desktop badge. The default `wardogs` filename filter is editable.
- Zero SmartScreen/antivirus warnings on friends' PCs: cannot be guaranteed by signing or by the local scan.
- Hard crashes/power failures cannot guarantee volume restoration. Settings replacement is atomic under normal same-volume filesystem operation; Windows audio sessions are not an ACID database.
