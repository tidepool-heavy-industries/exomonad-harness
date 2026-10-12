//! Authenticated chat projection and form submission. Native owns continuation.
use super::AppState;
use crate::{
    model::RequestId,
    store::{StoreError, actor_output::ActorOutputOrigin},
};
use axum::{
    Json,
    extract::{Path, Query, State},
    http::{StatusCode, header},
    response::{IntoResponse, Response},
};
use serde::Deserialize;
#[derive(Deserialize)]
pub(super) struct ChatQuery {
    run: String,
    actor: u64,
    incarnation: u64,
    request: Option<String>,
    #[serde(default)]
    after: i64,
    limit: Option<usize>,
}
pub(super) async fn chat(State(state): State<AppState>, Query(q): Query<ChatQuery>) -> Response {
    let Some(store) = state.history_store else {
        return StatusCode::SERVICE_UNAVAILABLE.into_response();
    };
    let origin = ActorOutputOrigin {
        run: q.run,
        native_actor: q.actor,
        incarnation: q.incarnation,
    };
    let request = q.request.map(RequestId);
    match tokio::task::spawn_blocking(move || {
        store.chat_page(&origin, request.as_ref(), q.after, q.limit.unwrap_or(50))
    })
    .await
    {
        Ok(Ok(page)) => ([(header::CACHE_CONTROL, "no-store")], Json(page)).into_response(),
        Ok(Err(StoreError::MissingRequest(_))) => StatusCode::NOT_FOUND.into_response(),
        Ok(Err(StoreError::InvalidHistoryOffset | StoreError::InvalidActorOutput)) => {
            StatusCode::BAD_REQUEST.into_response()
        }
        _ => StatusCode::INTERNAL_SERVER_ERROR.into_response(),
    }
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct FormSubmission {
    origin: ActorOutputOrigin,
    mount_id: String,
    operation_id: String,
    draft: serde_json::Value,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct FormDismissal {
    origin: ActorOutputOrigin,
    mount_id: String,
    operation_id: String,
}
pub(super) async fn submit(
    State(state): State<AppState>,
    Json(input): Json<FormSubmission>,
) -> Response {
    let Some(store) = state.history_store else {
        return StatusCode::SERVICE_UNAVAILABLE.into_response();
    };
    response(
        tokio::task::spawn_blocking(move || {
            store.submit_actor_form(
                &input.origin,
                &input.mount_id,
                &input.operation_id,
                &input.draft,
            )
        })
        .await,
    )
}
pub(super) async fn dismiss(
    State(state): State<AppState>,
    Json(input): Json<FormDismissal>,
) -> Response {
    let Some(store) = state.history_store else {
        return StatusCode::SERVICE_UNAVAILABLE.into_response();
    };
    response(
        tokio::task::spawn_blocking(move || {
            store.dismiss_actor_form(&input.origin, &input.mount_id, &input.operation_id)
        })
        .await,
    )
}
fn response(
    result: std::result::Result<
        crate::store::Result<crate::store::forms::StoredActorForm>,
        tokio::task::JoinError,
    >,
) -> Response {
    match result {
        Ok(Ok(form)) => ([(header::CACHE_CONTROL, "no-store")], Json(form)).into_response(),
        Ok(Err(StoreError::InvalidForm | StoreError::InvalidActorOutput)) => {
            StatusCode::BAD_REQUEST.into_response()
        }
        Ok(Err(StoreError::FormUnavailable | StoreError::ConflictingFormOperation)) => {
            StatusCode::CONFLICT.into_response()
        }
        _ => StatusCode::INTERNAL_SERVER_ERROR.into_response(),
    }
}
pub(super) async fn media(State(state): State<AppState>, Path(hash): Path<String>) -> Response {
    let Some(store) = state.history_store else {
        return StatusCode::SERVICE_UNAVAILABLE.into_response();
    };
    match tokio::task::spawn_blocking(move || store.actor_media(&hash)).await {
        Ok(Ok(Some(media))) => (
            [
                (header::CONTENT_TYPE, media.mime),
                (
                    header::CACHE_CONTROL,
                    "private, max-age=31536000, immutable".into(),
                ),
                (header::X_CONTENT_TYPE_OPTIONS, "nosniff".into()),
            ],
            axum::body::Body::from_stream(futures_util::stream::unfold(
                (media.bytes, 0usize),
                |(bytes, offset)| async move {
                    if offset >= bytes.len() {
                        None
                    } else {
                        let end = (offset + 64 * 1024).min(bytes.len());
                        let chunk = bytes[offset..end].to_vec();
                        Some((Ok::<_, std::convert::Infallible>(chunk), (bytes, end)))
                    }
                },
            )),
        )
            .into_response(),
        Ok(Ok(None)) => StatusCode::NOT_FOUND.into_response(),
        Ok(Err(StoreError::InvalidMedia)) => StatusCode::BAD_REQUEST.into_response(),
        _ => StatusCode::INTERNAL_SERVER_ERROR.into_response(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        server::{BearerSecret, ServerConfig, server_with_config},
        store::actor_output::ActorOutputExecution,
        store::{Store, forms::*},
    };
    use serde_json::json;
    use std::{path::PathBuf, sync::Arc};
    struct Authority;
    impl ActorFormAuthority for Authority {
        fn validate_form(&self, _: &ActorFormOpen) -> std::result::Result<bool, String> {
            Ok(true)
        }
    }
    #[tokio::test]
    async fn chat_forms_and_retained_media_use_protected_routes_and_typed_conflicts() {
        let store = Arc::new(Store::memory().unwrap());
        let origin = ActorOutputOrigin {
            run: "run".into(),
            native_actor: 1,
            incarnation: 1,
        };
        let opening = ActorFormOpen {
            origin: origin.clone(),
            mount_id: "mount".into(),
            execution: ActorOutputExecution::ActorProgram,
            conversation: None,
            form: serde_json::from_value(
                json!({"version":1,"root":{"kind":"text","id":"f0","label":"Name","initial":null}}),
            )
            .unwrap(),
        };
        store.open_actor_form(&Authority, &opening).unwrap();
        let asset = store
            .retain_actor_media("image/png", b"\x89PNG\r\n\x1a\nretained")
            .unwrap();
        let secret = "chat-route-secret-with-enough-bytes";
        let (router, _control, _commands) = server_with_config(
            ServerConfig::new(PathBuf::new())
                .with_bearer_secret(BearerSecret::new(secret).unwrap())
                .with_history_store(store.clone()),
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
        let client = reqwest::Client::new();
        let base = format!("http://{address}/api");
        assert_eq!(
            client
                .get(format!("{base}/chat?run=run&actor=1&incarnation=1"))
                .send()
                .await
                .unwrap()
                .status(),
            StatusCode::UNAUTHORIZED
        );
        let page: serde_json::Value = client
            .get(format!("{base}/chat?run=run&actor=1&incarnation=1"))
            .bearer_auth(secret)
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
        assert_eq!(page["entries"][0]["kind"], "form");
        assert_eq!(page["entries"][0]["form"]["opening"]["mountId"], "mount");
        let body = json!({"origin":origin,"mountId":"mount","operationId":"submit","draft":{"f0":"answer"}});
        let submitted = client
            .post(format!("{base}/actor-form/submit"))
            .bearer_auth(secret)
            .json(&body)
            .send()
            .await
            .unwrap();
        assert_eq!(submitted.status(), StatusCode::OK);
        let submitted: serde_json::Value = submitted.json().await.unwrap();
        assert_eq!(submitted["state"], "submitted");
        assert_eq!(
            client
                .post(format!("{base}/actor-form/submit"))
                .bearer_auth(secret)
                .json(&body)
                .send()
                .await
                .unwrap()
                .status(),
            StatusCode::OK
        );
        let changed = json!({"origin":origin,"mountId":"mount","operationId":"submit","draft":{"f0":"changed"}});
        assert_eq!(
            client
                .post(format!("{base}/actor-form/submit"))
                .bearer_auth(secret)
                .json(&changed)
                .send()
                .await
                .unwrap()
                .status(),
            StatusCode::CONFLICT
        );
        assert_eq!(
            client
                .get(format!("{base}/actor-media/{}", asset.hash))
                .send()
                .await
                .unwrap()
                .status(),
            StatusCode::UNAUTHORIZED
        );
        let retained = client
            .get(format!("{base}/actor-media/{}", asset.hash))
            .bearer_auth(secret)
            .send()
            .await
            .unwrap();
        assert_eq!(retained.headers()[header::CONTENT_TYPE], "image/png");
        assert_eq!(
            retained.headers()[header::X_CONTENT_TYPE_OPTIONS],
            "nosniff"
        );
        assert_eq!(retained.bytes().await.unwrap().as_ref(), asset.bytes);
        server.abort();
    }
}
