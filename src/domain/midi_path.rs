use std::path::Path;

pub fn has_supported_midi_extension(path: impl AsRef<Path>) -> bool {
    path.as_ref()
        .extension()
        .and_then(|ext| ext.to_str())
        .is_some_and(|ext| ext.eq_ignore_ascii_case("mid") || ext.eq_ignore_ascii_case("midi"))
}

#[cfg(test)]
mod tests {
    use super::has_supported_midi_extension;
    use std::path::Path;

    #[test]
    fn supports_mid_and_midi_extensions_case_insensitively() {
        assert!(has_supported_midi_extension(Path::new("/tmp/input.mid")));
        assert!(has_supported_midi_extension(Path::new("/tmp/input.MIDI")));
        assert!(has_supported_midi_extension("relative/path/input.Mid"));
        assert!(!has_supported_midi_extension("/tmp/input.wav"));
    }
}
