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
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ActorDisplayExpansion {
    pub origin: ActorOutputOrigin,
    pub display_slot: u64,
    pub key: u64,
}
pub type ActorDisplayExpander = Arc<
    dyn Fn(
            ActorDisplayExpansion,
        ) -> Pin<Box<dyn Future<Output = Result<serde_json::Value, String>> + Send>>
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
        Ok(response) => (
            [(axum::http::header::CACHE_CONTROL, "no-store")],
            Json(response),
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
            native_actor: 4,
            incarnation: 2,
        };
        let emission = ActorOutputEmission {
            origin: origin.clone(),
            id: ActorOutputId {
                display_slot: 1,
                page_ordinal: 1,
            },
            execution: ActorOutputExecution::ActorProgram,
            conversation: None,
            page: ActorDisplayPage {
                text: "preview".into(),
                expansions: vec![(1, "field".into())],
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
        assert_eq!(event.event, "actor.output.committed");
        assert_eq!(
            event.payload["reference"]["sequence"],
            committed.output().reference().sequence
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
            .install_actor_display_expander(Arc::new(move |input| {
                called.fetch_add(1, Ordering::SeqCst);
                Box::pin(async move { Ok(json!({"key":input.key})) })
            }))
            .unwrap();
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
        let client = reqwest::Client::new();
        let history = format!("http://{address}/api/actor-output?run=run&actor=4&incarnation=2");
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
        assert_eq!(
            page["outputs"][0]["reference"],
            serde_json::to_value(committed.output().reference()).unwrap()
        );
        let expand = format!("http://{address}/api/actor-output/expand");
        let body = json!({"origin":origin,"displaySlot":1,"key":1});
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
        let invalid = json!({"origin":origin,"displaySlot":1,"key":0});
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
        let expanded = client
            .post(&expand)
            .bearer_auth(secret)
            .json(&body)
            .send()
            .await
            .unwrap();
        assert_eq!(expanded.status(), StatusCode::OK);
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
