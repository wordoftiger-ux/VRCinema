use std::collections::HashMap;
use std::io;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VideoEncoder {
    Libx264,
    H264Nvenc,
    H264Qsv,
    H264Amf,
}

impl VideoEncoder {
    pub const ALL: [VideoEncoder; 4] = [
        VideoEncoder::Libx264,
        VideoEncoder::H264Nvenc,
        VideoEncoder::H264Qsv,
        VideoEncoder::H264Amf,
    ];

    pub fn as_str(&self) -> &'static str {
        match self {
            VideoEncoder::Libx264 => "libx264",
            VideoEncoder::H264Nvenc => "h264_nvenc",
            VideoEncoder::H264Qsv => "h264_qsv",
            VideoEncoder::H264Amf => "h264_amf",
        }
    }

    fn parse(name: &str) -> Option<VideoEncoder> {
        Self::ALL.into_iter().find(|e| e.as_str() == name)
    }
}

#[derive(Debug, Clone)]
pub struct AppConfig {
    pub port: u16,
    pub host: String,
    pub admin_password: String,
    pub session_secret: String,
    pub public_url: String,
    pub data_dir: PathBuf,
    pub ffmpeg_path: PathBuf,
    pub ffprobe_path: PathBuf,
    pub ytdlp_path: PathBuf,
    pub ytdlp_js_runtime: Option<String>,
    pub video_encoder: VideoEncoder,
    pub trust_cloudflare: bool,
    pub trust_proxy: bool,
    pub ytdlp_cookies: Option<PathBuf>,
    pub pixeldrain_api_key: Option<String>,
    pub media_dirs: Vec<PathBuf>,
    pub font_file: PathBuf,
    pub segment_seconds: u32,
    pub window_size: usize,
}

#[derive(Debug, thiserror::Error)]
pub enum ConfigError {
    #[error("{0}")]
    Invalid(String),
}

/// Zmienna środowiskowa po przycięciu; pusta wartość traktowana jak brak.
fn trimmed<'a>(env: &'a HashMap<String, String>, key: &str) -> Option<&'a str> {
    env.get(key).map(|v| v.trim()).filter(|v| !v.is_empty())
}

fn flag(env: &HashMap<String, String>, key: &str) -> bool {
    env.get(key).is_some_and(|v| {
        matches!(
            v.trim().to_lowercase().as_str(),
            "1" | "true" | "yes" | "tak"
        )
    })
}

fn default_font(windows: bool) -> &'static str {
    if windows {
        "C:/Windows/Fonts/arial.ttf"
    } else {
        "/usr/share/fonts/truetype/dejavu/DejaVuSans.ttf"
    }
}

fn random_secret() -> Result<String, ConfigError> {
    let mut bytes = [0u8; 32];
    getrandom::fill(&mut bytes)
        .map_err(|e| ConfigError::Invalid(format!("Nie można wygenerować SESSION_SECRET: {e}")))?;
    Ok(bytes.iter().map(|b| format!("{b:02x}")).collect())
}

