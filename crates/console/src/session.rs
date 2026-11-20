//! Line-oriented session log.
//!
//! This is distinct from the journal: the journal is a replayable record of
//! show edits, while this log records every command and its result for a
//! support case. It is append-only and flushed after every line.

use std::fs::{File, OpenOptions};
use std::io::Write;
use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

#[derive(Debug)]
pub struct SessionLog {
    file: File,
}

impl SessionLog {
    pub fn open(path: &Path) -> Result<Self, String> {
        let file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(path)
            .map_err(|error| format!("cannot open log `{}`: {error}", path.display()))?;
        Ok(Self { file })
    }

    pub fn record(&mut self, line: &str, result: &str) {
        let at = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|duration| duration.as_secs())
            .unwrap_or(0);
        let line = line.replace(['\n', '\r'], " ");
        let result = result.replace(['\n', '\r'], " | ");
        let _ = writeln!(self.file, "{at}\t{result}\t{line}");
        let _ = self.file.flush();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn records_timestamp_result_and_line() {
        let path = std::env::temp_dir().join(format!(
            "openlight-session-{}-{:?}.log",
            std::process::id(),
            std::thread::current().id()
        ));
        let mut log = SessionLog::open(&path).unwrap();
        log.record("Fixture 1 At 50", "Set 1 value");
        log.record("Bad command", "error: nope");
        let text = fs::read_to_string(&path).unwrap();
        assert!(text.contains("\tSet 1 value\tFixture 1 At 50\n"), "{text}");
        assert!(text.contains("\terror: nope\tBad command\n"), "{text}");
        let _ = fs::remove_file(path);
    }
}
