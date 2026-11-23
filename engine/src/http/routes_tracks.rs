//! The audio track and the subtitles of each film (spec `2026-11-22-viewer-and-tracks-design.md`
//! §4): the streams the engine found in it, the owner's choice and what the preferred
//! languages pick. Behind the session cookie, like the other owner routes.

use axum::Router;
use axum::extract::{Path as UrlPath, State};
use axum::http::StatusCode;
use axum::routing::post;
use serde_json::{Value, json};

use super::{ApiError, ApiResult, AppState, JsonBody, json_ok};
use crate::db::tracks::{ItemTracks, item_tracks, set_choice, tracks_of};
use crate::db::{ChannelSettings, QueueItem, QueueRepo};
use crate::engine::Command;
use crate::pipeline::tracks::{SUBTITLES_OFF, auto_audio, auto_subtitle, lang_list};

pub(super) fn router() -> Router<AppState> {
    Router::new().route(
        "/api/admin/channels/main/items/{id}/tracks",
        post(set_tracks),
    )
}

/// Highest stream number accepted (no film has more streams of one kind).
const MAX_TRACK: i64 = 99;

/// The panel's `tracks` of one item: `{audio, subtitles, chosenAudio, chosenSubtitle,
/// autoAudio, autoSubtitle}`; `null` while the film was never analysed and nothing is chosen.
pub(crate) fn tracks_json(t: &ItemTracks, settings: &ChannelSettings) -> Value {
    if t.streams.is_none() && t.audio_track.is_none() && t.subtitle_track.is_none() {
        return Value::Null;
    }
    let streams = t.streams.clone().unwrap_or_default();
    let adv = &settings.advanced;
    json!({
        "audio": streams.audio,
        "subtitles": streams.subtitles,
        "chosenAudio": t.audio_track,
        "chosenSubtitle": t.subtitle_track,
        "autoAudio": auto_audio(&streams.audio, &lang_list(&adv.preferred_audio_langs)),
        "autoSubtitle": auto_subtitle(
            &streams.subtitles,
            &lang_list(&adv.preferred_subtitle_langs)
        ),
    })
}

/// Adds `tracks` to the JSON of `items` (as listed, same order).
pub(crate) fn add_tracks(
    state: &AppState,
    settings: &ChannelSettings,
    items: &[QueueItem],
    json: &mut Value,
) {
    let ids: Vec<i64> = items.iter().map(|i| i.id).collect();
    let kept = state.with_db(|c| tracks_of(c, &ids)).unwrap_or_default();
    let Value::Array(rows) = json else { return };
    for (item, row) in items.iter().zip(rows.iter_mut()) {
        if let Value::Object(map) = row {
            let t = kept.get(&item.id).cloned().unwrap_or_default();
            map.insert("tracks".into(), tracks_json(&t, settings));
        }
    }
}

fn bad() -> ApiError {
    ApiError::bad_request("bad_tracks")
}

/// One field of the body: absent keeps the stored choice, `null` is automatic; otherwise a
/// stream number (`allow_off`: or `-1`, subtitles off).
fn track_field(
    body: &Value,
    key: &str,
    kept: Option<i64>,
    allow_off: bool,
) -> ApiResult<Option<i64>> {
    match body.get(key) {
        None => Ok(kept),
        Some(Value::Null) => Ok(None),
        Some(v) => {
            let n = v.as_i64().ok_or_else(bad)?;
            let off = allow_off && n == SUBTITLES_OFF;
            if off || (0..=MAX_TRACK).contains(&n) {
                Ok(Some(n))
            } else {
                Err(bad())
            }
        }
    }
}

/// `POST /api/admin/channels/main/items/{id}/tracks` `{"audio": n|null, "subtitle": n|null|-1}`:
/// the owner's audio stream and subtitle stream of the item (`null`: automatic; `-1`: no
/// subtitles; a key left out stays as it was). The film on air starts again from where it is.
/// Answers `{tracks, restart}` (`restart`: the film on air starts again).
async fn set_tracks(
    State(state): State<AppState>,
    UrlPath(id): UrlPath<String>,
    JsonBody(body): JsonBody,
) -> ApiResult {
    let no_item = || ApiError::coded(StatusCode::NOT_FOUND, "no_such_item", &[]);
    let id: i64 = id.trim().parse().map_err(|_| no_item())?;
    let item = state
        .with_db(|c| QueueRepo::new(c).get(id))?
        .filter(|i| i.channel_id == state.channel_id)
        .ok_or_else(no_item)?;
    if !body.is_object() {
        return Err(bad());
    }
    let kept = state.with_db(|c| item_tracks(c, item.id))?;
    let audio = track_field(&body, "audio", kept.audio_track, false)?;
    let subtitle = track_field(&body, "subtitle", kept.subtitle_track, true)?;
    state.with_db(|c| set_choice(c, item.id, audio, subtitle))?;
    let snapshot = state.snapshot();
    let control = &snapshot.state.control;
    let restart = control.item_id == Some(item.id) && !control.is_live && !snapshot.state.paused;
    state.send_command(Command::ApplyTracks { item_id: item.id });
    let settings = state.settings()?;
    let t = state.with_db(|c| item_tracks(c, item.id))?;
    Ok(json_ok(&json!({
        "tracks": tracks_json(&t, &settings),
        "restart": restart,
    })))
}
