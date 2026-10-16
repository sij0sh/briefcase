//! The JSONL transport: how to read session records safely, not what they
//! mean. The exact bytes are retained first; every record keeps its ordinal
//! and byte range so a normalized event can be explained back to the line
//! that produced it. Malformed and truncated records stay visible.
use super::super::identity;
use serde_json::Value;

/// One line of a retained JSONL artifact.
#[derive(Clone, Debug)]
pub struct RawRecord {
    /// 1-based line number in the retained artifact.
    pub ordinal: usize,
    /// Byte range of the line, excluding its `\n` terminator.
    pub byte_start: usize,
    pub byte_end: usize,
    /// The strictly parsed record, or `None` when the line is malformed.
    pub value: Option<Value>,
    /// Why the line could not be parsed (duplicate key, invalid UTF-8, …).
    pub error: Option<String>,
}

/// The retained artifact's record stream and its transport-level coverage.
#[derive(Clone, Debug)]
pub struct RawRecordStream {
    pub digest: String,
    pub bytes: usize,
    pub records: Vec<RawRecord>,
    pub blank_lines: usize,
    /// The final line has no `\n` terminator.
    pub missing_final_newline: bool,
    /// The final line has no terminator and does not parse: the writer most
    /// likely stopped mid-record, so the stream is not known to be complete.
    pub truncated_final_record: bool,
}
impl RawRecordStream {
    pub fn valid(&self) -> usize {
        self.records.iter().filter(|r| r.value.is_some()).count()
    }
    pub fn malformed(&self) -> usize {
        self.records.len() - self.valid()
    }
}

/// Split exact bytes into strictly parsed records. Never fails on content:
/// every problem is a visible property of the stream.
pub fn read(bytes: &[u8]) -> RawRecordStream {
    let mut records = Vec::new();
    let mut blank_lines = 0;
    let mut start = 0;
    let mut line = 0;
    while start < bytes.len() {
        line += 1;
        let end = bytes[start..]
            .iter()
            .position(|b| *b == b'\n')
            .map_or(bytes.len(), |i| start + i);
        let slice = &bytes[start..end];
        if slice.iter().all(u8::is_ascii_whitespace) {
            blank_lines += 1;
        } else {
            let (value, error) = match std::str::from_utf8(slice) {
                Err(_) => (None, Some("invalid UTF-8".to_owned())),
                Ok(_) => match identity::parse(slice) {
                    Ok(v) => (Some(v), None),
                    Err(e) => (None, Some(e.to_string())),
                },
            };
            records.push(RawRecord {
                ordinal: line,
                byte_start: start,
                byte_end: end,
                value,
                error,
            });
        }
        start = end + 1;
    }
    let missing_final_newline = !bytes.is_empty() && bytes[bytes.len() - 1] != b'\n';
    let truncated_final_record = missing_final_newline
        && records
            .last()
            .is_some_and(|r| r.byte_end == bytes.len() && r.value.is_none());
    RawRecordStream {
        digest: identity::bytes_digest(bytes),
        bytes: bytes.len(),
        records,
        blank_lines,
        missing_final_newline,
        truncated_final_record,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ranges_ordinals_and_failures_stay_visible() {
        let raw = b"{\"a\":1}\n\nnot json\n{\"a\":1,\"a\":2}\n{\"b\":";
        let s = read(raw);
        assert_eq!(s.records.len(), 4);
        assert_eq!(s.blank_lines, 1);
        assert_eq!(s.records[0].ordinal, 1);
        assert_eq!(
            &raw[s.records[0].byte_start..s.records[0].byte_end],
            b"{\"a\":1}"
        );
        assert_eq!(s.records[1].ordinal, 3);
        assert!(s.records[1].value.is_none());
        assert!(s.records[2].error.as_deref().unwrap().contains("Duplicate"));
        assert!(s.truncated_final_record);
        assert_eq!(s.valid(), 1);
    }

    #[test]
    fn a_complete_unterminated_record_is_not_truncated() {
        let s = read(b"{\"a\":1}");
        assert!(s.missing_final_newline);
        assert!(!s.truncated_final_record);
        assert_eq!(s.valid(), 1);
    }
}
