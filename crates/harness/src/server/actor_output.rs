//! Browser expansion invokes the native actor owner; Store history remains canonical.
use super::{AppState, ServerControl};
use crate::store::actor_output::ActorOutputOrigin;
use axum::{
    Json,
    extract::State,
    http::StatusCode,
    response::{IntoResponse, Response},
};
use serde::Deserialize;
use std::{future::Future, pin::Pin, sync::Arc};

#[derive(Clone, Debug, Deserialize)]
#[serde(from = "super::browser_contract::ActorDisplayExpansion")]
pub struct ActorDisplayExpansion {
    pub origin: ActorOutputOrigin,
    pub display_slot: u64,
    pub key: u64,
}
impl From<super::browser_contract::ActorDisplayExpansion> for ActorDisplayExpansion {
    fn from(wire: super::browser_contract::ActorDisplayExpansion) -> Self {
        Self {
            origin: wire.origin.into(),
            display_slot: wire.display_slot.get(),
            key: wire.key.get(),
        }
    }
}
pub type ActorDisplayExpander = Arc<
    dyn Fn(ActorDisplayExpansion) -> Pin<Box<dyn Future<Output = Result<(), String>> + Send>>
        + Send
        + Sync,
>;

impl ServerControl {
    pub fn install_actor_display_expander(
        &self,
        expand: ActorDisplayExpander,
    ) -> Result<(), &'static str> {
        self.actor_display_expander
            .set(expand)
            .map_err(|_| "actor display expansion owner is already installed")
    }
}

