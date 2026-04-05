use rustyclawd_core::client::ToolDefinition;

/// Build tool definitions for the standard tool set provided to the RustyClawd
/// client. These mirror the tools available in rustyclawd-tools.
pub(super) fn rustyclawd_tool_definitions() -> Vec<ToolDefinition> {
    vec![
        ToolDefinition::new(
            "Bash",
            "Execute shell commands. Returns stdout, stderr and exit code.",
            serde_json::json!({
                "type": "object",
                "properties": {
                    "command": { "type": "string", "description": "Shell command to execute" },
                    "timeout": { "type": "integer", "description": "Timeout in milliseconds" }
                },
                "required": ["command"]
            }),
        ),
        ToolDefinition::new(
            "Read",
            "Read file contents from the filesystem.",
            serde_json::json!({
                "type": "object",
                "properties": {
                    "file_path": { "type": "string", "description": "Absolute path to read" },
                    "offset": { "type": "integer", "description": "Line offset" },
                    "limit": { "type": "integer", "description": "Max lines to read" }
                },
                "required": ["file_path"]
            }),
        ),
        ToolDefinition::new(
            "Write",
            "Write content to a file.",
            serde_json::json!({
                "type": "object",
                "properties": {
                    "file_path": { "type": "string", "description": "Absolute path to write" },
                    "content": { "type": "string", "description": "Content to write" }
                },
                "required": ["file_path", "content"]
            }),
        ),
        ToolDefinition::new(
            "Edit",
            "Edit a file by replacing text.",
            serde_json::json!({
                "type": "object",
                "properties": {
                    "file_path": { "type": "string", "description": "Absolute path" },
                    "old_string": { "type": "string" },
                    "new_string": { "type": "string" }
                },
                "required": ["file_path", "old_string", "new_string"]
            }),
        ),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tool_definitions_contains_expected_tools() {
        let tools = rustyclawd_tool_definitions();
        assert_eq!(tools.len(), 4);
        let names: Vec<&str> = tools.iter().map(|t| t.name.as_str()).collect();
        assert!(names.contains(&"Bash"));
        assert!(names.contains(&"Read"));
        assert!(names.contains(&"Write"));
        assert!(names.contains(&"Edit"));
    }

    #[test]
    fn tool_definitions_all_have_descriptions() {
        let tools = rustyclawd_tool_definitions();
        for tool in &tools {
            assert!(
                !tool.description.is_empty(),
                "tool {} has empty description",
                tool.name
            );
        }
    }

    #[test]
    fn tool_definitions_have_valid_json_schemas() {
        let tools = rustyclawd_tool_definitions();
        for tool in &tools {
            let schema = &tool.input_schema;
            assert_eq!(
                schema["type"].as_str(),
                Some("object"),
                "tool {} schema should be object type",
                tool.name
            );
            assert!(
                schema["properties"].is_object(),
                "tool {} should have properties",
                tool.name
            );
        }
    }

    #[test]
    fn tool_definitions_bash_has_command_required() {
        let tools = rustyclawd_tool_definitions();
        let bash = tools.iter().find(|t| t.name == "Bash").unwrap();
        let required = bash.input_schema["required"].as_array().unwrap();
        assert!(required.iter().any(|v| v.as_str() == Some("command")));
    }

    #[test]
    fn tool_definitions_write_has_file_path_and_content_required() {
        let tools = rustyclawd_tool_definitions();
        let write = tools.iter().find(|t| t.name == "Write").unwrap();
        let required = write.input_schema["required"].as_array().unwrap();
        let required_strs: Vec<&str> = required.iter().filter_map(|v| v.as_str()).collect();
        assert!(required_strs.contains(&"file_path"));
        assert!(required_strs.contains(&"content"));
    }

    #[test]
    fn tool_definitions_edit_has_three_required_fields() {
        let tools = rustyclawd_tool_definitions();
        let edit = tools.iter().find(|t| t.name == "Edit").unwrap();
        let required = edit.input_schema["required"].as_array().unwrap();
        assert_eq!(required.len(), 3);
    }

    #[test]
    fn tool_definitions_read_has_file_path_required() {
        let tools = rustyclawd_tool_definitions();
        let read = tools.iter().find(|t| t.name == "Read").unwrap();
        let required = read.input_schema["required"].as_array().unwrap();
        assert!(required.iter().any(|v| v.as_str() == Some("file_path")));
    }
}
