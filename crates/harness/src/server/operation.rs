//! Exact operation inspection shares the embedding's native status reader.
use super::{AppState, ServerControl};
use crate::{embedding::HostIdentity, model::OperationId, operation_observation::*};
use axum::{
    Json,
    extract::State,
    http::{StatusCode, header},
    response::{IntoResponse, Response},
};
use serde::Deserialize;
use std::{future::Future, pin::Pin, sync::Arc};

#[derive(Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OperationInspection {
    pub observer: HostIdentity,
    pub operation: OperationId,
}

pub type OperationObserver = Arc<
    dyn Fn(
            OperationInspection,
        ) -> Pin<
            Box<
                dyn Future<Output = Result<OperationObservation, OperationObservationError>> + Send,
            >,
        > + Send
        + Sync,
>;

impl ServerControl {
    pub fn install_operation_observer(
        &self,
        observer: OperationObserver,
    ) -> Result<(), &'static str> {
        self.operation_observer
            .set(observer)
            .map_err(|_| "operation observation owner is already installed")
    }
}

pub(super) async fn inspect(
    State(state): State<AppState>,
    Json(input): Json<OperationInspection>,
) -> Response {
    let crate::model::ConversationIdentity::Embedded {
        run,
        actor,
        incarnation,
    } = &input.operation.origin
    else {
        return StatusCode::BAD_REQUEST.into_response();
    };
    if [
        &input.observer.run,
        &input.observer.actor.0,
        &input.observer.incarnation,
        run,
        &actor.0,
        incarnation,
        &input.operation.request.0,
        &input.operation.call.0,
    ]
    .iter()
    .any(|field| field.is_empty() || field.len() > 1024)
    {
        return StatusCode::BAD_REQUEST.into_response();
    }
    let Some(observer) = state.operation_observer.get() else {
        return (
            StatusCode::SERVICE_UNAVAILABLE,
            "operation observation owner is unavailable",
        )
            .into_response();
    };
    match observer(input).await {
        Ok(observation) => {
            ([(header::CACHE_CONTROL, "no-store")], Json(observation)).into_response()
        }
        Err(OperationObservationError::Unauthorized) => StatusCode::FORBIDDEN.into_response(),
        Err(OperationObservationError::Unavailable(_)) => {
            StatusCode::SERVICE_UNAVAILABLE.into_response()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        item::Item,
        model::{AgentPath, CallId, RequestId},
        server::{BearerSecret, ServerConfig, server_with_config},
        store::Store,
    };
    use serde_json::json;

    #[tokio::test]
    async fn protected_operation_read_uses_current_owner_without_admission_or_events() {
        let secret = "operation-route-secret-at-least-32-bytes";
        let store = Arc::new(Store::memory().unwrap());
        let host = HostIdentity {
            run: "route".into(),
            actor: AgentPath("/root".into()),
            incarnation: "1".into(),
        };
        store.bind_embedded_actor(&host, None).unwrap();
        let request = RequestId("route-request".into());
        store
            .write_request(
                &request,
                None,
                "/root",
                &[Item(
                    json!({"type":"function_call","call_id":"call","name":"read","arguments":"{}"}),
                )],
                Default::default(),
            )
            .unwrap();
        let operation = store.claim(&CallId("call".into()), &request).unwrap();
        assert!(
            store
                .advance_agent_head(&host.actor, None, Some(&request))
                .unwrap()
        );
        let (router, control, mut commands) = server_with_config(
            ServerConfig::new(Default::default())
                .with_bearer_secret(BearerSecret::new(secret).unwrap()),
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            axum::serve(
                listener,
                router.into_make_service_with_connect_info::<std::net::SocketAddr>(),
            )
            .await
            .unwrap()
        });
        let client = reqwest::Client::new();
        let endpoint = format!("http://{address}/api/operation");
        let input = json!({"observer":host,"operation":operation});
        assert_eq!(
            client
                .post(&endpoint)
                .json(&input)
                .send()
                .await
                .unwrap()
                .status(),
            StatusCode::UNAUTHORIZED
        );
        assert_eq!(
            client
                .post(&endpoint)
                .bearer_auth(secret)
                .json(&input)
                .send()
                .await
                .unwrap()
                .status(),
            StatusCode::SERVICE_UNAVAILABLE
        );
        let observed = store.clone();
        control
            .install_operation_observer(Arc::new(move |input| {
                let store = observed.clone();
                Box::pin(async move {
                    store
                        .observe_embedded_operation(&input.observer, &input.operation)
                        .map_err(|_| OperationObservationError::Unauthorized)
                })
            }))
            .unwrap();
        let before = store.claims_for_operation(&operation).unwrap();
        let response = client
            .post(&endpoint)
            .bearer_auth(secret)
            .json(&input)
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(response.headers()[header::CACHE_CONTROL], "no-store");
        let result: OperationObservation = response.json().await.unwrap();
        assert_eq!(result.operation, operation);
        assert_eq!(result.settlement, OperationSettlement::Pending);
        assert_eq!(result.delivery, OperationDelivery::NotInCurrentHistory);
        let mut foreign = input.clone();
        foreign["observer"]["incarnation"] = json!("other");
        assert_eq!(
            client
                .post(&endpoint)
                .bearer_auth(secret)
                .json(&foreign)
                .send()
                .await
                .unwrap()
                .status(),
            StatusCode::FORBIDDEN
        );
        assert_eq!(store.claims_for_operation(&operation).unwrap(), before);
        assert!(
            commands.try_recv().is_err(),
            "inspection must not enqueue a host command"
        );
        server.abort();
        let _ = server.await;
    }
}
