//! Test-only copy of `aidex-sandbox-exec` (feature `test-helper`): the broker's
//! integration tests launch sandboxed processes through it, because Cargo does not
//! build another package's binaries for `cargo test -p aidex-broker`. Never shipped.

fn main() {
    std::process::exit(aidex_sandbox::helper::run(std::env::args_os()));
}
