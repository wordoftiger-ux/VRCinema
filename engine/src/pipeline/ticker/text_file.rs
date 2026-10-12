//! The text files ffmpeg re-reads.

use std::io;
use std::path::Path;
use std::time::Duration;

/// Size of a text file ffmpeg re-reads: the text, then NUL bytes (drawtext stops at the first
/// one). Room for the longest text: the film part and ten messages of 200 characters.
pub const TEXT_FILE_BYTES: usize = 16 * 1024;
/// How many times a write that another program blocks (an antivirus scan) is tried.
const WRITE_TRIES: u32 = 50;

/// Text files ffmpeg re-reads while it runs (`reload=1`: once per frame).
pub struct TickerText;

impl TickerText {
    /// Puts `text` into `path` so ffmpeg reading it every frame never fails and never sees an
    /// empty file: the file always has [`TEXT_FILE_BYTES`] bytes (the text, NUL after it) and
    /// is overwritten in place with one write, never truncated, never replaced.
    ///
    /// Not a temporary file renamed over it (the plan's first idea): on Windows a rename that
    /// replaces the file makes ffmpeg's open of it fail now and then ("Permission denied"),
    /// and drawtext then stops ffmpeg (Review Focus 2). Opening it for writing never conflicts
    /// with ffmpeg's read (both share reading and writing). A write is tried again for a short
    /// while when another program holds the file.
    ///
    /// Not atomic for the reader: ffmpeg maps the file, so a read that lands in the
    /// microseconds of the copy can see the start of the new text with the rest of the old one
    /// (or the new text cut at that point) for a single frame. It is never empty, never
    /// missing and never stops ffmpeg; the next frame reads the whole new text.
    pub fn write_in_place(path: &Path, text: &str) -> io::Result<()> {
        let block = text_block(text);
        let mut attempt = 0;
        loop {
            match overwrite(path, &block) {
                Ok(()) => return Ok(()),
                Err(e) if attempt + 1 < WRITE_TRIES && is_locked(&e) => {
                    attempt += 1;
                    std::thread::sleep(Duration::from_millis(u64::from(attempt.min(10))));
                }
                Err(e) => return Err(e),
            }
        }
    }

    /// The text of a file written by [`Self::write_in_place`].
    pub fn read(path: &Path) -> io::Result<String> {
        let bytes = std::fs::read(path)?;
        let end = bytes.iter().position(|&b| b == 0).unwrap_or(bytes.len());
        Ok(String::from_utf8_lossy(&bytes[..end]).into_owned())
    }
}

/// `text` (cut at a character boundary when too long, no NUL inside) padded with NUL bytes to
/// [`TEXT_FILE_BYTES`].
fn text_block(text: &str) -> Vec<u8> {
    let mut block = Vec::with_capacity(TEXT_FILE_BYTES);
    for c in text.chars().filter(|&c| c != '\0') {
        let mut buf = [0u8; 4];
        let encoded = c.encode_utf8(&mut buf).as_bytes();
        if block.len() + encoded.len() >= TEXT_FILE_BYTES {
            break;
        }
        block.extend_from_slice(encoded);
    }
    block.resize(TEXT_FILE_BYTES, 0);
    block
}

fn overwrite(path: &Path, block: &[u8]) -> io::Result<()> {
    use std::io::Write;
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(false)
        .open(path)?;
    file.write_all(block)
}

/// Another program holds the file (access denied, sharing or lock violation).
fn is_locked(e: &io::Error) -> bool {
    matches!(e.raw_os_error(), Some(5 | 32 | 33)) || e.kind() == io::ErrorKind::PermissionDenied
}
