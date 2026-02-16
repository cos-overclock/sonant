use std::path::{Path, PathBuf};

use gpui::ExternalPaths;
use sonant::domain::{GenerationRequest, has_supported_midi_extension};

use super::{DEBUG_PROMPT_LOG_ENV, DEBUG_PROMPT_PREVIEW_CHARS};

pub(super) fn log_generation_request_submission(request: &GenerationRequest) {
    let prompt_chars = request.prompt.chars().count();
    if helper_debug_prompt_log_enabled() {
        let preview = prompt_preview(&request.prompt, DEBUG_PROMPT_PREVIEW_CHARS);
        eprintln!(
            "sonant-helper: submitting request_id={} prompt_chars={} prompt_preview={:?}",
            request.request_id, prompt_chars, preview
        );
    } else {
        eprintln!(
            "sonant-helper: submitting request_id={} prompt_chars={}",
            request.request_id, prompt_chars
        );
    }
}

fn helper_debug_prompt_log_enabled() -> bool {
    std::env::var(DEBUG_PROMPT_LOG_ENV)
        .ok()
        .as_deref()
        .is_some_and(parse_truthy_flag)
}

pub(super) fn parse_truthy_flag(raw: &str) -> bool {
    raw.eq_ignore_ascii_case("1")
        || raw.eq_ignore_ascii_case("true")
        || raw.eq_ignore_ascii_case("yes")
        || raw.eq_ignore_ascii_case("on")
}

pub(super) fn prompt_preview(prompt: &str, max_chars: usize) -> String {
    let mut chars = prompt.chars();
    let mut preview: String = chars.by_ref().take(max_chars).collect();
    if chars.next().is_some() {
        preview.push_str("...");
    }
    preview
}

pub(super) fn dropped_path_to_load(paths: &ExternalPaths) -> Option<String> {
    choose_dropped_midi_path(paths.paths()).map(|path| path.to_string_lossy().to_string())
}

pub(super) fn choose_dropped_midi_path(paths: &[PathBuf]) -> Option<PathBuf> {
    paths
        .iter()
        .find(|path| has_supported_midi_extension(path))
        .cloned()
        .or_else(|| paths.first().cloned())
}

pub(super) fn display_file_name_from_path(path: &str) -> String {
    Path::new(path)
        .file_name()
        .and_then(|name| name.to_str())
        .filter(|name| !name.is_empty())
        .unwrap_or(path)
        .to_string()
}

pub(super) fn normalize_api_key_input(raw: &str) -> Option<String> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        None
    } else {
        Some(trimmed.to_string())
    }
}
