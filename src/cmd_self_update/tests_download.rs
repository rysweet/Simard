//! Tests for find_binary_in_dir (download module).

use super::download::find_binary_in_dir;
use std::fs;

#[test]
fn test_find_binary_in_flat_dir() {
    let tmp = std::env::temp_dir().join(format!("simard-test-flat-{}", std::process::id()));
    fs::create_dir_all(&tmp).unwrap();
    let bin_path = tmp.join("simard");
    fs::write(&bin_path, b"fake-binary").unwrap();

    let found = find_binary_in_dir(&tmp).unwrap();
    assert_eq!(found, bin_path);

    fs::remove_dir_all(&tmp).unwrap();
}

#[test]
fn test_find_binary_in_nested_dir() {
    let tmp = std::env::temp_dir().join(format!("simard-test-nested-{}", std::process::id()));
    let nested = tmp.join("subdir");
    fs::create_dir_all(&nested).unwrap();
    let bin_path = nested.join("simard");
    fs::write(&bin_path, b"fake-binary").unwrap();

    let found = find_binary_in_dir(&tmp).unwrap();
    assert_eq!(found, bin_path);

    fs::remove_dir_all(&tmp).unwrap();
}

#[test]
fn test_find_binary_missing_returns_error() {
    let tmp = std::env::temp_dir().join(format!("simard-test-empty-{}", std::process::id()));
    fs::create_dir_all(&tmp).unwrap();
    fs::write(tmp.join("not-simard"), b"wrong").unwrap();

    let result = find_binary_in_dir(&tmp);
    assert!(result.is_err());
    assert!(
        result
            .unwrap_err()
            .to_string()
            .contains("not found in downloaded archive")
    );

    fs::remove_dir_all(&tmp).unwrap();
}

#[test]
fn test_find_binary_respects_depth_limit() {
    let tmp = std::env::temp_dir().join(format!("simard-test-deep-{}", std::process::id()));
    let deep = tmp.join("a").join("b").join("c").join("d").join("e");
    fs::create_dir_all(&deep).unwrap();
    fs::write(deep.join("simard"), b"fake-binary").unwrap();

    let result = find_binary_in_dir(&tmp);
    assert!(result.is_err(), "should not find binary beyond depth 3");

    fs::remove_dir_all(&tmp).unwrap();
}

#[test]
fn test_find_binary_ignores_directories_named_simard() {
    let tmp = std::env::temp_dir().join(format!("simard-test-dirname-{}", std::process::id()));
    fs::create_dir_all(&tmp).unwrap();
    fs::create_dir_all(tmp.join("simard")).unwrap();

    let result = find_binary_in_dir(&tmp);
    assert!(result.is_err(), "should not match directory named simard");

    fs::remove_dir_all(&tmp).unwrap();
}

#[test]
fn test_find_binary_at_depth_boundary() {
    let tmp = std::env::temp_dir().join(format!("simard-test-depth3-{}", std::process::id()));
    let at_depth3 = tmp.join("a").join("b").join("c");
    fs::create_dir_all(&at_depth3).unwrap();
    fs::write(at_depth3.join("simard"), b"fake").unwrap();

    let result = find_binary_in_dir(&tmp);
    assert!(result.is_ok(), "binary at depth 3 should be found");

    fs::remove_dir_all(&tmp).unwrap();
}

#[test]
fn test_find_binary_nonexistent_dir_returns_error() {
    let tmp = std::env::temp_dir().join(format!("simard-test-nodir-{}", std::process::id()));
    let result = find_binary_in_dir(&tmp);
    assert!(result.is_err());
}

#[test]
fn test_find_binary_empty_dir_returns_error() {
    let tmp = std::env::temp_dir().join(format!("simard-test-emptydir-{}", std::process::id()));
    fs::create_dir_all(&tmp).unwrap();

    let result = find_binary_in_dir(&tmp);
    assert!(result.is_err());
    assert!(result.unwrap_err().to_string().contains("not found"));

    fs::remove_dir_all(&tmp).unwrap();
}

#[test]
fn test_find_binary_prefers_first_found() {
    let tmp = std::env::temp_dir().join(format!("simard-test-multi-{}", std::process::id()));
    fs::create_dir_all(&tmp).unwrap();
    fs::write(tmp.join("simard"), b"root-binary").unwrap();
    let sub = tmp.join("sub");
    fs::create_dir_all(&sub).unwrap();
    fs::write(sub.join("simard"), b"nested-binary").unwrap();

    let found = find_binary_in_dir(&tmp).unwrap();
    assert!(found.exists());

    fs::remove_dir_all(&tmp).unwrap();
}

#[test]
fn test_find_binary_with_multiple_non_matching_files() {
    let tmp =
        std::env::temp_dir().join(format!("simard-test-multi-nomatch-{}", std::process::id()));
    fs::create_dir_all(&tmp).unwrap();
    fs::write(tmp.join("not-simard"), b"wrong").unwrap();
    fs::write(tmp.join("simard.bak"), b"wrong").unwrap();
    fs::write(tmp.join("simard.exe"), b"wrong").unwrap();
    let result = find_binary_in_dir(&tmp);
    assert!(result.is_err());
    fs::remove_dir_all(&tmp).unwrap();
}

#[test]
fn test_find_binary_at_depth_1() {
    let tmp = std::env::temp_dir().join(format!("simard-test-depth1-{}", std::process::id()));
    let sub = tmp.join("subdir");
    fs::create_dir_all(&sub).unwrap();
    fs::write(sub.join("simard"), b"fake").unwrap();
    let result = find_binary_in_dir(&tmp);
    assert!(result.is_ok());
    assert_eq!(result.unwrap(), sub.join("simard"));
    fs::remove_dir_all(&tmp).unwrap();
}