pub fn load_config(
    env: &HashMap<String, String>,
    root: &Path,
    windows: bool,
) -> Result<AppConfig, ConfigError> {
    let admin_password = trimmed(env, "ADMIN_PASSWORD").ok_or_else(|| {
        ConfigError::Invalid(
            "Brak ADMIN_PASSWORD w pliku .env — ustaw hasło do panelu administratora.".into(),
        )
    })?;
    if admin_password == "zmien-mnie" || admin_password.encode_utf16().count() < 8 {
        return Err(ConfigError::Invalid(
            "ADMIN_PASSWORD musi mieć co najmniej 8 znaków i nie może być przykładowym \"zmien-mnie\"."
                .into(),
        ));
    }

    let encoder_name = trimmed(env, "VIDEO_ENCODER").unwrap_or("libx264");
    let video_encoder = VideoEncoder::parse(encoder_name).ok_or_else(|| {
        let allowed: Vec<&str> = VideoEncoder::ALL.iter().map(|e| e.as_str()).collect();
        ConfigError::Invalid(format!(
            "Nieznany VIDEO_ENCODER \"{encoder_name}\". Dozwolone: {}",
            allowed.join(", ")
        ))
    })?;

    let exe = if windows { ".exe" } else { "" };
    let bin_dir = root.join("bin");

    let port = match env.get("PORT") {
        None => 8080,
        Some(raw) => raw
            .trim()
            .parse::<u16>()
            .ok()
            .filter(|p| *p > 0)
            .ok_or_else(|| ConfigError::Invalid(format!("Niepoprawny PORT: {raw}")))?,
    };

    let session_secret = match trimmed(env, "SESSION_SECRET") {
        Some(s) => s.to_string(),
        None => random_secret()?,
    };

    let tool_path = |key: &str, name: &str| -> PathBuf {
        trimmed(env, key)
            .map(PathBuf::from)
            .unwrap_or_else(|| bin_dir.join(format!("{name}{exe}")))
    };

    Ok(AppConfig {
        port,
        host: trimmed(env, "HOST").unwrap_or("0.0.0.0").to_string(),
        admin_password: admin_password.to_string(),
        session_secret,
        public_url: trimmed(env, "PUBLIC_URL")
            .unwrap_or("")
            .trim_end_matches('/')
            .to_string(),
        data_dir: root.join(trimmed(env, "DATA_DIR").unwrap_or("data")),
        ffmpeg_path: tool_path("FFMPEG_PATH", "ffmpeg"),
        ffprobe_path: tool_path("FFPROBE_PATH", "ffprobe"),
        ytdlp_path: tool_path("YTDLP_PATH", "yt-dlp"),
        ytdlp_js_runtime: trimmed(env, "YTDLP_JS_RUNTIME").map(str::to_string),
        video_encoder,
        trust_cloudflare: flag(env, "TRUST_CLOUDFLARE"),
        trust_proxy: flag(env, "TRUST_PROXY"),
        ytdlp_cookies: trimmed(env, "YTDLP_COOKIES").map(PathBuf::from),
        pixeldrain_api_key: trimmed(env, "PIXELDRAIN_API_KEY").map(str::to_string),
        media_dirs: env
            .get("MEDIA_DIRS")
            .map(|v| v.as_str())
            .unwrap_or("")
            .split(';')
            .map(str::trim)
            .filter(|d| !d.is_empty())
            .map(PathBuf::from)
            .collect(),
        font_file: PathBuf::from(trimmed(env, "FONT_FILE").unwrap_or(default_font(windows))),
        segment_seconds: 4,
        window_size: 10,
    })
}

/// Czyta plik .env, także zapisany przez Notatnik lub PowerShell ze znacznikiem BOM.
/// Niepoprawny UTF-8 jest zamieniany znakiem zastępczym (jak w Node), bez paniki.
pub fn read_env_file(path: &Path) -> io::Result<Vec<(String, String)>> {
    let bytes = std::fs::read(path)?;
    let text = String::from_utf8_lossy(&bytes);
    let text = text.strip_prefix('\u{FEFF}').unwrap_or(&text);
    Ok(parse_env(text))
}

