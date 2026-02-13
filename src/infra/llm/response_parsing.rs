const MAX_ERROR_MESSAGE_LEN: usize = 256;

pub(crate) fn truncate_message(body: &str) -> String {
    let compact = body.trim().replace('\n', " ");
    compact.chars().take(MAX_ERROR_MESSAGE_LEN).collect()
}

pub(crate) fn extract_json_payload(text: &str) -> Option<&str> {
    let trimmed = text.trim();
    if trimmed.is_empty() {
        return None;
    }

    if let Some(fenced) = extract_markdown_fenced_block(trimmed) {
        let fenced = fenced.trim();
        if !fenced.is_empty() {
            return Some(fenced);
        }
    }

    let start = trimmed.find('{')?;
    let end = trimmed.rfind('}')?;
    (start <= end).then_some(&trimmed[start..=end])
}

fn extract_markdown_fenced_block(text: &str) -> Option<&str> {
    let stripped = text.strip_prefix("```")?;
    let first_newline = stripped.find('\n')?;
    let (_, rest) = stripped.split_at(first_newline + 1);
    let end = rest.rfind("```")?;
    Some(&rest[..end])
}

#[cfg(test)]
mod tests {
    use super::{extract_json_payload, truncate_message};

    #[test]
    fn extract_json_payload_parses_markdown_fenced_json() {
        let content = "```json\n{\"request_id\":\"req-1\"}\n```";
        let payload = extract_json_payload(content).expect("JSON payload should be extracted");

        assert_eq!(payload, "{\"request_id\":\"req-1\"}");
    }

    #[test]
    fn extract_json_payload_parses_json_with_surrounding_text() {
        let content = "prefix {\"request_id\":\"req-1\"} suffix";
        let payload = extract_json_payload(content).expect("JSON payload should be extracted");

        assert_eq!(payload, "{\"request_id\":\"req-1\"}");
    }

    #[test]
    fn truncate_message_compacts_newlines_and_limits_length() {
        let input = "line-1\nline-2";
        let truncated = truncate_message(input);

        assert_eq!(truncated, "line-1 line-2");

        let long = "x".repeat(512);
        let truncated = truncate_message(&long);
        assert_eq!(truncated.len(), 256);
    }
}