#[test]
fn test_find_binary_at_depth_2() {
    let tmp = std::env::temp_dir().join(format!("simard-test-depth2-{}", std::process::id()));
    let sub = tmp.join("a").join("b");
    fs::create_dir_all(&sub).unwrap();
    fs::write(sub.join("simard"), b"fake").unwrap();
    let result = find_binary_in_dir(&tmp);
    assert!(result.is_ok());
    fs::remove_dir_all(&tmp).unwrap();
}

#[test]
fn test_find_binary_at_depth_4_not_found() {
    let tmp = std::env::temp_dir().join(format!("simard-test-depth4-{}", std::process::id()));
    let sub = tmp.join("a").join("b").join("c").join("d");
    fs::create_dir_all(&sub).unwrap();
    fs::write(sub.join("simard"), b"fake").unwrap();
    let result = find_binary_in_dir(&tmp);
    assert!(result.is_err());
    fs::remove_dir_all(&tmp).unwrap();
}

#[test]
fn test_find_binary_with_mixed_files_and_dirs() {
    let tmp = std::env::temp_dir().join(format!("simard-test-mixed-{}", std::process::id()));
    fs::create_dir_all(&tmp).unwrap();
    fs::write(tmp.join("README.md"), b"readme").unwrap();
    fs::write(tmp.join("LICENSE"), b"license").unwrap();
    fs::create_dir_all(tmp.join("bin")).unwrap();
    fs::write(tmp.join("bin").join("simard"), b"binary").unwrap();
    let result = find_binary_in_dir(&tmp);
    assert!(result.is_ok());
    fs::remove_dir_all(&tmp).unwrap();
}

#[test]
fn test_find_binary_error_message_content() {
    let tmp = std::env::temp_dir().join(format!("simard-test-errmsg-{}", std::process::id()));
    fs::create_dir_all(&tmp).unwrap();
    let result = find_binary_in_dir(&tmp);
    let err = result.unwrap_err();
    let msg = err.to_string();
    assert!(msg.contains("simard"));
    assert!(msg.contains("not found"));
    fs::remove_dir_all(&tmp).unwrap();
}

#[test]
fn test_find_binary_with_symlink_named_simard() {
    let tmp = std::env::temp_dir().join(format!("simard-test-symlink-{}", std::process::id()));
    fs::create_dir_all(&tmp).unwrap();
    let real = tmp.join("simard_real");
    fs::write(&real, b"binary").unwrap();
    #[cfg(unix)]
    {
        let _ = std::os::unix::fs::symlink(&real, tmp.join("simard"));
    }
    #[cfg(not(unix))]
    {
        fs::write(tmp.join("simard"), b"binary").unwrap();
    }
    let result = find_binary_in_dir(&tmp);
    assert!(result.is_ok(), "should find simard (or symlink to it)");
    fs::remove_dir_all(&tmp).unwrap();
}

#[test]
fn test_find_binary_deeply_nested_multiple_dirs() {
    let tmp =
        std::env::temp_dir().join(format!("simard-test-multi-nested-{}", std::process::id()));
    fs::create_dir_all(tmp.join("dir_a/sub_a")).unwrap();
    fs::create_dir_all(tmp.join("dir_b/sub_b")).unwrap();
    fs::write(tmp.join("dir_a/sub_a/not_simard"), b"wrong").unwrap();
    fs::write(tmp.join("dir_b/sub_b/simard"), b"binary").unwrap();
    let result = find_binary_in_dir(&tmp);
    assert!(result.is_ok(), "should find simard in dir_b/sub_b");
    fs::remove_dir_all(&tmp).unwrap();
}

#[test]
fn test_find_binary_result_path_exists() {
    let tmp = std::env::temp_dir().join(format!("simard-test-exists-{}", std::process::id()));
    fs::create_dir_all(&tmp).unwrap();
    fs::write(tmp.join("simard"), b"binary").unwrap();
    let found = find_binary_in_dir(&tmp).unwrap();
    assert!(found.exists(), "found path should exist");
    assert!(found.is_file(), "found path should be a file");
    fs::remove_dir_all(&tmp).unwrap();
}

#[test]
fn test_find_binary_result_has_correct_name() {
    let tmp = std::env::temp_dir().join(format!("simard-test-name-{}", std::process::id()));
    fs::create_dir_all(&tmp).unwrap();
    fs::write(tmp.join("simard"), b"binary").unwrap();
    let found = find_binary_in_dir(&tmp).unwrap();
    assert_eq!(found.file_name().unwrap(), "simard");
    fs::remove_dir_all(&tmp).unwrap();
}

#[test]
fn test_find_binary_depth_3_found_depth_4_not() {
    let tmp3 = std::env::temp_dir().join(format!("simard-test-d3ok-{}", std::process::id()));
    let d3 = tmp3.join("a").join("b").join("c");
    fs::create_dir_all(&d3).unwrap();
    fs::write(d3.join("simard"), b"found").unwrap();
    assert!(find_binary_in_dir(&tmp3).is_ok());
    fs::remove_dir_all(&tmp3).unwrap();

    let tmp4 = std::env::temp_dir().join(format!("simard-test-d4fail-{}", std::process::id()));
    let d4 = tmp4.join("a").join("b").join("c").join("d");
    fs::create_dir_all(&d4).unwrap();
    fs::write(d4.join("simard"), b"too deep").unwrap();
    assert!(find_binary_in_dir(&tmp4).is_err());
    fs::remove_dir_all(&tmp4).unwrap();
}
