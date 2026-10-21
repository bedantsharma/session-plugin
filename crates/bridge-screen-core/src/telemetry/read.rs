//! Reading a transcript's new bytes: from a byte cursor, at most a bounded slice, whole
//! lines only (Claude Code may be writing the last one), with a cheap substring check
//! before any JSON is parsed.

use std::collections::HashSet;
use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::path::Path;

use serde::Deserialize;

use super::Reply;
use crate::error::Result;

/// The most new transcript bytes one hook reads from one file (spec §9.2, §11.1). The
/// rest is picked up by later hooks.
pub const MAX_HOOK_BYTES: u64 = 512 * 1024;

/// What one read found.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ReadOut {
    /// Each model reply once (the first line of it in this read).
    pub replies: Vec<Reply>,
    /// Where the next read starts.
    pub offset_after: u64,
    /// Whole lines remain after `offset_after` (or a line still being written).
    pub more: bool,
    /// The file was shorter than the cursor (shrunk or replaced): read again from 0.
    pub restarted: bool,
    /// One line was longer than twice the slice and was skipped without being parsed.
    pub skipped_long_line: bool,
}

/// Read the replies in `path` after byte `from`, at most `max` bytes (a line longer than
/// that may extend the slice to `2 × max`). A missing file reads as nothing, not an error.
pub fn read_new(path: &Path, from: u64, max: u64) -> Result<ReadOut> {
    let mut file = match File::open(path) {
        Ok(f) => f,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            return Ok(ReadOut {
                offset_after: from,
                ..ReadOut::default()
            });
        }
        Err(e) => return Err(e.into()),
    };
    let size = file.metadata()?.len();
    let (from, restarted) = if size < from {
        (0, true)
    } else {
        (from, false)
    };
    let want = max.min(size - from);
    let mut buf = Vec::new();
    file.seek(SeekFrom::Start(from))?;
    (&mut file).take(want).read_to_end(&mut buf)?;
    let mut skipped_long_line = false;
    let consumed = match buf.iter().rposition(|&b| b == b'\n') {
        Some(last) => last + 1,
        // One line fills the whole slice: look up to 2 × max for its end, so a long line
        // can't stall the cursor forever.
        None if want == max && size - from > max => {
            let more = (2 * max).min(size - from) - want;
            (&mut file).take(more).read_to_end(&mut buf)?;
            match buf.iter().rposition(|&b| b == b'\n') {
                Some(last) => last + 1,
                // The file ends inside the line: it is still being written.
                None if (buf.len() as u64) < 2 * max => 0,
                None => {
                    // Still no end: skip what was read; the rest of the line fails to
                    // parse on the next read and is skipped with it.
                    skipped_long_line = true;
                    buf.clear();
                    usize::try_from(2 * max).unwrap_or(usize::MAX)
                }
            }
        }
        None => 0,
    };
    let replies = if skipped_long_line {
        Vec::new()
    } else {
        parse_lines(&buf[..consumed])
    };
    let offset_after = from + consumed as u64;
    Ok(ReadOut {
        replies,
        offset_after,
        more: offset_after < size,
        restarted,
        skipped_long_line,
    })
}

/// The longest line the owner's catch-up reads before skipping it like a hook does.
pub const MAX_CATCH_UP_LINE: u64 = 256 * 1024 * 1024;

/// Read `path` from `from` to its last whole line, in slices (for the owner's catch-up;
/// never from a hook). Replies are deduplicated across slices. A line too long for a
/// hook's slice is read whole here (up to [`MAX_CATCH_UP_LINE`]).
pub fn read_to_end(path: &Path, from: u64) -> Result<ReadOut> {
    let mut out = slice(path, from)?;
    let mut seen: HashSet<String> = out.replies.iter().map(|r| r.message_id.clone()).collect();
    while out.more {
        let next = slice(path, out.offset_after)?;
        if next.offset_after == out.offset_after {
            break; // only a line still being written is left
        }
        out.offset_after = next.offset_after;
        out.more = next.more;
        out.skipped_long_line |= next.skipped_long_line;
        out.replies.extend(
            next.replies
                .into_iter()
                .filter(|r| seen.insert(r.message_id.clone())),
        );
    }
    Ok(out)
}

