use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use midly::Smf;

pub(crate) fn write_midi_file(prefix: &str, extension: &str, smf: &Smf<'_>) -> TestFile {
    let mut bytes = Vec::new();
    smf.write_std(&mut bytes)
        .expect("test MIDI serialization must succeed");
    write_bytes_file(prefix, extension, &bytes)
}

pub(crate) fn write_bytes_file(prefix: &str, extension: &str, bytes: &[u8]) -> TestFile {
    static NEXT_ID: AtomicU64 = AtomicU64::new(1);
    let id = NEXT_ID.fetch_add(1, Ordering::Relaxed);
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system clock should be after UNIX_EPOCH")
        .as_nanos();
    let path = std::env::temp_dir().join(format!("{prefix}-{nanos}-{id}.{extension}"));

    fs::write(&path, bytes).expect("test fixture file must be writable");
    TestFile { path }
}

pub(crate) struct TestFile {
    path: PathBuf,
}

impl TestFile {
    pub(crate) fn path(&self) -> &Path {
        &self.path
    }
}

impl Drop for TestFile {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.path);
    }
}