pub(super) async fn expand(
    State(state): State<AppState>,
    Json(input): Json<ActorDisplayExpansion>,
) -> Response {
    if input.origin.run.is_empty()
        || input.origin.run.len() > 1024
        || input.display_slot == 0
        || input.key == 0
        || [
            input.origin.native_actor,
            input.origin.incarnation,
            input.display_slot,
            input.key,
        ]
        .iter()
        .any(|value| *value > i64::MAX as u64)
    {
        return (StatusCode::BAD_REQUEST, "invalid display authority").into_response();
    }
    let Some(expand) = state.actor_display_expander.get() else {
        return (
            StatusCode::SERVICE_UNAVAILABLE,
            "native display owner is unavailable",
        )
            .into_response();
    };
    match expand(input).await {
        Ok(()) => (
            StatusCode::NO_CONTENT,
            [(axum::http::header::CACHE_CONTROL, "no-store")],
        )
            .into_response(),
        Err(error) => (StatusCode::CONFLICT, error).into_response(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        server::{BearerSecret, ServerConfig, server_with_config},
        store::{Store, actor_output::*},
    };
    use serde_json::json;
    use std::{
        path::PathBuf,
        sync::atomic::{AtomicUsize, Ordering},
    };

    struct Authorized;
    impl ActorOutputAuthority for Authorized {
        fn validate_output(&self, _: &ActorOutputEmission) -> Result<bool, String> {
            Ok(true)
        }
    }

    #[tokio::test]
    async fn committed_actor_output_has_one_projection_and_protected_history_and_expansion() {
        let store = Arc::new(Store::memory().unwrap());
        let origin = ActorOutputOrigin {
            run: "run".into(),
            native_actor: 9_007_199_254_740_993,
            incarnation: 9_007_199_254_740_995,
        };
        let emission = ActorOutputEmission {
            origin: origin.clone(),
            id: ActorOutputId {
                display_slot: 9_007_199_254_740_997,
                page_ordinal: 1,
            },
            execution: ActorOutputExecution::ActorProgram,
            conversation: None,
            page: ActorDisplayPage {
                text: "preview".into(),
                expansions: vec![(9_007_199_254_740_999, "field".into())],
                unavailable: false,
            },
        };
        let committed = store.append_actor_output(&Authorized, &emission).unwrap();
        let secret = "actor-output-secret-with-enough-bytes";
        let (router, control, _commands) = server_with_config(
            ServerConfig::new(PathBuf::new())
                .with_history_store(store.clone())
                .with_bearer_secret(BearerSecret::new(secret).unwrap()),
        );
        let mut events = control.events.subscribe();
        control.publish_actor_output(committed.output());
        control.publish_actor_output(committed.output());
        let event = events.recv().await.unwrap();
        assert_eq!(event.event.kind(), "actor.output.committed");
        assert_eq!(
            super::event_value(&event)["reference"]["sequence"],
            committed.output().reference().sequence.to_string()
        );
        assert!(events.try_recv().is_err());
        assert_eq!(
            control
                .snapshot
                .read()
                .unwrap()
                .actor_output_revisions
                .len(),
            1
        );
        let calls = Arc::new(AtomicUsize::new(0));
        let called = calls.clone();
        control
            .install_actor_display_expander(Arc::new(move |_input| {
                called.fetch_add(1, Ordering::SeqCst);
                Box::pin(async move { Ok(()) })
            }))
            .unwrap();
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
        let client = reqwest::Client::new();
        let history = format!(
            "http://{address}/api/actor-output?run=run&actor={}&incarnation={}",
            origin.native_actor, origin.incarnation
        );
        assert_eq!(
            client.get(&history).send().await.unwrap().status(),
            StatusCode::UNAUTHORIZED
        );
        let page = client
            .get(&history)
            .bearer_auth(secret)
            .send()
            .await
            .unwrap();
        assert_eq!(page.status(), StatusCode::OK);
        assert_eq!(
            page.headers()[axum::http::header::CACHE_CONTROL],
            "no-store"
        );
        let page: serde_json::Value = page.json().await.unwrap();
        assert_eq!(page["outputs"][0], super::event_value(&event));
        assert!(page["outputs"][0].get("execution").is_none());
        assert!(page["outputs"][0].get("createdAtMs").is_none());
        assert_eq!(
            page["outputs"][0]["reference"],
            serde_json::to_value(super::browser_contract::ActorOutputReference::from(
                committed.output().reference()
            ))
            .unwrap()
        );
        let expand = format!("http://{address}/api/actor-output/expand");
        let body = serde_json::to_value(super::browser_contract::ActorDisplayExpansion {
            origin: (&origin).into(),
            display_slot: emission.id.display_slot.into(),
            key: emission.page.expansions[0].0.into(),
        })
        .unwrap();
        assert_eq!(
            client
                .post(&expand)
                .json(&body)
                .send()
                .await
                .unwrap()
                .status(),
            StatusCode::UNAUTHORIZED
        );
        let mut invalid = body.clone();
        invalid["key"] = json!("0");
        assert_eq!(
            client
                .post(&expand)
                .bearer_auth(secret)
                .json(&invalid)
                .send()
                .await
                .unwrap()
                .status(),
            StatusCode::BAD_REQUEST
        );
        assert_eq!(calls.load(Ordering::SeqCst), 0);
        let mut lossy = body.clone();
        lossy["displaySlot"] = json!(emission.id.display_slot);
        assert_eq!(
            client
                .post(&expand)
                .bearer_auth(secret)
                .json(&lossy)
                .send()
                .await
                .unwrap()
                .status(),
            StatusCode::UNPROCESSABLE_ENTITY
        );
        assert_eq!(calls.load(Ordering::SeqCst), 0);
        let tail = client
            .get(format!("{history}&after={}", i64::MAX))
            .bearer_auth(secret)
            .send()
            .await
            .unwrap();
        assert_eq!(tail.status(), StatusCode::OK);
        let tail: super::browser_contract::ActorOutputHistoryPage = tail.json().await.unwrap();
        assert_eq!(tail.origin.native_actor.get(), origin.native_actor);
        assert!(tail.outputs.is_empty());
        let expanded = client
            .post(&expand)
            .bearer_auth(secret)
            .json(&body)
            .send()
            .await
            .unwrap();
        assert_eq!(expanded.status(), StatusCode::NO_CONTENT);
        assert_eq!(calls.load(Ordering::SeqCst), 1);
        assert_eq!(
            store
                .actor_output_page(&origin, 0, 10)
                .unwrap()
                .outputs
                .len(),
            1
        );
        server.abort();
    }
}