/// One catch-up slice: a hook-sized read, widened while a single line overflows it.
fn slice(path: &Path, from: u64) -> Result<ReadOut> {
    let mut max = MAX_HOOK_BYTES;
    loop {
        let out = read_new(path, from, max)?;
        if !out.skipped_long_line || 2 * max > MAX_CATCH_UP_LINE {
            return Ok(out);
        }
        max *= 2;
    }
}

/// The fields bridge-screen reads from a transcript line; everything else (the message
/// text, tool inputs and outputs) is skipped by the parser and never kept.
#[derive(Deserialize)]
struct Line {
    #[serde(rename = "type", default)]
    kind: Option<String>,
    #[serde(default)]
    timestamp: Option<String>,
    #[serde(rename = "isSidechain", default)]
    sidechain: Option<bool>,
    #[serde(default)]
    message: Option<Message>,
}

#[derive(Deserialize)]
struct Message {
    #[serde(default)]
    id: Option<String>,
    #[serde(default)]
    model: Option<String>,
    #[serde(default)]
    usage: Option<RawUsage>,
}

#[derive(Deserialize)]
struct RawUsage {
    #[serde(default)]
    input_tokens: Option<u64>,
    #[serde(default)]
    output_tokens: Option<u64>,
    #[serde(default)]
    cache_read_input_tokens: Option<u64>,
    #[serde(default)]
    cache_creation_input_tokens: Option<u64>,
    #[serde(default)]
    cache_creation: Option<CacheCreation>,
}

#[derive(Deserialize)]
struct CacheCreation {
    #[serde(default)]
    ephemeral_5m_input_tokens: Option<u64>,
    #[serde(default)]
    ephemeral_1h_input_tokens: Option<u64>,
}

/// Each model reply in `bytes` once, in order of first appearance.
fn parse_lines(bytes: &[u8]) -> Vec<Reply> {
    let mut seen = HashSet::new();
    let mut out = Vec::new();
    for line in bytes.split(|&b| b == b'\n') {
        if !(contains(line, b"\"usage\"") && contains(line, b"\"assistant\"")) {
            continue;
        }
        let Some(reply) = reply(line) else {
            continue;
        };
        if seen.insert(reply.message_id.clone()) {
            out.push(reply);
        }
    }
    out
}

fn contains(hay: &[u8], needle: &[u8]) -> bool {
    hay.windows(needle.len()).any(|w| w == needle)
}

/// A model reply with usage, or `None` for any other line (or a broken one).
fn reply(line: &[u8]) -> Option<Reply> {
    let l: Line = serde_json::from_slice(line).ok()?;
    if l.kind.as_deref() != Some("assistant") {
        return None;
    }
    let m = l.message?;
    let u = m.usage?;
    let id = m.id.filter(|i| !i.is_empty())?;
    // Writes without the 5m/1h split are the default (5-minute) cache.
    let (w5, w1) = match &u.cache_creation {
        Some(c) => (
            c.ephemeral_5m_input_tokens.unwrap_or(0),
            c.ephemeral_1h_input_tokens.unwrap_or(0),
        ),
        None => (u.cache_creation_input_tokens.unwrap_or(0), 0),
    };
    let reply = Reply {
        message_id: id,
        model: m.model.unwrap_or_else(|| "unknown".to_string()),
        ts: l.timestamp.unwrap_or_default(),
        input: u.input_tokens.unwrap_or(0),
        output: u.output_tokens.unwrap_or(0),
        cache_read: u.cache_read_input_tokens.unwrap_or(0),
        cache_write_5m: w5,
        cache_write_1h: w1,
        sidechain: l.sidechain.unwrap_or(false),
    };
    // Replies with no tokens at all (Claude Code's own synthetic messages) cost nothing
    // and aren't model replies worth a price.
    let any =
        reply.input + reply.output + reply.cache_read + reply.cache_write_5m + reply.cache_write_1h;
    (any > 0).then_some(reply)
}
