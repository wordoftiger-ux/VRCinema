//! Viewer lists: several links at once and playlists from the request page (the owner's
//! setting `viewerLists`). Every pasted link and every playlist entry passes the checks of a
//! single request (`routes_public::post_request`): the link mode and the home-network
//! protection before any lookup or yt-dlp call, the request limiter (one hit per pasted link),
//! the name lookup, the video rules (length, live). A list adds at most `viewerListMax` videos
//! and never more than the viewer's free places (`maxActivePerRequester`).

use std::collections::HashSet;
use std::sync::PoisonError;
use std::time::Duration;

use axum::Router;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::routing::post;
use futures_util::StreamExt;
use serde_json::{Value, json};
use url::Url;

use super::routes_public::{check_viewer, claim_viewer};
use super::{ApiError, ApiResult, AppState, Ctx, JsonBody, ReqLang, json_ok};
use crate::db::{AddedBy, ChannelSettings, ItemStatus, LinkMode, ListRepo, NewItem, QueueRepo};
use crate::engine::Command;
use crate::i18n::{Lang, Msg};
use crate::resolver::MediaMeta;
use crate::rules::{
    LinkError, check_host_resolves_public, check_viewer_link_offline, parse_source_url,
    validate_meta,
};

/// Links read (name lookup + yt-dlp) at the same time.
const CONCURRENT_FETCHES: usize = 4;

pub(super) fn public_router() -> Router<AppState> {
    Router::new().route("/api/channels/main/request-list", post(post_list))
}

/// Behind the admin session (merged with the admin routes).
pub(super) fn admin_router() -> Router<AppState> {
    Router::new().route("/api/admin/channels/main/lists/{id}", post(decide_list))
}

/// The panel's `pendingLists`: lists with at least two items waiting for approval.
pub(super) fn pending_lists_json(state: &AppState) -> ApiResult<Value> {
    let lists = state.with_db(|c| ListRepo::new(c).pending(state.channel_id, 2))?;
    Ok(Value::Array(
        lists
            .into_iter()
            .map(|l| json!({ "id": l.id, "title": l.title, "itemIds": l.item_ids }))
            .collect(),
    ))
}

/// Why a link of the list is refused, as the single request says it.
fn link_msg(e: LinkError, settings: &ChannelSettings) -> Msg {
    match e {
        LinkError::DomainNotAllowed if settings.link_mode == LinkMode::Blocklist => {
            Msg::error("domain_blocked")
        }
        LinkError::DomainNotAllowed => {
            Msg::error("domain_not_allowed").arg("domains", settings.allowed_domains.join(", "))
        }
        other => Msg::error(other.code()),
    }
}

/// A pasted line as an address: http(s) only (`local:` is no address at all).
fn parse_line(text: &str) -> Result<Url, Msg> {
    if let Some(url) = parse_source_url(text) {
        return Ok(url);
    }
    match Url::parse(text.trim()) {
        Ok(url) if url.scheme() != "local" => Err(Msg::error(LinkError::BadScheme.code())),
        _ => Err(Msg::error("invalid_link")),
    }
}

/// The non-empty lines of `urls` (an array of strings); none is an invalid link.
fn list_lines(body: &Value) -> ApiResult<Vec<String>> {
    let invalid = || ApiError::bad_request("invalid_link");
    let lines = body
        .get("urls")
        .and_then(Value::as_array)
        .ok_or_else(invalid)?
        .iter()
        .map(|v| v.as_str().map(|s| s.trim().to_string()).ok_or_else(invalid))
        .collect::<ApiResult<Vec<_>>>()?;
    // The page sends what the viewer typed, one line per entry; blank ones mean nothing.
    let lines: Vec<String> = lines.into_iter().filter(|l| !l.is_empty()).collect();
    if lines.is_empty() {
        return Err(invalid());
    }
    Ok(lines)
}

/// One row of the answer: a video added, or a line / entry left out with the reason.
struct Row {
    line: usize,
    title: Option<String>,
    skipped: Option<Msg>,
}

/// A video that passed every check.
struct Accepted {
    url: String,
    title: String,
    duration_s: Option<f64>,
}

/// A row for every video that passed the checks but has no place left in the list.
struct Budget {
    left: usize,
    /// The reason for what is left over: the list's own limit or the viewer's free places.
    over: Msg,
}

impl Budget {
    fn take(&mut self) -> Result<(), Msg> {
        if self.left == 0 {
            return Err(self.over.clone());
        }
        self.left -= 1;
        Ok(())
    }
}

