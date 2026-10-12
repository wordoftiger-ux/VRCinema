//! Info ticker burned into the stream (spec section 7): a half-transparent band with a scrolling
//! text (the film, who added it, the next film, the request page and the owner's messages), the
//! QR code of the request page and a progress line, drawn by ffmpeg while the clip is converted.
//!
//! ffmpeg reads the text from a file in the clip's folder (`reload=1`), so the text changes
//! without a restart: [`TickerText::write_in_place`] overwrites it in place. Everything the
//! filter reads (text, caption, font, code) sits in or next to ffmpeg's working directory and is
//! named relatively, so no Windows path (`C:\…`) has to be escaped inside a filter.
//!
//! Progress line: `drawbox` cannot follow time in this ffmpeg (its `t` is the thickness and its
//! expressions are evaluated once, when the filter is configured), so the line is the fallback
//! the spec names: a colour source slid in from the left by `overlay`, whose `x` is evaluated
//! for every frame.
//!
//! Modules: [`settings`] (what the owner sets), [`text`] (what the band says), [`filter`]
//! (how ffmpeg draws it), [`writer`] (keeping the text up to date while ffmpeg runs) and
//! [`text_file`] (the file ffmpeg re-reads).

mod filter;
mod settings;
mod text;
mod text_file;
mod writer;

pub use filter::*;
pub use settings::*;
pub use text::*;
pub use text_file::*;
pub use writer::*;

/// The ticker text, in the clip's folder (ffmpeg's working directory).
pub const TEXT_FILE: &str = "ticker.txt";
/// The caption under the QR code ("Add a video" in the slate language).
pub const CAPTION_FILE: &str = "qr.txt";
/// The QR code of the request page.
pub const QR_FILE: &str = "qr.png";
/// Folder next to the clip folders that holds one copy of each font the ticker uses.
pub const FONTS_DIR: &str = "_fonts";

#[cfg(test)]
mod tests;
