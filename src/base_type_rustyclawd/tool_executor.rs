use rustyclawd_core::client::ClientError;

/// Execute a tool call locally using process spawning.
pub(super) async fn execute_tool_locally(
    tool_name: &str,
    tool_input: &serde_json::Value,
) -> Result<serde_json::Value, ClientError> {
    match tool_name {
        "Bash" => {
            let command = tool_input
                .get("command")
                .and_then(|v| v.as_str())
                .unwrap_or("");
            let timeout_ms = tool_input
                .get("timeout")
                .and_then(|v| v.as_u64())
                .unwrap_or(120_000);

            let mut cmd = tokio::process::Command::new("sh");
            cmd.args(["-c", command]);
            // Pipe stdout/stderr so tool output doesn't leak to the terminal.
            cmd.stdout(std::process::Stdio::piped());
            cmd.stderr(std::process::Stdio::piped());
            let config = rustyclawd_tools::ProcessSpawnConfig::default();
            let child = rustyclawd_tools::spawn_with_isolation(cmd, &config)
                .await
                .map_err(|e| ClientError::Unknown(format!("spawn failed: {e}")))?;

            let output = tokio::time::timeout(
                std::time::Duration::from_millis(timeout_ms),
                child.wait_with_output(),
            )
            .await
            .map_err(|_| ClientError::Timeout("tool execution timed out".to_string()))?
            .map_err(|e| ClientError::Unknown(format!("process error: {e}")))?;

            Ok(serde_json::json!({
                "stdout": String::from_utf8_lossy(&output.stdout),
                "stderr": String::from_utf8_lossy(&output.stderr),
                "exit_code": output.status.code().unwrap_or(-1),
            }))
        }
        "Read" => {
            let path = tool_input
                .get("file_path")
                .and_then(|v| v.as_str())
                .unwrap_or("");
            match tokio::fs::read_to_string(path).await {
                Ok(contents) => Ok(serde_json::json!({ "content": contents })),
                Err(e) => Ok(serde_json::json!({ "error": format!("{e}") })),
            }
        }
        "Write" => {
            let path = tool_input
                .get("file_path")
                .and_then(|v| v.as_str())
                .unwrap_or("");
            let content = tool_input
                .get("content")
                .and_then(|v| v.as_str())
                .unwrap_or("");
            match tokio::fs::write(path, content).await {
                Ok(()) => Ok(serde_json::json!({ "status": "ok" })),
                Err(e) => Ok(serde_json::json!({ "error": format!("{e}") })),
            }
        }
        "Edit" => {
            let path = tool_input
                .get("file_path")
                .and_then(|v| v.as_str())
                .unwrap_or("");
            let old = tool_input
                .get("old_string")
                .and_then(|v| v.as_str())
                .unwrap_or("");
            let new = tool_input
                .get("new_string")
                .and_then(|v| v.as_str())
                .unwrap_or("");
            match tokio::fs::read_to_string(path).await {
                Ok(contents) => {
                    let replaced = contents.replacen(old, new, 1);
                    match tokio::fs::write(path, &replaced).await {
                        Ok(()) => Ok(serde_json::json!({ "status": "ok" })),
                        Err(e) => Ok(serde_json::json!({ "error": format!("{e}") })),
                    }
                }
                Err(e) => Ok(serde_json::json!({ "error": format!("{e}") })),
            }
        }
        _ => Ok(serde_json::json!({ "error": format!("unknown tool: {tool_name}") })),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn execute_tool_locally_unknown_tool_returns_error_json() {
        let input = serde_json::json!({});
        let result = execute_tool_locally("UnknownTool", &input)
            .await
            .expect("tool execution should succeed");
        let error = result
            .get("error")
            .and_then(|v| v.as_str())
            .expect("expected field present");
        assert!(error.contains("unknown tool"));
        assert!(error.contains("UnknownTool"));
    }

    #[tokio::test]
    async fn execute_tool_locally_read_nonexistent_file_returns_error() {
        let input = serde_json::json!({ "file_path": "/nonexistent/path/to/file.txt" });
        let result = execute_tool_locally("Read", &input)
            .await
            .expect("tool execution should succeed");
        assert!(
            result.get("error").is_some(),
            "should return error for missing file"
        );
    }

    #[tokio::test]
    async fn execute_tool_locally_write_to_invalid_path_returns_error() {
        let input =
            serde_json::json!({ "file_path": "/nonexistent/dir/file.txt", "content": "hello" });
        let result = execute_tool_locally("Write", &input)
            .await
            .expect("tool execution should succeed");
        assert!(
            result.get("error").is_some(),
            "should return error for invalid path"
        );
    }

    #[tokio::test]
    async fn execute_tool_locally_edit_nonexistent_file_returns_error() {
        let input = serde_json::json!({
            "file_path": "/nonexistent/dir/file.txt",
            "old_string": "old",
            "new_string": "new"
        });
        let result = execute_tool_locally("Edit", &input)
            .await
            .expect("tool execution should succeed");
        assert!(
            result.get("error").is_some(),
            "should return error for missing file"
        );
    }

    #[tokio::test]
    async fn execute_tool_locally_read_with_empty_path_returns_error() {
        let input = serde_json::json!({});
        let result = execute_tool_locally("Read", &input)
            .await
            .expect("tool execution should succeed");
        assert!(
            result.get("error").is_some(),
            "empty path should yield error"
        );
    }

    #[tokio::test]
    async fn execute_tool_locally_bash_missing_command_runs_empty_string() {
        let input = serde_json::json!({});
        let result = execute_tool_locally("Bash", &input)
            .await
            .expect("tool execution should succeed");
        // Running empty command succeeds (sh -c "")
        assert!(result.get("exit_code").is_some());
    }

    #[tokio::test]
    async fn execute_tool_locally_bash_echo_captures_stdout() {
        let input = serde_json::json!({ "command": "echo hello_test_42" });
        let result = execute_tool_locally("Bash", &input)
            .await
            .expect("tool execution should succeed");
        let stdout = result.get("stdout").and_then(|v| v.as_str()).unwrap_or("");
        assert!(stdout.contains("hello_test_42"));
        let exit_code = result
            .get("exit_code")
            .and_then(|v| v.as_i64())
            .expect("expected numeric field");
        assert_eq!(exit_code, 0);
    }

    #[tokio::test]
    async fn execute_tool_locally_bash_failing_command_has_nonzero_exit() {
        let input = serde_json::json!({ "command": "false" });
        let result = execute_tool_locally("Bash", &input)
            .await
            .expect("tool execution should succeed");
        let exit_code = result
            .get("exit_code")
            .and_then(|v| v.as_i64())
            .expect("expected numeric field");
        assert_ne!(exit_code, 0);
    }

    #[tokio::test]
    async fn execute_tool_locally_write_and_read_roundtrip() {
        let dir = std::env::temp_dir().join(format!("simard-test-rw-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("create test dir");
        let file_path = dir.join("test_roundtrip.txt");
        let path_str = file_path.to_str().expect("path to str");

        let write_input =
            serde_json::json!({ "file_path": path_str, "content": "roundtrip_content" });
        let write_result = execute_tool_locally("Write", &write_input)
            .await
            .expect("tool execution should succeed");
        assert_eq!(
            write_result.get("status").and_then(|v| v.as_str()),
            Some("ok")
        );

        let read_input = serde_json::json!({ "file_path": path_str });
        let read_result = execute_tool_locally("Read", &read_input)
            .await
            .expect("tool execution should succeed");
        let content = read_result
            .get("content")
            .and_then(|v| v.as_str())
            .expect("expected field present");
        assert_eq!(content, "roundtrip_content");

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn execute_tool_locally_edit_replaces_content() {
        let dir = std::env::temp_dir().join(format!("simard-test-edit-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("create test dir");
        let file_path = dir.join("test_edit.txt");
        let path_str = file_path.to_str().expect("path to str");

        std::fs::write(&file_path, "hello world").expect("write test file");

        let edit_input = serde_json::json!({
            "file_path": path_str,
            "old_string": "hello",
            "new_string": "goodbye"
        });
        let edit_result = execute_tool_locally("Edit", &edit_input)
            .await
            .expect("tool execution should succeed");
        assert_eq!(
            edit_result.get("status").and_then(|v| v.as_str()),
            Some("ok")
        );

        let content = std::fs::read_to_string(&file_path).expect("read test file");
        assert_eq!(content, "goodbye world");

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn execute_tool_locally_bash_with_timeout_param() {
        let input = serde_json::json!({ "command": "echo timeout_test", "timeout": 5000 });
        let result = execute_tool_locally("Bash", &input)
            .await
            .expect("tool execution should succeed");
        let stdout = result.get("stdout").and_then(|v| v.as_str()).unwrap_or("");
        assert!(stdout.contains("timeout_test"));
    }

    #[tokio::test]
    async fn execute_tool_locally_bash_stderr_capture() {
        let input = serde_json::json!({ "command": "echo stderr_test >&2" });
        let result = execute_tool_locally("Bash", &input)
            .await
            .expect("tool execution should succeed");
        let stderr = result.get("stderr").and_then(|v| v.as_str()).unwrap_or("");
        assert!(stderr.contains("stderr_test"));
    }

    #[tokio::test]
    async fn execute_tool_locally_write_empty_content() {
        let dir =
            std::env::temp_dir().join(format!("simard-test-empty-write-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("create test dir");
        let file_path = dir.join("empty.txt");
        let input = serde_json::json!({ "file_path": file_path.to_str().expect("path to str"), "content": "" });
        let result = execute_tool_locally("Write", &input)
            .await
            .expect("tool execution should succeed");
        assert_eq!(result.get("status").and_then(|v| v.as_str()), Some("ok"));
        let content = std::fs::read_to_string(&file_path).expect("read test file");
        assert!(content.is_empty());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn execute_tool_locally_edit_no_match_still_writes() {
        let dir =
            std::env::temp_dir().join(format!("simard-test-edit-nomatch-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("create test dir");
        let file_path = dir.join("edit_nomatch.txt");
        std::fs::write(&file_path, "original content").expect("write test file");
        let input = serde_json::json!({
            "file_path": file_path.to_str().expect("path to str"),
            "old_string": "nonexistent",
            "new_string": "replacement"
        });
        let result = execute_tool_locally("Edit", &input)
            .await
            .expect("tool execution should succeed");
        assert_eq!(result.get("status").and_then(|v| v.as_str()), Some("ok"));
        // Content should be unchanged since old_string wasn't found
        let content = std::fs::read_to_string(&file_path).expect("read test file");
        assert_eq!(content, "original content");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn execute_tool_locally_read_existing_file() {
        let dir = std::env::temp_dir().join(format!("simard-test-read-ok-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("create test dir");
        let file_path = dir.join("readable.txt");
        std::fs::write(&file_path, "test content here").expect("write test file");
        let input = serde_json::json!({ "file_path": file_path.to_str().expect("path to str") });
        let result = execute_tool_locally("Read", &input)
            .await
            .expect("tool execution should succeed");
        let content = result
            .get("content")
            .and_then(|v| v.as_str())
            .expect("expected field present");
        assert_eq!(content, "test content here");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn execute_tool_locally_write_with_missing_content_writes_empty() {
        let dir =
            std::env::temp_dir().join(format!("simard-test-write-nocon-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("create test dir");
        let file_path = dir.join("no_content.txt");
        let input = serde_json::json!({ "file_path": file_path.to_str().expect("path to str") });
        let result = execute_tool_locally("Write", &input)
            .await
            .expect("tool execution should succeed");
        assert_eq!(result.get("status").and_then(|v| v.as_str()), Some("ok"));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
