//! Prepare tracking-issue commands even when the optional label is unavailable.
use std::io::Read;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

fn ensure_label() -> bool {
    // A regular file avoids waiting for EOF from descendants that inherit stderr.
    let path = std::env::temp_dir().join(format!("simard-label-{}.log", uuid::Uuid::now_v7()));
    let diagnostic = match std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&path)
    {
        Ok(file) => file,
        Err(error) => {
            tracing::warn!(%error, "could not capture label creation diagnostics; filing tracking issue without label");
            return false;
        }
    };
    struct Cleanup {
        path: std::path::PathBuf,
        file: Option<std::fs::File>,
    }
    impl Drop for Cleanup {
        fn drop(&mut self) {
            drop(self.file.take());
            if let Err(error) = std::fs::remove_file(&self.path) {
                tracing::warn!(%error, "could not remove label creation diagnostics");
            }
        }
    }
    let cleanup = Cleanup {
        path,
        file: Some(diagnostic),
    };
    let stderr = match cleanup.file.as_ref().and_then(|file| file.try_clone().ok()) {
        Some(file) => file,
        None => {
            tracing::warn!(
                "could not capture label creation diagnostics; filing tracking issue without label"
            );
            return false;
        }
    };
    let mut child = match Command::new("gh")
        .args(["label", "create", "ooda-stuck"])
        .stdout(Stdio::null())
        .stderr(Stdio::from(stderr))
        .spawn()
    {
        Ok(child) => child,
        Err(error) => {
            tracing::warn!(%error, "could not start label creation; filing tracking issue without label");
            return false;
        }
    };
    let deadline = Instant::now() + Duration::from_secs(5);
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) if Instant::now() < deadline => std::thread::sleep(Duration::from_millis(50)),
            Ok(None) => {
                let _ = child.kill();
                let _ = child.wait();
                tracing::warn!("label creation timed out; filing tracking issue without label");
                return false;
            }
            Err(error) => {
                let _ = child.kill();
                let _ = child.wait();
                tracing::warn!(%error, "label creation could not be polled; filing tracking issue without label");
                return false;
            }
        }
    };
    if status.success() {
        tracing::info!("created ooda-stuck label for tracking issues");
        return true;
    }
    let mut bytes = Vec::new();
    if let Err(error) =
        std::fs::File::open(&cleanup.path).and_then(|file| file.take(8192).read_to_end(&mut bytes))
    {
        tracing::warn!(%error, "could not read label creation diagnostics; filing tracking issue without label");
        return false;
    }
    let stderr = String::from_utf8_lossy(&bytes);
    if stderr.to_ascii_lowercase().contains("already exists") {
        true
    } else {
        tracing::warn!(%stderr, "label creation failed; filing tracking issue without label");
        false
    }
}

pub(crate) fn tracking_issue_command(title: &str, body: &str) -> Command {
    let include_label = ensure_label();
    let mut command = Command::new("gh");
    command.args(["issue", "create", "--title", title, "--body", body]);
    if include_label {
        command.args(["--label", "ooda-stuck"]);
    }
    command
}

#[cfg(all(test, unix))]
mod tests {
    use std::os::unix::fs::PermissionsExt;

    #[test]
    fn existing_label_does_not_wait_for_inherited_stderr() {
        if std::env::var_os("SIMARD_LABEL_EXISTS_CHILD").is_some() {
            let started = std::time::Instant::now();
            let output = super::tracking_issue_command("title", "body")
                .output()
                .unwrap();
            assert!(output.status.success());
            assert!(started.elapsed() < std::time::Duration::from_secs(2));
            return;
        }
        let dir = tempfile::tempdir().unwrap();
        let gh = dir.path().join("gh");
        std::fs::write(
            &gh,
            r#"#!/bin/sh
if [ "$1" = label ]; then
  /bin/sleep 3 >/dev/null &
  echo 'label already exists' >&2
  exit 1
fi
for arg in "$@"; do [ "$arg" = --label ] && exit 0; done
exit 2
"#,
        )
        .unwrap();
        std::fs::set_permissions(&gh, std::fs::Permissions::from_mode(0o755)).unwrap();
        let output = std::process::Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "ooda_actions::gh_label::tests::existing_label_does_not_wait_for_inherited_stderr",
                "--nocapture",
            ])
            .env("PATH", dir.path())
            .env("SIMARD_LABEL_EXISTS_CHILD", "1")
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{} {}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
    }

    #[test]
    fn unavailable_label_files_without_label() {
        if std::env::var_os("SIMARD_LABEL_FALLBACK_CHILD").is_some() {
            let output = super::tracking_issue_command("title", "body")
                .output()
                .unwrap();
            assert!(output.status.success());
            return;
        }
        let dir = tempfile::tempdir().unwrap();
        let gh = dir.path().join("gh");
        std::fs::write(
            &gh,
            r#"#!/bin/sh
if [ "$1" = label ]; then echo denied >&2; exit 1; fi
for arg in "$@"; do [ "$arg" = --label ] && exit 2; done
exit 0
"#,
        )
        .unwrap();
        std::fs::set_permissions(&gh, std::fs::Permissions::from_mode(0o755)).unwrap();
        let output = std::process::Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "ooda_actions::gh_label::tests::unavailable_label_files_without_label",
                "--nocapture",
            ])
            .env("PATH", dir.path())
            .env("SIMARD_LABEL_FALLBACK_CHILD", "1")
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "child failed: {} {}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
    }
}
