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
        if let Some(json) = extract_braced_json_slice(fenced) {
            return Some(json);
        }
        if !fenced.is_empty() {
            return Some(fenced);
        }
    }

    extract_braced_json_slice(trimmed)
}

fn extract_markdown_fenced_block(text: &str) -> Option<&str> {
    let stripped = text.strip_prefix("```")?;
    let end = stripped.rfind("```")?;
    let content = stripped[..end].trim();
    if content.is_empty() {
        return None;
    }

    // Handle both "```json\n{...}```" and "```json {...}```".
    if let Some((info, body)) = split_fence_info_and_body(content)
        && is_likely_fence_info(info)
    {
        let body = body.trim_start();
        if !body.is_empty() {
            return Some(body);
        }
    }

    Some(content)
}

fn split_fence_info_and_body(content: &str) -> Option<(&str, &str)> {
    if let Some((first_line, rest)) = content.split_once('\n') {
        return Some((first_line.trim(), rest));
    }

    let whitespace_index = content.find(char::is_whitespace)?;
    let (info, body) = content.split_at(whitespace_index);
    Some((info.trim(), body))
}

fn is_likely_fence_info(info: &str) -> bool {
    if info.is_empty() || info.len() > 64 {
        return false;
    }
    info.chars()
        .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '_' | '-' | '.' | '+' | ':' | '/'))
}

fn extract_braced_json_slice(text: &str) -> Option<&str> {
    let start = text.find('{')?;
    let end = text.rfind('}')?;
    (start <= end).then_some(&text[start..=end])
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
    fn extract_json_payload_parses_inline_markdown_fenced_json() {
        let content = "```json {\"request_id\":\"req-1\"}```";
        let payload = extract_json_payload(content).expect("JSON payload should be extracted");

        assert_eq!(payload, "{\"request_id\":\"req-1\"}");
    }

    #[test]
    fn extract_json_payload_parses_fenced_json_with_no_language() {
        let content = "```\n{\"request_id\":\"req-1\"}\n```";
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