async fn post_list(
    State(state): State<AppState>,
    ctx: Ctx,
    ReqLang(lang): ReqLang,
    JsonBody(body): JsonBody,
) -> ApiResult {
    let settings = state.settings()?;
    if !settings.viewer_lists {
        return Err(ApiError::coded(
            StatusCode::FORBIDDEN,
            "viewer_lists_off",
            &[],
        ));
    }
    let lines = list_lines(&body)?;
    let viewer = check_viewer(&state, &body)?;
    let ip = ctx.ip;
    let active = state.with_db(|c| QueueRepo::new(c).count_active_by_ip(state.channel_id, &ip))?;
    let max_active = i64::from(settings.max_active_per_requester);
    if active >= max_active {
        return Err(
            ApiError::coded(StatusCode::TOO_MANY_REQUESTS, "too_many_active", &[])
                .with_count(max_active),
        );
    }
    let list_max = settings.viewer_list_max.max(1) as usize;
    let free = usize::try_from(max_active - active).unwrap_or(usize::MAX);
    let mut budget = Budget {
        left: list_max.min(free),
        over: if free < list_max {
            Msg::error("too_many_active").count(max_active)
        } else {
            Msg::error("list_full")
        },
    };

    // 1. Without the network, in line order: the address, the link mode and the home network,
    //    duplicates, the list's length, then the limiter (one hit per link that will be read).
    let mut rows: Vec<Row> = Vec::new();
    let mut to_fetch: Vec<(usize, Url)> = Vec::new();
    let mut seen: HashSet<String> = HashSet::new();
    let mut limited = false;
    for (i, text) in lines.iter().enumerate() {
        let line = i + 1;
        let mut skip = |msg: Msg| {
            rows.push(Row {
                line,
                title: None,
                skipped: Some(msg),
            });
        };
        if i >= list_max {
            skip(Msg::error("list_full"));
            continue;
        }
        let url = match parse_line(text) {
            Ok(url) => url,
            Err(msg) => {
                skip(msg);
                continue;
            }
        };
        if !seen.insert(url.to_string()) {
            skip(Msg::error("duplicate_link"));
            continue;
        }
        if let Err(e) = check_viewer_link_offline(&url, &settings) {
            skip(link_msg(e, &settings));
            continue;
        }
        if !limited {
            let mut limiter = state
                .request_limiter
                .lock()
                .unwrap_or_else(PoisonError::into_inner);
            limiter.set_limits(
                settings.request_limit_count,
                Duration::from_secs(settings.request_limit_window_s),
            );
            limited = !limiter.hit(&ip);
        }
        if limited {
            skip(Msg::error("too_many_requests"));
            continue;
        }
        to_fetch.push((line, url));
    }

    // 2. The name lookup, then yt-dlp (a playlist read up to the list's size), a few at a time.
    let fetched: Vec<(usize, Url, Result<MediaMeta, Msg>)> = futures_util::stream::iter(to_fetch)
        .map(|(line, url)| {
            let state = state.clone();
            let settings = settings.clone();
            async move {
                let meta =
                    match check_host_resolves_public(&url, state.host_resolver.as_ref()).await {
                        Err(e) => Err(link_msg(e, &settings)),
                        Ok(()) => state
                            .resolver
                            .fetch_meta_capped(url.as_str(), list_max)
                            .await
                            .map_err(|e| Msg::error("fetch_failed").arg_msg("reason", e.0)),
                    };
                (line, url, meta)
            }
        })
        .buffered(CONCURRENT_FETCHES)
        .collect()
        .await;

    // 3. In line order: the video rules, each playlist entry checked like a pasted link, and
    //    the places left.
    let mut accepted: Vec<Accepted> = Vec::new();
    let playlists = fetched
        .iter()
        .filter(|(_, _, m)| m.as_ref().is_ok_and(|m| m.is_playlist))
        .count();
    let mut list_title = None;
    for (line, url, meta) in fetched {
        let meta = match meta {
            Ok(meta) => meta,
            Err(msg) => {
                rows.push(Row {
                    line,
                    title: None,
                    skipped: Some(msg),
                });
                continue;
            }
        };
        if !meta.is_playlist {
            let skipped =
                validate_meta(&meta, &settings, false).map_or_else(|| budget.take().err(), Some);
            if skipped.is_none() {
                accepted.push(Accepted {
                    url: url.to_string(),
                    title: meta.title.clone(),
                    duration_s: meta.duration_s,
                });
            }
            rows.push(Row {
                line,
                title: Some(meta.title),
                skipped,
            });
            continue;
        }
        if playlists == 1 {
            list_title = Some(meta.title.clone());
        }
        let entries = meta.entries.unwrap_or_default();
        if entries.is_empty() {
            rows.push(Row {
                line,
                title: Some(meta.title),
                skipped: Some(Msg::error("empty_playlist")),
            });
            continue;
        }
        // Without the network first, then every remaining name at the same time.
        let checked: Vec<(String, Option<f64>, Result<Url, Msg>)> = entries
            .into_iter()
            .map(|e| {
                let entry_meta = MediaMeta {
                    title: e.title.clone(),
                    duration_s: e.duration_s,
                    is_live: e.is_live,
                    is_playlist: false,
                    entries: None,
                };
                let url = parse_source_url(&e.url)
                    .ok_or_else(|| Msg::error("invalid_link"))
                    .and_then(|url| {
                        check_viewer_link_offline(&url, &settings)
                            .map_err(|err| link_msg(err, &settings))?;
                        match validate_meta(&entry_meta, &settings, false) {
                            Some(reason) => Err(reason),
                            None => Ok(url),
                        }
                    });
                (e.title, e.duration_s, url)
            })
            .collect();
        let resolver = state.host_resolver.as_ref();
        let rules = &settings;
        let looked_up = futures_util::future::join_all(checked.into_iter().map(
            |(title, duration_s, url)| async move {
                let url = match url {
                    Ok(url) => check_host_resolves_public(&url, resolver)
                        .await
                        .map(|()| url)
                        .map_err(|e| link_msg(e, rules)),
                    Err(msg) => Err(msg),
                };
                (title, duration_s, url)
            },
        ))
        .await;
        for (title, duration_s, url) in looked_up {
            let skipped = match url {
                Err(msg) => Some(msg),
                Ok(url) if !seen.insert(url.to_string()) => Some(Msg::error("duplicate_link")),
                Ok(url) => match budget.take() {
                    Err(msg) => Some(msg),
                    Ok(()) => {
                        accepted.push(Accepted {
                            url: url.to_string(),
                            title: title.clone(),
                            duration_s,
                        });
                        None
                    }
                },
            };
            rows.push(Row {
                line,
                title: Some(title),
                skipped,
            });
        }
    }
    rows.sort_by_key(|r| r.line);

    let status = if settings.moderation {
        ItemStatus::PendingApproval
    } else {
        ItemStatus::Queued
    };
    let mut list_id = None;
    if !accepted.is_empty() {
        claim_viewer(&state, &viewer)?;
        let channel_id = state.channel_id;
        list_id = Some(state.with_db(|c| {
            let q = QueueRepo::new(c);
            let mut ids = Vec::with_capacity(accepted.len());
            for a in &accepted {
                ids.push(
                    q.add(&NewItem {
                        channel_id,
                        source_url: a.url.clone(),
                        title: a.title.clone(),
                        duration_s: a.duration_s,
                        is_live: false,
                        status,
                        added_by: AddedBy::Public,
                        requester_name: viewer.nick.clone(),
                        requester_ip: Some(ip.clone()),
                        requester_key_hash: viewer.key_hash.clone(),
                    })?
                    .id,
                );
            }
            ListRepo::new(c).create(channel_id, list_title.as_deref(), &ids)
        })?);
    }
    let skipped = rows.iter().filter(|r| r.skipped.is_some()).count();
    (state.log)(
        Msg::new("log.request_list_received")
            .arg("viewer", state.viewer_tag(&ip))
            .arg("added", accepted.len())
            .arg("skipped", skipped)
            .arg_msg("status", status.msg()),
    );
    Ok(json_ok(&json!({
        "status": status.as_str(),
        "added": accepted.len(),
        "skipped": skipped,
        "listId": list_id,
        // The icon of the viewer's device, as a single request's item carries it.
        "requesterBadge": super::requester_badge(AddedBy::Public, viewer.key_hash.as_deref()),
        "results": rows.iter().map(|r| row_json(r, status, lang)).collect::<Vec<_>>(),
    })))
}

