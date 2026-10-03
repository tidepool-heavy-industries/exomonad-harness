//! Protected, bounded request history reads for browser inspection.
//! Oversized Items retain their Store hash; full large-detail retrieval is
//! deferred until an artifact route exists.

use super::AppState;
use crate::{
    model::RequestId,
    store::{StoreError, history::MAX_HISTORY_ITEMS},
};
use axum::{
    Json,
    extract::{Path, Query, State},
    http::{StatusCode, header},
    response::{IntoResponse, Response},
};
use serde::Deserialize;

#[derive(Default, Deserialize)]
pub(super) struct HistoryQuery {
    #[serde(default)]
    offset: u64,
    limit: Option<usize>,
}

pub(super) async fn request_history(
    State(state): State<AppState>,
    Path(request_id): Path<String>,
    Query(query): Query<HistoryQuery>,
) -> Response {
    let Some(store) = state.history_store else {
        return (
            StatusCode::SERVICE_UNAVAILABLE,
            "history Store is unavailable",
        )
            .into_response();
    };
    let limit = query.limit.unwrap_or(50);
    if limit == 0 || limit > MAX_HISTORY_ITEMS || i64::try_from(query.offset).is_err() {
        return (StatusCode::BAD_REQUEST, "invalid history page bounds").into_response();
    }
    let page = tokio::task::spawn_blocking(move || {
        store.history_page(&RequestId(request_id), query.offset, limit)
    })
    .await;
    let page = match page {
        Ok(Ok(page)) => page,
        Ok(Err(StoreError::MissingRequest(_))) => return StatusCode::NOT_FOUND.into_response(),
        Ok(Err(StoreError::InvalidHistoryOffset)) => {
            return StatusCode::BAD_REQUEST.into_response();
        }
        Ok(Err(_)) | Err(_) => return StatusCode::INTERNAL_SERVER_ERROR.into_response(),
    };
    let status = if page.items.is_empty() && page.oversized_item.is_some() {
        StatusCode::PAYLOAD_TOO_LARGE
    } else {
        StatusCode::OK
    };
    (status, [(header::CACHE_CONTROL, "no-store")], Json(page)).into_response()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        item::Item,
        server::{BearerSecret, ServerConfig, server_with_config},
        store::Store,
    };
    use serde_json::json;
    use std::{path::PathBuf, sync::Arc};

    const SECRET: &str = "history-route-secret-with-enough-bytes";

    #[tokio::test]
    async fn protected_history_reports_pages_missing_store_and_oversized_items() {
        let store = Arc::new(Store::memory().unwrap());
        let request = RequestId("history-route".into());
        store.create_request(&request, None, "/root").unwrap();
        let first = Item(json!({"type":"message","role":"user","content":"first"}));
        let large = Item(json!({"type":"message","role":"user","content":"z".repeat(256 * 1024)}));
        store
            .append_items(&request, &[first.clone(), large])
            .unwrap();
        let config = ServerConfig::new(PathBuf::new())
            .with_bearer_secret(BearerSecret::new(SECRET).unwrap())
            .with_history_store(store);
        let (router, _control, _commands) = server_with_config(config);
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
        let client = reqwest::Client::new();
        let url = format!("http://{address}/api/history/{}", request.0);
        let denied = client.get(&url).send().await.unwrap();
        assert_eq!(denied.status(), StatusCode::UNAUTHORIZED);
        let page = client.get(&url).bearer_auth(SECRET).send().await.unwrap();
        assert_eq!(page.status(), StatusCode::OK);
        assert_eq!(page.headers()[header::CACHE_CONTROL], "no-store");
        let page: serde_json::Value = page.json().await.unwrap();
        assert_eq!(page["requestId"], "history-route");
        assert_eq!(page["items"][0]["item"], first.0);
        assert_eq!(page["nextOffset"], 1);
        assert_eq!(page["oversizedItem"]["skipOffset"], 2);
        assert_eq!(page["oversizedItem"]["hash"].as_str().unwrap().len(), 64);

        let oversized = client
            .get(format!("{url}?offset=1"))
            .bearer_auth(SECRET)
            .send()
            .await
            .unwrap();
        assert_eq!(oversized.status(), StatusCode::PAYLOAD_TOO_LARGE);
        let oversized: serde_json::Value = oversized.json().await.unwrap();
        assert_eq!(
            oversized["oversizedItem"]["hash"],
            page["oversizedItem"]["hash"]
        );
        assert_eq!(oversized["items"].as_array().unwrap().len(), 0);
        let missing = client
            .get(format!("http://{address}/api/history/missing"))
            .bearer_auth(SECRET)
            .send()
            .await
            .unwrap();
        assert_eq!(missing.status(), StatusCode::NOT_FOUND);
        let invalid = client
            .get(format!("{url}?limit=101"))
            .bearer_auth(SECRET)
            .send()
            .await
            .unwrap();
        assert_eq!(invalid.status(), StatusCode::BAD_REQUEST);
        server.abort();

        let no_store = ServerConfig::new(PathBuf::new())
            .with_bearer_secret(BearerSecret::new(SECRET).unwrap());
        let (router, _control, _commands) = server_with_config(no_store);
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
        let unavailable = client
            .get(format!("http://{address}/api/history/history-route"))
            .bearer_auth(SECRET)
            .send()
            .await
            .unwrap();
        assert_eq!(unavailable.status(), StatusCode::SERVICE_UNAVAILABLE);
        server.abort();
    }
}

#[derive(Deserialize)]
pub(super) struct ActorOutputQuery {
    run: String,
    actor: u64,
    incarnation: u64,
    #[serde(default)]
    after: i64,
    limit: Option<usize>,
}

pub(super) async fn actor_output_history(
    State(state): State<AppState>,
    Query(query): Query<ActorOutputQuery>,
) -> Response {
    let Some(store) = state.history_store else {
        return (StatusCode::SERVICE_UNAVAILABLE, "history Store is unavailable").into_response();
    };
    let limit = query.limit.unwrap_or(50);
    if query.after < 0 || limit == 0 || limit > MAX_HISTORY_ITEMS || query.run.len() > 1024 {
        return (StatusCode::BAD_REQUEST, "invalid history page bounds").into_response();
    }
    let origin = crate::store::actor_output::ActorOutputOrigin {
        run: query.run, native_actor: query.actor, incarnation: query.incarnation,
    };
    let result = tokio::task::spawn_blocking(move || store.actor_output_page(&origin, query.after, limit)).await;
    match result {
        Ok(Ok(page)) => ([(header::CACHE_CONTROL, "no-store")], Json(page)).into_response(),
        Ok(Err(StoreError::InvalidHistoryOffset | StoreError::InvalidActorOutput)) => StatusCode::BAD_REQUEST.into_response(),
        _ => StatusCode::INTERNAL_SERVER_ERROR.into_response(),
    }
}
