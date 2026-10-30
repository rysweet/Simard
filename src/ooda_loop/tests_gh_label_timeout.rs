//! Run the production filer in a subprocess so PATH and tracing stay isolated.

use std::fs::{self, OpenOptions};
use std::io::{Read, Seek, SeekFrom};
use std::os::unix::fs::PermissionsExt;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use super::no_progress::{GhIssueFiler, NoProgressIssueFiler};

fn cleanup_stalled_label(dir: &std::path::Path) {
    if let Ok(raw) = fs::read_to_string(dir.join("label.pid"))
        && let Ok(pid) = raw.trim().parse::<i32>()
    {
        let _ = nix::sys::signal::kill(
            nix::unistd::Pid::from_raw(pid),
            nix::sys::signal::Signal::SIGKILL,
        );
    }
}

#[test]
#[serial_test::serial(simard_process_reaper)]
fn stalled_label_times_out_and_files_unlabeled_issue() {
    const FIXTURE_ENV: &str = "SIMARD_TEST_STALLED_LABEL_DIR";
    if let Some(dir) = std::env::var_os(FIXTURE_ENV) {
        let dir = std::path::PathBuf::from(dir);
        let events = OpenOptions::new()
            .create(true)
            .append(true)
            .open(dir.join("events"))
            .unwrap();
        let subscriber = tracing_subscriber::fmt()
            .with_ansi(false)
            .without_time()
            .with_writer(move || events.try_clone().unwrap())
            .finish();
        let _subscriber = tracing::subscriber::set_default(subscriber);
        let started = Instant::now();
        let filed = GhIssueFiler
            .file_issue("stalled goal", "needs operator")
            .unwrap();
        assert_eq!(filed.number, "4478");
        assert!(started.elapsed() < Duration::from_secs(10));

        let pid: i32 = fs::read_to_string(dir.join("label.pid"))
            .unwrap()
            .trim()
            .parse()
            .unwrap();
        let mut status = 0;
        // SAFETY: pid was recorded by this fixture and status is writable.
        // ECHILD proves the direct child was reaped, rather than left a zombie.
        assert_eq!(
            unsafe { libc::waitpid(pid, &mut status, libc::WNOHANG) },
            -1
        );
        assert_eq!(
            std::io::Error::last_os_error().raw_os_error(),
            Some(libc::ECHILD)
        );
        return;
    }

    let fixture = tempfile::tempdir().unwrap();
    let gh = fixture.path().join("gh");
    fs::write(
        &gh,
        r#"#!/bin/sh
printf '%s\n' "$*" >> events
case "$1 $2" in
    'label create')
        printf '%s\n' "$$" > label.pid
        exec /bin/sleep 60
        ;;
    'issue create')
        read -r label_pid < label.pid
        if kill -0 "$label_pid" 2>/dev/null; then
            printf '%s\n' 'label process was not reaped before issue creation' >&2
            exit 2
        fi
        printf '%s\n' 'https://github.com/fixture/repo/issues/4478'
        ;;
    *) exit 3 ;;
esac
"#,
    )
    .unwrap();
    fs::set_permissions(&gh, fs::Permissions::from_mode(0o755)).unwrap();
    let output = tempfile::tempfile().unwrap();
    let mut child = Command::new(std::env::current_exe().unwrap())
        .args([
            "--exact",
            "ooda_loop::tests_gh_label_timeout::stalled_label_times_out_and_files_unlabeled_issue",
            "--nocapture",
        ])
        .env(FIXTURE_ENV, fixture.path())
        .env("PATH", fixture.path())
        .current_dir(fixture.path())
        .stdin(Stdio::null())
        .stdout(output.try_clone().unwrap())
        .stderr(output.try_clone().unwrap())
        .spawn()
        .unwrap();
    let started = Instant::now();
    let status = loop {
        if let Some(status) = child.try_wait().unwrap() {
            break status;
        }
        if started.elapsed() >= Duration::from_secs(15) {
            cleanup_stalled_label(fixture.path());
            child.kill().unwrap();
            child.wait().unwrap();
            panic!("stalled label prevented escalation within the fixture deadline");
        }
        std::thread::sleep(Duration::from_millis(20));
    };
    if !status.success() {
        cleanup_stalled_label(fixture.path());
    }
    let mut output = output;
    output.seek(SeekFrom::Start(0)).unwrap();
    let mut diagnostics = String::new();
    output.read_to_string(&mut diagnostics).unwrap();
    assert!(status.success(), "fixture failed: {diagnostics}");
    assert!(
        diagnostics.contains("1 passed"),
        "child test did not run: {diagnostics}"
    );

    let events = fs::read_to_string(fixture.path().join("events")).unwrap();
    let label = events.find("label create ooda-stuck").unwrap();
    let warning = events
        .find("filing issue WITHOUT label (degraded)")
        .unwrap();
    let issue = events
        .find("issue create --title stalled goal --body needs operator")
        .unwrap();
    assert!(
        label < warning && warning < issue,
        "wrong event order: {events}"
    );
    assert!(events.contains("WARN"), "missing warning: {events}");
    assert!(
        events.contains("gh label create timed out after 5000 ms"),
        "missing cause: {events}"
    );
    assert!(
        !events.contains("--label"),
        "issue must be unlabeled: {events}"
    );
}
