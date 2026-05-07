pub fn strip_markdown_fence(raw: &str) -> &str {
    let trimmed = raw.trim();
    if let Some(rest) = trimmed.strip_prefix("```json") {
        rest.strip_suffix("```").unwrap_or(rest).trim()
    } else if let Some(rest) = trimmed.strip_prefix("```") {
        rest.strip_suffix("```").unwrap_or(rest).trim()
    } else {
        trimmed
    }
}

pub fn parse_json_or_content(content: &str) -> serde_json::Value {
    serde_json::from_str(strip_markdown_fence(content))
        .unwrap_or_else(|_| serde_json::json!({ "content": content }))
}
