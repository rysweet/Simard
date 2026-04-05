use std::path::PathBuf;

pub(crate) fn next_required(
    args: &mut impl Iterator<Item = String>,
    label: &'static str,
) -> Result<String, Box<dyn std::error::Error>> {
    args.next()
        .ok_or_else(|| format!("expected {label}").into())
}

pub(crate) fn next_optional_path(args: &mut impl Iterator<Item = String>) -> Option<PathBuf> {
    args.next().map(PathBuf::from)
}

pub(crate) fn reject_extra_args(
    mut args: impl Iterator<Item = String>,
) -> Result<(), Box<dyn std::error::Error>> {
    if let Some(extra) = args.next() {
        let mut extras = vec![extra];
        extras.extend(args);
        return Err(format!("unexpected trailing arguments: {}", extras.join(" ")).into());
    }
    Ok(())
}

pub(crate) fn parse_state_root_and_json(
    trailing: Vec<String>,
) -> Result<(Option<PathBuf>, bool), Box<dyn std::error::Error>> {
    match trailing.as_slice() {
        [] => Ok((None, false)),
        [flag] if flag == "--json" => Ok((None, true)),
        [state_root] => Ok((Some(PathBuf::from(state_root)), false)),
        [state_root, flag] if flag == "--json" => Ok((Some(PathBuf::from(state_root)), true)),
        _ => Err(format!("unexpected trailing arguments: {}", trailing.join(" ")).into()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_state_root_and_json_empty() {
        let (root, json) = parse_state_root_and_json(vec![]).unwrap();
        assert!(root.is_none());
        assert!(!json);
    }

    #[test]
    fn test_parse_state_root_and_json_flag_only() {
        let (root, json) = parse_state_root_and_json(vec!["--json".to_string()]).unwrap();
        assert!(root.is_none());
        assert!(json);
    }

    #[test]
    fn test_parse_state_root_and_json_path_and_flag() {
        let (root, json) =
            parse_state_root_and_json(vec!["/tmp/state".to_string(), "--json".to_string()])
                .unwrap();
        assert_eq!(root.unwrap(), PathBuf::from("/tmp/state"));
        assert!(json);
    }

    #[test]
    fn test_parse_state_root_and_json_path_only() {
        let (root, json) = parse_state_root_and_json(vec!["/some/path".to_string()]).unwrap();
        assert_eq!(root.unwrap(), PathBuf::from("/some/path"));
        assert!(!json);
    }

    #[test]
    fn test_parse_state_root_and_json_too_many_args() {
        let result =
            parse_state_root_and_json(vec!["a".to_string(), "b".to_string(), "c".to_string()]);
        assert!(result.is_err());
        assert!(
            result
                .unwrap_err()
                .to_string()
                .contains("unexpected trailing")
        );
    }

    #[test]
    fn test_next_required_returns_value_when_present() {
        let mut iter = vec!["value".to_string()].into_iter();
        let result = next_required(&mut iter, "test");
        assert_eq!(result.unwrap(), "value");
    }

    #[test]
    fn test_next_required_errors_when_missing() {
        let mut iter = Vec::<String>::new().into_iter();
        let result = next_required(&mut iter, "my-label");
        assert!(result.is_err());
        assert!(
            result
                .unwrap_err()
                .to_string()
                .contains("expected my-label")
        );
    }

    #[test]
    fn test_next_optional_path_returns_some_when_present() {
        let mut iter = vec!["/foo/bar".to_string()].into_iter();
        let result = next_optional_path(&mut iter);
        assert_eq!(result, Some(PathBuf::from("/foo/bar")));
    }

    #[test]
    fn test_next_optional_path_returns_none_when_empty() {
        let mut iter = Vec::<String>::new().into_iter();
        let result = next_optional_path(&mut iter);
        assert!(result.is_none());
    }

    #[test]
    fn test_reject_extra_args_ok_when_empty() {
        let iter = Vec::<String>::new().into_iter();
        assert!(reject_extra_args(iter).is_ok());
    }

    #[test]
    fn test_reject_extra_args_errors_with_one_arg() {
        let iter = vec!["extra1".to_string()].into_iter();
        let result = reject_extra_args(iter);
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("extra1"));
    }

    #[test]
    fn test_reject_extra_args_collects_multiple() {
        let iter = vec!["a".to_string(), "b".to_string(), "c".to_string()].into_iter();
        let result = reject_extra_args(iter);
        assert!(result.is_err());
        let msg = result.unwrap_err().to_string();
        assert!(msg.contains("a b c"));
    }

    #[test]
    fn test_parse_state_root_and_json_reversed_order_is_error() {
        let result = parse_state_root_and_json(vec!["--json".to_string(), "/state".to_string()]);
        assert!(result.is_err());
    }
}
