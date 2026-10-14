# Desktop lifecycle

Plain Rust (no Tauri types), unit-tested in place.

- `drive.rs`: the session drive. Resolution order: a folder chosen in this session with `select_drive` (memory only; nothing is written to host-side config), `AIDEX_DRIVE_ROOT`, then the drive holding the running executable (nearest ancestor with a regular `aidex-drive.json`, at most 16 levels up), else none and the UI asks for a drive. A selection must hold a supported manifest (`aidex_drive::status_with` without the removable check, so developer folders work; schema v1 is accepted so migration can be offered). `display_path` drops the Windows `\?\` prefix for display.
- `daemon.rs`: `DaemonSupervisor`. `start` finds `aidexd` next to the app executable (else on `PATH`) and calls `aidexd::lifecycle::launch`, which recovers a provably stale endpoint, spawns `aidexd run --drive <root>` detached, and waits up to 30 s for an authenticated `daemon.status`. A daemon that is already running is reported as such and never adopted. `stop` calls `aidexd::lifecycle::stop`. On exit the app stops the daemon only if it launched it (`stop_owned`, 5 s).