/// Parser .env zgodny z `util.parseEnv` z Node: komentarze `#`, `export`, wartości w
/// cudzysłowach (`"`, `'`, `` ` ``), w podwójnych `\n` oznacza nowy wiersz. Wartości bez
/// cudzysłowów są literalne (backslashe zostają). Późniejszy duplikat nadpisuje wcześniejszy.
fn parse_env(text: &str) -> Vec<(String, String)> {
    let text = text.replace("\r\n", "\n").replace('\r', "\n");
    let mut out: Vec<(String, String)> = Vec::new();
    let mut pos = 0;

    while pos < text.len() {
        let line_end = text[pos..].find('\n').map_or(text.len(), |i| pos + i);
        let line_start = pos;
        let line = &text[line_start..line_end];
        pos = line_end + 1;

        let trimmed_line = line.trim();
        if trimmed_line.is_empty() || trimmed_line.starts_with('#') {
            continue;
        }
        let Some(eq) = line.find('=') else {
            continue;
        };
        let key = line[..eq].trim();
        let key = key
            .strip_prefix("export")
            .filter(|k| k.starts_with(char::is_whitespace))
            .map_or(key, str::trim_start);
        if key.is_empty() {
            continue;
        }

        let raw_value = &line[eq + 1..];
        let value_start = raw_value.trim_start();
        let value_offset = line_start + eq + 1 + (raw_value.len() - value_start.len());
        let quote = value_start
            .chars()
            .next()
            .filter(|c| matches!(c, '"' | '\'' | '`'));
        // Wartość w cudzysłowie może obejmować kilka wierszy.
        let quoted = quote.and_then(|q| {
            let body_start = value_offset + 1;
            text[body_start..]
                .find(q)
                .map(|i| (q, body_start, body_start + i))
        });

        let value = if let Some((q, body_start, body_end)) = quoted {
            pos = text[body_end..]
                .find('\n')
                .map_or(text.len(), |i| body_end + i + 1);
            let body = &text[body_start..body_end];
            if q == '"' {
                body.replace("\\n", "\n")
            } else {
                body.to_string()
            }
        } else {
            let cut = value_start.find('#').unwrap_or(value_start.len());
            value_start[..cut].trim().to_string()
        };

        let key = key.to_string();
        match out.iter_mut().find(|(k, _)| *k == key) {
            Some(entry) => entry.1 = value,
            None => out.push((key, value)),
        }
    }
    out
}

/// Dopisuje wartości z pliku, nie nadpisując zmiennych ustawionych już w środowisku.
pub fn apply_env(target: &mut HashMap<String, String>, values: Vec<(String, String)>) {
    for (k, v) in values {
        target.entry(k).or_insert(v);
    }
}

