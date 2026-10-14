# Daemon lifecycle

`Daemon::new(root)` opens `Services` for the drive (refusing a missing root and
symlinked `.aidex`, `.aidex/runtime`, or event-database paths) and registers
every method. `Daemon::start` draws a 256-bit capability from the OS random
source, binds `127.0.0.1:0`, and publishes `.aidex/runtime/daemon.json` with
`create_new` (endpoint `version` 2). That exclusive create is the single-instance
lock: a second daemon fails with `EndpointError::Exists`. On Unix the file is
created 0600; on Windows the capability is protected with the current user's
DPAPI credential (via PowerShell), so another Windows account cannot use it.

`aidexd start` / `aidex daemon start` call `lifecycle::launch`, which first runs
stale-endpoint recovery and then spawns `aidexd run --drive <root>` detached and
waits (up to 30 s) for an authenticated `daemon.status`. Unix spawns it in a new
process group. On Windows the daemon is started through PowerShell
`Start-Process` because `std::process::Command` always enables handle
inheritance, which would leak a caller's stdout pipe into the daemon and keep
`aidex daemon start --json | ...` from ever finishing; clearing the inherit flag
would need FFI, which the workspace forbids.

Stale-endpoint recovery (`recover_stale_endpoint`) never removes an endpoint it
cannot prove dead: an endpoint that completes an authenticated handshake is
`AlreadyRunning`; one whose port still accepts TCP (including an incompatible
contract-v1 daemon) or that cannot be parsed is refused with `EndpointBusy`;
only a parseable endpoint whose loopback port refuses connections is removed.
Other drive data is never touched.

`aidexd stop` / `aidex daemon stop` call `daemon.stop` and wait (up to 15 s)
for the endpoint file to disappear. Shutdown stops accepting, sends
`aidex.closing` to every connection, drains in-flight responses, ends
subscriptions, and removes the endpoint. If the drive disappears (checked on
every listener poll, 50 ms), the daemon performs the same shutdown and exits;
an endpoint it could not delete is recovered as stale when the drive returns.

Known limitations: Windows startup costs two PowerShell invocations (launch and
DPAPI), and a ShellExecute-based launch is subject to Windows zone checks on
binaries carrying a mark-of-the-web. Drive-removal detection is polling, not an
OS notification. A daemon killed without cleanup leaves its endpoint for
recovery by the next start.
