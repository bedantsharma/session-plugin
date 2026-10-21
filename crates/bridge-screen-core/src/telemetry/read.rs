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
    /// The file's identity now (see [`identity`]), kept with the cursor; `None` for a
    /// missing file.
    pub identity: Option<String>,
}

/// Bytes at the start of a transcript that identify it, with its inode.
const HEAD_BYTES: u64 = 256;

/// `ino:<inode>:len:<n>:head:<hash>` — the inode (Unix) and a hash of the first `n ≤ 256`
/// bytes. A replaced file (a new inode, or different first bytes) gets a new identity,
/// so its cursor restarts from 0 even when it is as long as the old one.
fn identity(file: &mut File, size: u64) -> Result<String> {
    use sha2::{Digest, Sha256};
    #[cfg(unix)]
    let ino = {
        use std::os::unix::fs::MetadataExt as _;
        file.metadata()?.ino()
    };
    #[cfg(not(unix))]
    let ino = 0_u64;
    let n = size.min(HEAD_BYTES);
    let mut head = Vec::new();
    file.seek(SeekFrom::Start(0))?;
    (&mut *file).take(n).read_to_end(&mut head)?;
    let digest = Sha256::digest(&head);
    let hash = digest.iter().take(8).fold(String::new(), |mut out, b| {
        use std::fmt::Write as _;
        let _ = write!(out, "{b:02x}");
        out
    });
    Ok(format!("ino:{ino}:len:{}:head:{hash}", head.len()))
}

/// Is the file still the one `known` describes? Its inode must match and its first bytes
/// (as many as were hashed then) must hash the same.
fn same_file(file: &mut File, size: u64, known: &str) -> Result<bool> {
    // An identity this version can't read: trust the cursor.
    let Some(len) = known
        .strip_prefix("ino:")
        .and_then(|rest| rest.split(':').nth(2))
        .and_then(|l| l.parse::<u64>().ok())
    else {
        return Ok(true);
    };
    if size < len {
        return Ok(false);
    }
    Ok(identity(file, len)? == known)
}

/// Read the replies in `path` after byte `from`, at most `max` bytes (a line longer than
/// that may extend the slice to `2 × max`). A missing file reads as nothing, not an error.
pub fn read_new(path: &Path, from: u64, max: u64) -> Result<ReadOut> {
    read_new_checked(path, from, max, None)
}

/// [`read_new`], restarting from 0 when the file is no longer the one `known` (the
/// identity stored with the cursor) describes: replaced, even by one as long or longer.
pub fn read_new_checked(path: &Path, from: u64, max: u64, known: Option<&str>) -> Result<ReadOut> {
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
    let replaced = match known {
        Some(k) if from > 0 => !same_file(&mut file, size, k)?,
        _ => false,
    };
    let (from, restarted) = if size < from || replaced {
        (0, true)
    } else {
        (from, false)
    };
    let identity = Some(identity(&mut file, size)?);
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
        identity,
    })
}

/// The longest line the owner's catch-up reads before skipping it like a hook does.
pub const MAX_CATCH_UP_LINE: u64 = 256 * 1024 * 1024;

/// Read `path` from `from` to its last whole line, in slices (for the owner's catch-up;
/// never from a hook). Replies are deduplicated across slices. A line too long for a
/// hook's slice is read whole here (up to [`MAX_CATCH_UP_LINE`]).
pub fn read_to_end(path: &Path, from: u64) -> Result<ReadOut> {
    read_to_end_checked(path, from, None)
}

/// [`read_to_end`], restarting when the file was replaced (see [`read_new_checked`]).
pub fn read_to_end_checked(path: &Path, from: u64, known: Option<&str>) -> Result<ReadOut> {
    let mut out = slice(path, from, known)?;
    let mut seen: HashSet<String> = out.replies.iter().map(|r| r.message_id.clone()).collect();
    while out.more {
        let next = slice(path, out.offset_after, None)?;
        if next.offset_after == out.offset_after {
            break; // only a line still being written is left
        }
        out.offset_after = next.offset_after;
        out.more = next.more;
        out.skipped_long_line |= next.skipped_long_line;
        out.identity = next.identity;
        out.replies.extend(
            next.replies
                .into_iter()
                .filter(|r| seen.insert(r.message_id.clone())),
        );
    }
    Ok(out)
}

/// One catch-up slice: a hook-sized read, widened while a single line overflows it.
fn slice(path: &Path, from: u64, known: Option<&str>) -> Result<ReadOut> {
    let mut max = MAX_HOOK_BYTES;
    loop {
        let out = read_new_checked(path, from, max, known)?;
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
    /// `fast` for fast mode, which is priced apart (see [`super::FAST`]).
    #[serde(default)]
    speed: Option<String>,
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
    let mut model = m.model.unwrap_or_else(|| "unknown".to_string());
    if u.speed.as_deref() == Some("fast") {
        model.push_str(super::FAST);
    }
    let reply = Reply {
        message_id: id,
        model,
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
