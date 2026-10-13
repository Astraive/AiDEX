mod common;
use aidex_sandbox::{CommandSpec, SandboxLevel};
use std::{
    thread,
    time::{Duration, Instant},
};
#[test]
fn interactive_input_stays_open_and_tree_is_tracked() {
    let work = common::workspace();
    let sandbox = common::sandbox();
    let plan = sandbox
        .negotiate(&common::request(SandboxLevel::Brokered, &work))
        .unwrap();
    let command = if cfg!(windows) {
        CommandSpec::new(plan.resolve_program("powershell.exe").unwrap()).args(["-NoProfile", "-NonInteractive", "-Command", "while ($null -ne ($line = [Console]::ReadLine())) { [Console]::WriteLine('reply:' + $line) }"])
    } else {
        CommandSpec::new("/bin/cat")
    };
    let process = sandbox.spawn_interactive(&plan, command).unwrap();
    assert!(process.is_running());
    process.write_stdin(b"one\n").unwrap();
    let deadline = Instant::now() + Duration::from_secs(10);
    while !process.output_so_far().stdout.text().contains("one") {
        assert!(Instant::now() < deadline);
        thread::sleep(Duration::from_millis(20));
    }
    process.write_stdin(b"two\n").unwrap();
    process.close_stdin();
    assert!(common::finish(&process).success());
    assert!(process.output_so_far().stdout.text().contains("two"));
    assert!(process.write_stdin(b"late").is_err());
    assert!(sandbox.registry().live_ids().is_empty());
}
#[test]
fn interactive_input_is_writable_as_soon_as_spawn_returns() {
    // The plan frame and the caller's input share one pipe. A launch must hand the pipe
    // over before it can return, or a protocol client's first request fails as if the
    // runtime had closed its input. Concurrent launches load the host so that a lagging
    // pipe hand-over would surface here.
    let work = common::workspace();
    let sandbox = common::sandbox();
    let plan = sandbox
        .negotiate(&common::request(SandboxLevel::Brokered, &work))
        .unwrap();
    thread::scope(|scope| {
        for _ in 0..8 {
            scope.spawn(|| {
                for _ in 0..4 {
                    let command = if cfg!(windows) {
                        CommandSpec::new(plan.resolve_program("sort.exe").unwrap())
                    } else {
                        CommandSpec::new("/bin/cat")
                    };
                    let process = sandbox.spawn_interactive(&plan, command).unwrap();
                    process.write_stdin(b"first\n").unwrap();
                    process.close_stdin();
                    assert!(common::finish(&process).success());
                    assert!(process.output_so_far().stdout.text().contains("first"));
                }
            });
        }
    });
    assert!(sandbox.registry().live_ids().is_empty());
}