/// Katalog główny aplikacji: ten z `public/`, najpierw obok pliku wykonywalnego, potem cwd.
pub fn find_root(exe_dir: &Path, cwd: &Path) -> PathBuf {
    if exe_dir.join("public").is_dir() {
        exe_dir.to_path_buf()
    } else {
        cwd.to_path_buf()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn env(pairs: &[(&str, &str)]) -> HashMap<String, String> {
        pairs
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect()
    }

    fn err_text(r: Result<AppConfig, ConfigError>) -> String {
        r.expect_err("oczekiwano błędu").to_string()
    }

    fn root() -> PathBuf {
        PathBuf::from("/proj")
    }

    #[test]
    fn wymaga_admin_password() {
        let e = err_text(load_config(&env(&[]), &root(), false));
        assert!(e.contains("ADMIN_PASSWORD"));
        // sama spacja = brak
        let e = err_text(load_config(
            &env(&[("ADMIN_PASSWORD", "   ")]),
            &root(),
            false,
        ));
        assert!(e.contains("Brak ADMIN_PASSWORD"));
    }

    #[test]
    fn odrzuca_domyslne_i_zbyt_krotkie_haslo() {
        for pw in ["zmien-mnie", "abc"] {
            let e = err_text(load_config(&env(&[("ADMIN_PASSWORD", pw)]), &root(), false));
            assert!(e.contains("ADMIN_PASSWORD"), "{e}");
        }
        let e = err_text(load_config(
            &env(&[("ADMIN_PASSWORD", "abc")]),
            &root(),
            false,
        ));
        assert_eq!(
            e,
            "ADMIN_PASSWORD musi mieć co najmniej 8 znaków i nie może być przykładowym \"zmien-mnie\"."
        );
    }

    #[test]
    fn ustawia_wartosci_domyslne() {
        let c = load_config(&env(&[("ADMIN_PASSWORD", "haslo123")]), &root(), true).unwrap();
        assert_eq!(c.port, 8080);
        assert_eq!(c.host, "0.0.0.0");
        assert_eq!(c.segment_seconds, 4);
        assert_eq!(c.window_size, 10);
        assert_eq!(c.video_encoder, VideoEncoder::Libx264);
        assert!(!c.trust_cloudflare);
        assert!(!c.trust_proxy);
        assert_eq!(c.public_url, "");
        assert_eq!(c.data_dir, root().join("data"));
        assert_eq!(c.ffmpeg_path, root().join("bin").join("ffmpeg.exe"));
        assert_eq!(c.ffprobe_path, root().join("bin").join("ffprobe.exe"));
        assert_eq!(c.ytdlp_path, root().join("bin").join("yt-dlp.exe"));
        assert_eq!(c.ytdlp_js_runtime, None);
        assert_eq!(c.ytdlp_cookies, None);
        assert_eq!(c.pixeldrain_api_key, None);
        assert!(c.media_dirs.is_empty());
        assert_eq!(c.font_file, PathBuf::from("C:/Windows/Fonts/arial.ttf"));
        assert!(c.session_secret.len() >= 32);
    }

    #[test]
    fn losowy_sekret_sesji_jest_rozny_i_hex() {
        let e = env(&[("ADMIN_PASSWORD", "haslo123")]);
        let a = load_config(&e, &root(), false).unwrap().session_secret;
        let b = load_config(&e, &root(), false).unwrap().session_secret;
        assert_eq!(a.len(), 64);
        assert!(a.chars().all(|c| c.is_ascii_hexdigit()));
        assert_ne!(a, b);
        let c = load_config(
            &env(&[("ADMIN_PASSWORD", "haslo123"), ("SESSION_SECRET", " moj ")]),
            &root(),
            false,
        )
        .unwrap();
        assert_eq!(c.session_secret, "moj");
    }

    #[test]
    fn na_linuksie_uzywa_nazw_bez_exe() {
        let c = load_config(&env(&[("ADMIN_PASSWORD", "haslo123")]), &root(), false).unwrap();
        assert_eq!(c.ffmpeg_path, root().join("bin").join("ffmpeg"));
        assert_eq!(c.ytdlp_path, root().join("bin").join("yt-dlp"));
        assert_eq!(
            c.font_file,
            PathBuf::from("/usr/share/fonts/truetype/dejavu/DejaVuSans.ttf")
        );
    }

    #[test]
    fn czyta_nadpisania_z_env() {
        let c = load_config(
            &env(&[
                ("ADMIN_PASSWORD", "haslo123"),
                ("PORT", "9000"),
                ("VIDEO_ENCODER", "h264_nvenc"),
                ("TRUST_CLOUDFLARE", "true"),
                ("TRUST_PROXY", " TAK "),
                ("FFMPEG_PATH", "/usr/bin/ffmpeg"),
                ("PUBLIC_URL", "https://tv.example.com/"),
                ("HOST", "127.0.0.1"),
                ("DATA_DIR", "/var/lib/mp4"),
                ("YTDLP_COOKIES", "/c/cookies.txt"),
                ("PIXELDRAIN_API_KEY", " klucz "),
                ("FONT_FILE", "/f.ttf"),
                ("YTDLP_JS_RUNTIME", "deno"),
            ]),
            &root(),
            false,
        )
        .unwrap();
        assert_eq!(c.port, 9000);
        assert_eq!(c.host, "127.0.0.1");
        assert_eq!(c.video_encoder, VideoEncoder::H264Nvenc);
        assert_eq!(c.video_encoder.as_str(), "h264_nvenc");
        assert!(c.trust_cloudflare);
        assert!(c.trust_proxy);
        assert_eq!(c.ffmpeg_path, PathBuf::from("/usr/bin/ffmpeg"));
        assert_eq!(c.public_url, "https://tv.example.com");
        assert_eq!(c.data_dir, PathBuf::from("/var/lib/mp4"));
        assert_eq!(c.ytdlp_cookies, Some(PathBuf::from("/c/cookies.txt")));
        assert_eq!(c.pixeldrain_api_key.as_deref(), Some("klucz"));
        assert_eq!(c.font_file, PathBuf::from("/f.ttf"));
        assert_eq!(c.ytdlp_js_runtime.as_deref(), Some("deno"));
    }

    #[test]
    fn public_url_obcina_wszystkie_koncowe_ukosniki() {
        let c = load_config(
            &env(&[
                ("ADMIN_PASSWORD", "haslo123"),
                ("PUBLIC_URL", " https://a.b// "),
            ]),
            &root(),
            false,
        )
        .unwrap();
        assert_eq!(c.public_url, "https://a.b");
    }

    #[test]
    fn ytdlp_js_runtime_puste_to_none() {
        for v in ["", "   "] {
            let c = load_config(
                &env(&[("ADMIN_PASSWORD", "haslo123"), ("YTDLP_JS_RUNTIME", v)]),
                &root(),
                false,
            )
            .unwrap();
            assert_eq!(c.ytdlp_js_runtime, None);
        }
        let c = load_config(
            &env(&[
                ("ADMIN_PASSWORD", "haslo123"),
                ("YTDLP_JS_RUNTIME", " node "),
            ]),
            &root(),
            false,
        )
        .unwrap();
        assert_eq!(c.ytdlp_js_runtime.as_deref(), Some("node"));
    }

    #[test]
    fn wszystkie_enkodery() {
        for (name, enc) in [
            ("libx264", VideoEncoder::Libx264),
            ("h264_nvenc", VideoEncoder::H264Nvenc),
            ("h264_qsv", VideoEncoder::H264Qsv),
            ("h264_amf", VideoEncoder::H264Amf),
        ] {
            let c = load_config(
                &env(&[("ADMIN_PASSWORD", "haslo123"), ("VIDEO_ENCODER", name)]),
                &root(),
                false,
            )
            .unwrap();
            assert_eq!(c.video_encoder, enc);
            assert_eq!(enc.as_str(), name);
        }
    }

    #[test]
    fn media_dirs_rozdzielone_srednikami_domyslnie_puste() {
        let c = load_config(&env(&[("ADMIN_PASSWORD", "haslo123")]), &root(), true).unwrap();
        assert!(c.media_dirs.is_empty());
        let c = load_config(
            &env(&[
                ("ADMIN_PASSWORD", "haslo123"),
                ("MEDIA_DIRS", " D:/Filmy ; E:/Seriale;;"),
            ]),
            &root(),
            true,
        )
        .unwrap();
        assert_eq!(
            c.media_dirs,
            vec![PathBuf::from("D:/Filmy"), PathBuf::from("E:/Seriale")]
        );
    }

    #[test]
    fn odrzuca_nieznany_enkoder() {
        let e = err_text(load_config(
            &env(&[("ADMIN_PASSWORD", "haslo123"), ("VIDEO_ENCODER", "foo")]),
            &root(),
            false,
        ));
        assert!(e.contains("VIDEO_ENCODER"));
        assert_eq!(
            e,
            "Nieznany VIDEO_ENCODER \"foo\". Dozwolone: libx264, h264_nvenc, h264_qsv, h264_amf"
        );
    }

    #[test]
    fn odrzuca_niepoprawny_port() {
        for p in ["0", "65536", "abc", "-1", "1.5", ""] {
            let e = err_text(load_config(
                &env(&[("ADMIN_PASSWORD", "haslo123"), ("PORT", p)]),
                &root(),
                false,
            ));
            assert_eq!(e, format!("Niepoprawny PORT: {p}"));
        }
        let c = load_config(
            &env(&[("ADMIN_PASSWORD", "haslo123"), ("PORT", "65535")]),
            &root(),
            false,
        )
        .unwrap();
        assert_eq!(c.port, 65535);
    }

    fn write_env(content: &[u8]) -> (tempfile::TempDir, PathBuf) {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join(".env");
        std::fs::write(&file, content).unwrap();
        (dir, file)
    }

    fn map(v: Vec<(String, String)>) -> HashMap<String, String> {
        v.into_iter().collect()
    }

    #[test]
    fn read_env_file_bom_i_crlf() {
        let (_d, f) = write_env(
            "\u{FEFF}ADMIN_PASSWORD=haslo123\r\nMEDIA_DIRS=D:\\Filmy;E:\\Seriale\r\n# komentarz\r\nPORT=9000\r\n"
                .as_bytes(),
        );
        let v = read_env_file(&f).unwrap();
        assert_eq!(v.len(), 3);
        assert_eq!(
            map(v),
            env(&[
                ("ADMIN_PASSWORD", "haslo123"),
                ("MEDIA_DIRS", "D:\\Filmy;E:\\Seriale"),
                ("PORT", "9000"),
            ])
        );
    }

    #[test]
    fn read_env_file_cudzyslowy_i_komentarze() {
        let (_d, f) = write_env(
            b"A=\"war tosc # nie komentarz\"\nB='jedn\\n'\nC=bez # komentarz\nexport D=eksport\nE=\"\"\nF = spacje \n\n  # wciety komentarz\nG=`tyl`\nH=\"a\\nb\"\n"
        );
        let m = map(read_env_file(&f).unwrap());
        assert_eq!(m["A"], "war tosc # nie komentarz");
        assert_eq!(m["B"], "jedn\\n");
        assert_eq!(m["C"], "bez");
        assert_eq!(m["D"], "eksport");
        assert_eq!(m["E"], "");
        assert_eq!(m["F"], "spacje");
        assert_eq!(m["G"], "tyl");
        assert_eq!(m["H"], "a\nb");
        assert_eq!(m.len(), 8);
    }

    #[test]
    fn read_env_file_zachowuje_backslashe_i_ostatni_duplikat_wygrywa() {
        let (_d, f) = write_env(b"MEDIA_DIRS=D:\\Filmy;E:\\Seriale\nPORT=1\nPORT=2\n");
        let v = read_env_file(&f).unwrap();
        assert_eq!(v.len(), 2);
        let m = map(v);
        assert_eq!(m["MEDIA_DIRS"], "D:\\Filmy;E:\\Seriale");
        assert_eq!(m["PORT"], "2");
    }

    #[test]
    fn read_env_file_nieistniejacy_plik_to_blad() {
        let dir = tempfile::tempdir().unwrap();
        assert!(read_env_file(&dir.path().join("brak")).is_err());
    }

    #[test]
    fn read_env_file_nie_panikuje_na_nieutf8() {
        let (_d, f) = write_env(b"A=\xff\xfe\nB=ok\n");
        let m = map(read_env_file(&f).unwrap());
        assert_eq!(m["B"], "ok");
    }

    #[test]
    fn apply_env_nie_nadpisuje() {
        let mut e = env(&[("PORT", "1")]);
        apply_env(
            &mut e,
            vec![
                ("PORT".into(), "2".into()),
                ("ADMIN_PASSWORD".into(), "x".into()),
            ],
        );
        assert_eq!(e, env(&[("PORT", "1"), ("ADMIN_PASSWORD", "x")]));
    }

    #[test]
    fn find_root_obok_exe_gdy_jest_public() {
        let exe = tempfile::tempdir().unwrap();
        let cwd = tempfile::tempdir().unwrap();
        std::fs::create_dir(exe.path().join("public")).unwrap();
        std::fs::create_dir(cwd.path().join("public")).unwrap();
        assert_eq!(find_root(exe.path(), cwd.path()), exe.path());
    }

    #[test]
    fn find_root_cwd_gdy_brak_public_przy_exe() {
        let exe = tempfile::tempdir().unwrap();
        let cwd = tempfile::tempdir().unwrap();
        assert_eq!(find_root(exe.path(), cwd.path()), cwd.path());
        // plik o nazwie public to nie katalog
        std::fs::write(exe.path().join("public"), b"x").unwrap();
        assert_eq!(find_root(exe.path(), cwd.path()), cwd.path());
    }
}
