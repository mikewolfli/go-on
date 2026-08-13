//! Shared `BufRead` helpers for line-oriented streaming.

/// Drain the remainder of the current line from `reader` up to AND INCLUDING
/// the newline, without buffering the line.
///
/// Used after a capped `read_until` detects an oversized line: the reader must
/// be advanced to the next line boundary so subsequent reads start clean, but
/// bytes AFTER the newline (the next line's prefix) must never be consumed.
/// A naive `read()` loop over-consumes the internal buffer, silently dropping
/// the next line's prefix (data corruption). `fill_buf`/`consume` avoid that.
pub fn drain_to_newline<R: std::io::BufRead>(reader: &mut R) {
    loop {
        let buf = match reader.fill_buf() {
            Ok(buf) => buf,
            Err(_) => return,
        };
        if buf.is_empty() {
            return; // EOF
        }
        if let Some(pos) = buf.iter().position(|&b| b == b'\n') {
            reader.consume(pos + 1);
            return;
        }
        let len = buf.len();
        reader.consume(len);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{BufRead, BufReader, Cursor, Read};

    #[test]
    fn drains_only_up_to_newline() {
        // A long line followed by a short sentinel line: after draining, the
        // sentinel must be readable intact (the naive read() bug dropped it).
        let mut data = b"a".repeat(3000);
        data.extend_from_slice(b"\nsentinel\n");
        let mut reader = BufReader::new(Cursor::new(data));

        // Read the first (oversized) line with a cap, then drain.
        let mut line = Vec::new();
        let n = (&mut reader)
            .take(1025)
            .read_until(b'\n', &mut line)
            .unwrap();
        assert!(n > 0);
        assert_ne!(
            line.last(),
            Some(&b'\n'),
            "capped read stops before newline"
        );
        drain_to_newline(&mut reader);

        // The next line must be the intact sentinel.
        line.clear();
        reader.read_until(b'\n', &mut line).unwrap();
        assert_eq!(&line[..], b"sentinel\n");
    }

    #[test]
    fn drains_to_eof_when_no_newline() {
        let mut reader = BufReader::new(Cursor::new(b"abc"));
        drain_to_newline(&mut reader); // no newline — must not hang
        assert!(reader.fill_buf().unwrap().is_empty());
    }

    #[test]
    fn handles_line_ending_at_cap() {
        // Line ends with \n exactly at the read cap: the reader is already at
        // the next line boundary, so callers skip the drain (via the
        // ended_at_newline / oversized checks) — the next line must be
        // readable intact WITHOUT draining.
        let mut data = b"a".repeat(1024);
        data.extend_from_slice(b"\nnext\n");
        let mut reader = BufReader::new(Cursor::new(data));
        let mut line = Vec::new();
        (&mut reader)
            .take(1025)
            .read_until(b'\n', &mut line)
            .unwrap();
        assert_eq!(line.last(), Some(&b'\n'));
        line.clear();
        reader.read_until(b'\n', &mut line).unwrap();
        assert_eq!(&line[..], b"next\n");
    }
}
