# Validation

Tested on Windows x64 on 12 October 2026. The release build uses Rust 1.90.0, MSVC, a static CRT, native Win32 UI and embedded version/manifest resources.

## Observed resource use

Baseline measured from the GitHub-hosted MSVC build of commit `3624d6a` using `scripts/smoke.ps1`, after exercising its controls and hiding it in the tray:

| Measurement | Result |
| --- | ---: |
| EXE size | 269,312 bytes (263 KiB) |
| Resident working set | 11,509,760 bytes (about 11 MiB) |
| Private committed memory | 2,220,032 bytes (about 2.1 MiB) |
| CPU time during 15 seconds idle | 0 ms recorded |
| OS-observed threads | 4 |

The app creates one UI thread; Windows/COM may create internal threads. Working set includes shared Windows code. CPU counters are finite-resolution measurements, so 0 ms recorded does not establish literally zero CPU usage forever. Measurements vary by machine, driver, UI state and interactions; these are observed values, not maximum guarantees.

## Passed checks

- Formatting, Clippy with warnings denied, five ordinary unit/filesystem tests, release compilation and GitHub-hosted Windows build.
- A real Windows mixer roundtrip using a silent fixture: 80% -> 40% -> 80%; a 0% target; preserving a manual change to 25%; never raising a session already at 20%; missing-game reporting.
- Actual global F2 input sent through Windows against the GitHub-built release EXE while hidden: lowering, restoring, lowering again and restore-on-exit.
- Isolated UI smoke test: 40% default, numeric validation, invalid input never saved, settings persistence, hotkey dispatch, close-to-tray, duplicate-instance handling and normal exit.
- Visual inspection of the actual native window: numeric fields, labels, buttons and status fit without clipping.
- Microsoft Defender custom scan of the GitHub-built EXE completed with no matching threat detection reported on this computer. This does not establish a universal antivirus result.

The final tagged build repeats CI validation; the release signing stage refuses public publication without a `Valid` trusted Authenticode signature. SHA-256 checksums are generated from the actual release assets.

## Not verified / external prerequisites

- Real WarDogs executable name, game-specific audio routing, exclusive fullscreen hotkey delivery and vendor anti-cheat acceptance: not tested with the actual game. The default `wardogs` filename filter is editable.
- Trusted Authenticode signing: pending an approved provider/account and identity validation. Current binaries are unsigned and the release stays a draft.
- Zero SmartScreen/antivirus warnings on friends' PCs: cannot be guaranteed by signing or by the local scan.
- Hard crashes/power failures cannot guarantee volume restoration. Settings replacement is atomic under normal same-volume filesystem operation; Windows audio sessions are not an ACID database.
