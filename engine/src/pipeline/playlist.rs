//! Live HLS playlist text (port of `src/pipeline/playlist.ts`).

#[derive(Debug, Clone, PartialEq)]
pub struct PlaylistSegment {
    pub seq: u64,
    pub name: String,
    pub duration: f64,
    /// The segment starts a new source (another video or a filler slate).
    pub discontinuity: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub struct LivePlaylistInput {
    pub segments: Vec<PlaylistSegment>,
    pub target_duration: f64,
    /// Number of discontinuity markers that already left the playlist window.
    pub discontinuity_sequence: u64,
    /// Segment path prefix relative to the playlist URL, e.g. `"main/"`.
    pub path_prefix: String,
}

pub fn build_live_playlist(input: &LivePlaylistInput) -> String {
    let media_sequence = input.segments.first().map_or(0, |s| s.seq);
    let mut lines = vec![
        "#EXTM3U".to_string(),
        "#EXT-X-VERSION:3".to_string(),
        // `as` saturates and maps NaN to 0.
        format!(
            "#EXT-X-TARGETDURATION:{}",
            input.target_duration.ceil() as i64
        ),
        format!("#EXT-X-MEDIA-SEQUENCE:{media_sequence}"),
        format!(
            "#EXT-X-DISCONTINUITY-SEQUENCE:{}",
            input.discontinuity_sequence
        ),
    ];
    for s in &input.segments {
        if s.discontinuity {
            lines.push("#EXT-X-DISCONTINUITY".to_string());
        }
        lines.push(format!("#EXTINF:{:.6},", s.duration));
        lines.push(format!("{}{}", input.path_prefix, s.name));
    }
    let mut text = lines.join("\n");
    text.push('\n');
    text
}

#[cfg(test)]
mod tests {
    use super::*;

    fn seg(seq: u64, name: &str, duration: f64, discontinuity: bool) -> PlaylistSegment {
        PlaylistSegment {
            seq,
            name: name.to_string(),
            duration,
            discontinuity,
        }
    }

    #[test]
    fn builds_live_playlist_with_numbering_and_discontinuities() {
        let text = build_live_playlist(&LivePlaylistInput {
            target_duration: 4.0,
            discontinuity_sequence: 2,
            segments: vec![seg(41, "41.ts", 4.004, false), seg(42, "42.ts", 3.5, true)],
            path_prefix: "main/".to_string(),
        });
        let lines: Vec<&str> = text.split('\n').collect();
        assert_eq!(
            lines,
            vec![
                "#EXTM3U",
                "#EXT-X-VERSION:3",
                "#EXT-X-TARGETDURATION:4",
                "#EXT-X-MEDIA-SEQUENCE:41",
                "#EXT-X-DISCONTINUITY-SEQUENCE:2",
                "#EXTINF:4.004000,",
                "main/41.ts",
                "#EXT-X-DISCONTINUITY",
                "#EXTINF:3.500000,",
                "main/42.ts",
                "",
            ]
        );
    }

    #[test]
    fn rounds_target_duration_up_to_an_integer() {
        let text = build_live_playlist(&LivePlaylistInput {
            target_duration: 6.2,
            discontinuity_sequence: 0,
            segments: vec![],
            path_prefix: String::new(),
        });
        assert!(text.contains("#EXT-X-TARGETDURATION:7"));
        assert!(text.contains("#EXT-X-MEDIA-SEQUENCE:0"));
        assert!(text.ends_with('\n'));
    }
}