fn row_json(row: &Row, status: ItemStatus, lang: Lang) -> Value {
    match &row.skipped {
        None => json!({ "line": row.line, "title": row.title, "status": status.as_str() }),
        Some(msg) => json!({
            "line": row.line,
            "title": row.title,
            "status": "skipped",
            "code": msg.code(),
            "reason": msg.render(lang),
        }),
    }
}

/// `{"action": "approve" | "reject"}` for every item of list `id` still waiting for approval.
/// Answers `{changed}`; 404 for an unknown list, 409 `not_pending` when none waits any more.
async fn decide_list(
    State(state): State<AppState>,
    Path(id): Path<String>,
    JsonBody(body): JsonBody,
) -> ApiResult {
    let no_such_list = || ApiError::coded(StatusCode::NOT_FOUND, "no_such_list", &[]);
    let id: i64 = id.trim().parse().map_err(|_| no_such_list())?;
    let status = match body.get("action").and_then(Value::as_str) {
        Some("approve") => ItemStatus::Queued,
        Some("reject") => ItemStatus::Rejected,
        _ => return Err(ApiError::bad_request("action_or_position")),
    };
    let channel_id = state.channel_id;
    let changed = state.with_db(|c| {
        let lists = ListRepo::new(c);
        if !lists.exists(channel_id, id)? {
            return Ok(None);
        }
        let items = lists.pending_items(channel_id, id)?;
        let q = QueueRepo::new(c);
        for item in &items {
            q.set_status(*item, status, None)?;
        }
        Ok(Some(items.len()))
    })?;
    match changed {
        None => Err(no_such_list()),
        Some(0) => Err(ApiError::coded(StatusCode::CONFLICT, "not_pending", &[])),
        Some(n) => {
            state.send_command(Command::QueueChanged);
            Ok(json_ok(&json!({ "changed": n })))
        }
    }
}
