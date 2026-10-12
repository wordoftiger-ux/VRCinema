//! The app's language (spec section 6): the `language` setting changes every text at once, the
//! log is kept as messages and read in the language of the request, the file stays English.

use super::*;
use crate::i18n::{Lang, Msg};

/// `path` read with the window's session and `X-VRCinema-Lang: lang` (none: the app's language).
async fn get_in(app: &App, cookie: &str, path: &str, lang: Option<&str>) -> Value {
    let mut req = app.http.get(app.url(path)).header("cookie", cookie);
    if let Some(lang) = lang {
        req = req.header("X-VRCinema-Lang", lang);
    }
    let res = req.send().await.unwrap();
    assert_eq!(res.status(), StatusCode::OK, "{path}");
    res.json().await.unwrap()
}

#[tokio::test]
async fn changing_the_language_applies_at_once_without_a_restart() {
    let app = App::start().await;
    let cookie = app.window_cookie().await;
    let mut changes = app.engine().language_changes();
    assert_eq!(
        app.get("/api/desktop/config", &cookie).await["language"],
        "pl"
    );
    let (status, res) = app.put_config(&cookie, json!({"port": 80})).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(res["error"], "Port musi być liczbą od 1024 do 65535.");

    let (status, res) = app.put_config(&cookie, json!({"language": "en"})).await;
    assert_eq!(status, StatusCode::OK, "{res}");
    assert_eq!(res["language"], "en");
    assert_eq!(res["restarted"], false);
    // At once: the very next answer, the saved file, the tray's watch and the engine.
    assert_eq!(app.saved()["language"], "en");
    assert_eq!(app.engine().language(), Lang::En);
    assert!(changes.has_changed().unwrap());
    assert_eq!(*changes.borrow_and_update(), Lang::En);
    let (status, res) = app.put_config(&cookie, json!({"port": 80})).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(
        res["error"],
        "The port must be a number from 1024 to 65535."
    );
    assert_eq!(res["code"], "invalid_port");
    // The server was not restarted for it, and the request's own language still wins.
    assert_eq!(app.engine().server_starts(), 1);
    let info = get_in(&app, &cookie, "/api/desktop/info", Some("pl")).await;
    assert_eq!(info["language"], "en", "the setting itself");
    assert_eq!(
        app.get("/api/desktop/config", &cookie).await["language"],
        "en"
    );

    // An unknown language changes nothing.
    let (status, res) = app.put_config(&cookie, json!({"language": "xx"})).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(res["code"], "invalid_language");
    assert_eq!(app.saved()["language"], "en");
    app.stop().await;
}

#[tokio::test]
async fn the_log_is_read_in_the_language_of_the_request_and_the_file_stays_english() {
    let app = App::start().await;
    let cookie = app.window_cookie().await;
    app.engine().log(Msg::new("log.program_stopped"));
    app.engine().log(
        Msg::new("log.tools_job_failed")
            .arg_msg("what", Msg::new("log.tools_job_install"))
            .arg_msg("error", Msg::error("busy")),
    );
    let lines = |v: &Value| -> Vec<String> {
        v["lines"]
            .as_array()
            .unwrap()
            .iter()
            .map(|l| l.as_str().unwrap().to_string())
            .collect()
    };
    let pl = lines(&get_in(&app, &cookie, "/api/desktop/log", Some("pl")).await);
    let en = lines(&get_in(&app, &cookie, "/api/desktop/log", Some("en")).await);
    let mine = lines(&get_in(&app, &cookie, "/api/desktop/log", None).await);
    assert_eq!(pl.len(), en.len());
    assert!(
        pl.iter().any(|l| l.ends_with("] Program zatrzymany.")),
        "{pl:?}"
    );
    assert!(
        en.iter().any(|l| l.ends_with("] Program stopped.")),
        "{en:?}"
    );
    assert!(
        pl.last()
            .unwrap()
            .contains("Narzędzia (pobieranie): błąd — Trwa już inna operacja"),
        "{pl:?}"
    );
    assert!(
        en.last()
            .unwrap()
            .contains("Tools (downloading): error — Another permanent link"),
        "{en:?}"
    );
    // Without a language of its own the request gets the app's (Polish in these tests) ...
    assert_eq!(mine, pl);
    // ... and a switch of the app's language changes it with no new log entry needed.
    app.put_config(&cookie, json!({"language": "en"})).await;
    assert_eq!(
        lines(&get_in(&app, &cookie, "/api/desktop/log", None).await),
        {
            let mut expected = en.clone();
            // The settings change itself adds nothing to the log.
            expected.truncate(pl.len());
            expected
        }
    );
    // The file on disk is the English reading, whatever language the window reads in.
    let file = std::fs::read_to_string(app.home.join("logs").join("vrcinema.log")).unwrap();
    assert!(file.contains("] Program stopped."), "{file}");
    assert!(
        file.contains("Tools (downloading): error — Another permanent link"),
        "{file}"
    );
    assert!(!file.contains("Program zatrzymany"), "{file}");
    assert!(!file.contains("Narzędzia"), "{file}");
    app.stop().await;
}

#[tokio::test]
async fn a_failed_tools_job_is_told_in_the_language_of_the_request() {
    let app = App::start().await;
    let cookie = app.window_cookie().await;
    let (status, res) = app
        .call("POST", "/api/desktop/tools/install", Some(&cookie), None)
        .await;
    assert_eq!(status, StatusCode::ACCEPTED, "{res}");
    // Nothing listens at the tools' address: the first download fails.
    eventually(30, "the failed job", || async {
        app.get("/api/desktop/tools/progress", &cookie).await["done"] == true
    })
    .await;
    let pl = get_in(&app, &cookie, "/api/desktop/tools/progress", Some("pl")).await;
    let en = get_in(&app, &cookie, "/api/desktop/tools/progress", Some("en")).await;
    let (pl, en) = (pl["error"].as_str().unwrap(), en["error"].as_str().unwrap());
    assert!(
        pl.starts_with("Pobieranie nie powiodło się: http://127.0.0.1:9/"),
        "{pl}"
    );
    assert!(
        en.starts_with("Download failed: http://127.0.0.1:9/"),
        "{en}"
    );
    // The log kept the failure as a message too: both readings, the file in English.
    let log_pl = get_in(&app, &cookie, "/api/desktop/log", Some("pl"))
        .await
        .to_string();
    let log_en = get_in(&app, &cookie, "/api/desktop/log", Some("en"))
        .await
        .to_string();
    assert!(
        log_pl.contains("Narzędzia (pobieranie): błąd — Pobieranie nie powiodło się"),
        "{log_pl}"
    );
    assert!(
        log_en.contains("Tools (downloading): error — Download failed"),
        "{log_en}"
    );
    app.stop().await;
}

#[tokio::test]
async fn why_the_channel_did_not_start_is_told_in_the_language_of_the_request() {
    let app = App::start().await;
    let cookie = app.window_cookie().await;
    // The channel start failure is kept as a message, not as the text of one language.
    *lock(&app.engine().core.channel_error) =
        Some(Msg::error("server_start_timeout").arg("seconds", 30));
    let pl = get_in(&app, &cookie, "/api/desktop/info", Some("pl")).await;
    let en = get_in(&app, &cookie, "/api/desktop/info", Some("en")).await;
    assert_eq!(pl["channelError"], "Serwer nie wystartował w ciągu 30 s.");
    assert_eq!(en["channelError"], "The server did not start within 30 s.");
    app.stop().await;
}
