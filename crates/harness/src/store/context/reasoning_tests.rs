use super::*;
use crate::{
    context::{ContextChange, ContextPatch, ContextTransactionDraft, ContextTransactionStatus},
    model::{CallId, Effort},
    store::Usage,
    transport::{ResponsesProtocol, ResponsesRequest, ResponsesRoute, ResponsesTurn},
};

fn call(id: &str) -> Item {
    Item(json!({"type":"function_call","call_id":id,"name":"edit","arguments":"{}"}))
}

fn selection() -> crate::transport::PreparedTransportSelection {
    use crate::transport::{
        CredentialReference, InferenceLimits, NativeTransportAdmission, NativeTransportContract,
        PreparedTransportSelection,
    };
    PreparedTransportSelection::native(
        NativeTransportAdmission::rehydrate(NativeTransportContract {
            route: ResponsesRoute::DeepSeek,
            protocol: ResponsesProtocol::Standard,
            endpoint: crate::transport::client::DEEPSEEK_ENDPOINT.into(),
            credential: CredentialReference::Env {
                name: "OFFLINE_REASONING_FIXTURE".into(),
            },
            limits: InferenceLimits::deepseek_default(),
        })
        .unwrap(),
    )
}

fn target() -> ContextProjectionTarget {
    ContextProjectionTarget::for_transport("deepseek-flash", &selection())
}

fn record(store: &Store, head: &RequestId, items: Vec<Item>, route: ResponsesRoute) {
    if route != ResponsesRoute::DeepSeek {
        record_legacy(store, head, items, route);
        return;
    }
    let prepared = prepare_native(store, head);
    store
        .record_provider_turn(
            prepared.issuer(),
            &ResponsesTurn {
                response_id: head.0.clone(),
                items,
                usage: Default::default(),
            },
        )
        .unwrap();
}

fn prepare_native(store: &Store, head: &RequestId) -> crate::store::PreparedProviderRequest {
    store
        .prepare_provider_request(
            head,
            head,
            ResponsesRequest {
                input: vec![],
                instructions: "fixture".into(),
                tools: vec![].into(),
                tools_allowed: None,
                model: "deepseek-flash".into(),
                pinned_effort: Effort::High,
                session_id: "fixture".into(),
                prompt_cache_comparison: None,
            },
            &[],
            None,
            selection(),
            crate::store::ProviderRequestPurpose::Inference,
        )
        .unwrap()
}

fn record_legacy(store: &Store, head: &RequestId, items: Vec<Item>, route: ResponsesRoute) {
    store
        .record_replay_turn_for_target(
            head,
            &ResponsesRequest {
                input: vec![],
                instructions: "fixture".into(),
                tools: vec![].into(),
                tools_allowed: None,
                model: if route == ResponsesRoute::DeepSeek {
                    "deepseek-flash"
                } else {
                    "fixture-model"
                }
                .into(),
                pinned_effort: Effort::High,
                session_id: "fixture".into(),
                prompt_cache_comparison: None,
            },
            &ResponsesTurn {
                response_id: head.0.clone(),
                items,
                usage: Default::default(),
            },
            route,
            ResponsesProtocol::Standard,
        )
        .unwrap();
}

fn fixture(store: &Store, reasoning: Item, route: ResponsesRoute) -> (ContextSnapshot, Item) {
    fixture_with_source_contract(store, reasoning, route, true)
}

fn fixture_with_source_contract(
    store: &Store,
    reasoning: Item,
    route: ResponsesRoute,
    known: bool,
) -> (ContextSnapshot, Item) {
    let previous = RequestId("response".into());
    store
        .write_request(&previous, None, "/root", &[], Usage::default())
        .unwrap();
    let record_source = if known { record } else { record_legacy };
    record_source(
        store,
        &previous,
        vec![
            reasoning.clone(),
            Item(
                json!({"type":"message","role":"assistant","content":[{"type":"output_text","text":"answer"}]}),
            ),
        ],
        route,
    );
    let head = RequestId("editor".into());
    store
        .write_request(&head, Some(&previous), "/root", &[], Usage::default())
        .unwrap();
    record(store, &head, vec![call("edit")], route);
    let edit = store.claim(&CallId("edit".into()), &head).unwrap();
    store
        .initialize_context_target(
            &edit.origin,
            &if route == ResponsesRoute::DeepSeek {
                target()
            } else {
                ContextProjectionTarget {
                    model: "fixture-model".into(),
                    route,
                    protocol: ResponsesProtocol::Standard,
                    native_transport: None,
                }
            },
        )
        .unwrap();
    (store.begin_context(&edit, &head).unwrap(), reasoning)
}

fn fields(snapshot: &ContextSnapshot) -> Vec<ContextVisibleText> {
    snapshot
        .document
        .blocks
        .iter()
        .flat_map(|block| match block {
            ContextBlock::Native { texts, .. } => texts.clone(),
            _ => vec![],
        })
        .filter(|field| matches!(field.selector, ContextTextSelector::ReasoningText { .. }))
        .collect()
}

fn rewrite(
    store: &Store,
    snapshot: &ContextSnapshot,
    field: &ContextVisibleText,
    body: &str,
) -> ContextCommitReceipt {
    let patch = ContextPatch {
        changes: vec![ContextChange::ReplaceContextText {
            reference: field.reference.clone(),
            selector: field.selector.clone(),
            expected: crate::context::context_digest(field.text.as_bytes()),
            text: body.into(),
        }],
    };
    let draft = ContextTransactionDraft {
        patch,
        next_model: None,
        next_effort: None,
    };
    match store
        .commit_context_transaction_guarded(
            snapshot,
            &draft,
            &JobOutput::Completed(Ok(json!("editor done"))),
            &[],
            || false,
        )
        .unwrap()
    {
        ContextTransactionStatus::Committed(receipt) => ContextCommitReceipt {
            head: receipt.head,
            generation: receipt.generation,
            changed: receipt.changed,
            model: None,
        },
        other => panic!("unexpected {other:?}"),
    }
}

#[test]
fn reasoning_rewrite_preserves_original_and_projects_only_selected_part() {
    let store = Store::memory().unwrap();
    let original = Item(
        json!({"type":"reasoning","id":"r","status":"completed","summary":[],"content":[{"type":"reasoning_text","text":"first λ"},{"type":"reasoning_text","text":"second"}]}),
    );
    let (snapshot, original) = fixture(&store, original, ResponsesRoute::DeepSeek);
    let fields = fields(&snapshot);
    assert_eq!(
        fields
            .iter()
            .map(|f| (&*f.text, f.editable))
            .collect::<Vec<_>>(),
        vec![("first λ", true), ("second", true)]
    );
    let receipt = rewrite(&store, &snapshot, &fields[0], "authored revision");
    assert!(receipt.changed);
    assert_eq!(
        store
            .original_context_text(&snapshot, &fields[0].reference, &fields[0].selector)
            .unwrap()
            .unwrap(),
        "first λ"
    );
    let projected = store
        .context_request_state(&receipt.head, &snapshot.operation.origin)
        .unwrap();
    let actual = projected
        .history
        .iter()
        .find(|(_, _, item)| item.0["type"] == "reasoning")
        .unwrap();
    let mut expected = original.clone();
    expected.0["content"][0]["text"] = json!("authored revision");
    assert_eq!(actual.2, expected);
    let canonical = store
        .context_history(&RequestId("response".into()))
        .unwrap();
    assert_eq!(
        canonical
            .iter()
            .find(|(_, _, item)| item.0["type"] == "reasoning")
            .unwrap()
            .2,
        original
    );
    let inspection = store.inspect_context(&snapshot).unwrap();
    assert!(
        inspection
            .authorship
            .iter()
            .any(|author| author.reference == fields[0].reference
                && author.author == crate::context::ContextTextAuthor::Assistant)
    );
}

fn prepared_projection(
    store: &Store,
    head: &RequestId,
    identity: &ConversationIdentity,
    selected: crate::transport::PreparedTransportSelection,
) -> Result<crate::store::PreparedProviderRequest, crate::store::PrepareProviderError> {
    let projected = store.context_request_state_for_transport(head, identity, &selected)?;
    let local = RequestId(uuid::Uuid::new_v4().to_string());
    store.create_request(&local, Some(head), "/root")?;
    store.prepare_provider_request(
        head,
        &local,
        ResponsesRequest {
            input: projected
                .history
                .into_iter()
                .map(|(_, _, item)| item)
                .collect(),
            instructions: "fixture".into(),
            tools: vec![].into(),
            tools_allowed: None,
            model: "deepseek-flash".into(),
            pinned_effort: Effort::High,
            session_id: "fixture".into(),
            prompt_cache_comparison: None,
        },
        &projected.occurrences,
        None,
        selected,
        crate::store::ProviderRequestPurpose::Inference,
    )
}

#[test]
fn reasoning_qualification_requires_exact_issued_native_contract() {
    use crate::transport::{
        CredentialReference, NativeTransportAdmission, PreparedTransportSelection,
    };
    let original = Item(
        json!({"type":"reasoning","summary":[],"content":[{"type":"reasoning_text","text":"canonical"}]}),
    );
    let base = selection().native_admission().unwrap().contract().clone();
    let mut alternatives = vec![];
    let mut endpoint = base.clone();
    endpoint.endpoint = "http://127.0.0.1:40001/responses".into();
    alternatives.push(endpoint);
    let mut credential = base.clone();
    credential.credential = CredentialReference::Env {
        name: "DIFFERENT_OFFLINE_REFERENCE".into(),
    };
    alternatives.push(credential);
    let mut limits = base.clone();
    limits.limits.max_output_tokens = Some(1024);
    alternatives.push(limits);
    let mut connect = base.clone();
    connect.limits.connect_timeout_secs += 1;
    alternatives.push(connect);
    let mut timeout = base.clone();
    timeout.limits.total_timeout_secs += 1;
    alternatives.push(timeout);
    let mut concurrency = base.clone();
    concurrency.limits.max_inflight += 1;
    alternatives.push(concurrency);
    for alternative in alternatives {
        let selected = PreparedTransportSelection::native(
            NativeTransportAdmission::rehydrate(alternative).unwrap(),
        );
        let store = Store::memory().unwrap();
        let (snapshot, _) = fixture(&store, original.clone(), ResponsesRoute::DeepSeek);
        let field = fields(&snapshot).remove(0);
        assert!(field.editable);
        store
            .initialize_context_target(
                &snapshot.operation.origin,
                &ContextProjectionTarget::for_transport("deepseek-flash", &selected),
            )
            .unwrap();
        let changed = store
            .begin_context(&snapshot.operation, &snapshot.head)
            .unwrap();
        let observed = fields(&changed);
        assert_eq!(observed[0].text, "canonical");
        assert!(!observed[0].editable);
        // Commit requalifies the prior editable snapshot against the current owner target.
        let draft = ContextTransactionDraft {
            patch: ContextPatch {
                changes: vec![ContextChange::ReplaceContextText {
                    reference: field.reference,
                    selector: field.selector,
                    expected: crate::context::context_digest(b"canonical"),
                    text: "revised".into(),
                }],
            },
            next_model: None,
            next_effort: None,
        };
        assert!(matches!(
            store
                .commit_context_transaction_guarded(
                    &snapshot,
                    &draft,
                    &JobOutput::Completed(Ok(json!("done"))),
                    &[],
                    || false,
                )
                .unwrap(),
            ContextTransactionStatus::Rejected(_)
        ));
        // Existing overlays must fail final projection under the different contract as well.
        let store = Store::memory().unwrap();
        let (snapshot, _) = fixture(&store, original.clone(), ResponsesRoute::DeepSeek);
        let receipt = rewrite(&store, &snapshot, &fields(&snapshot)[0], "revised");
        assert!(matches!(
            prepared_projection(&store, &receipt.head, &snapshot.operation.origin, selected),
            Err(crate::store::PrepareProviderError::Store(
                StoreError::Context(ContextError::NativeEdit)
            ))
        ));
        let prepared = prepared_projection(
            &store,
            &receipt.head,
            &snapshot.operation.origin,
            selection(),
        )
        .unwrap();
        let wire: serde_json::Value = serde_json::from_slice(prepared.wire_body()).unwrap();
        assert!(wire.to_string().contains("revised"));
        assert_eq!(
            store
                .context_history(&RequestId("response".into()))
                .unwrap()
                .iter()
                .find(|(_, _, item)| item.0["type"] == "reasoning")
                .unwrap()
                .2,
            original
        );
    }
}

#[test]
fn malformed_retained_native_contract_is_not_unknown_continuity() {
    for malformed in [json!("not a contract"), json!({"route":"deep_seek"})] {
        let store = Store::memory().unwrap();
        let original = Item(
            json!({"type":"reasoning","content":[{"type":"reasoning_text","text":"canonical"}]}),
        );
        let (snapshot, _) = fixture(&store, original, ResponsesRoute::DeepSeek);
        // Corrupt an otherwise genuinely issued capture; this is not an authority fixture.
        store.lock().unwrap().execute("UPDATE provider_requests SET issued_json=json_set(issued_json,'$.native_transport',json(?1)) WHERE request_id='response'", [malformed.to_string()]).unwrap();
        assert!(matches!(
            store.begin_context(&snapshot.operation, &snapshot.head),
            Err(StoreError::Json(_))
        ));
    }
    let store = Store::memory().unwrap();
    let original =
        Item(json!({"type":"reasoning","content":[{"type":"reasoning_text","text":"canonical"}]}));
    let (snapshot, _) = fixture(&store, original, ResponsesRoute::DeepSeek);
    store.lock().unwrap().execute("UPDATE provider_requests SET issued_json=json_set(issued_json,'$.native_transport.endpoint','https://unqualified.invalid/responses') WHERE request_id='response'", []).unwrap();
    assert!(matches!(
        store.begin_context(&snapshot.operation, &snapshot.head),
        Err(StoreError::ProviderAdmission(
            crate::transport::TransportError::InvalidAdmission
        ))
    ));
}

#[test]
fn reasoning_historical_absence_does_not_become_same_contract() {
    let store = Store::memory().unwrap();
    let original =
        Item(json!({"type":"reasoning","content":[{"type":"reasoning_text","text":"legacy"}]}));
    let (snapshot, _) =
        fixture_with_source_contract(&store, original, ResponsesRoute::DeepSeek, false);
    let observed = fields(&snapshot);
    assert_eq!(observed[0].text, "legacy");
    assert!(!observed[0].editable);
    let store = Store::memory().unwrap();
    let original = Item(
        json!({"type":"reasoning","content":[{"type":"reasoning_text","text":"known source"}]}),
    );
    let (snapshot, _) = fixture(&store, original, ResponsesRoute::DeepSeek);
    store
        .initialize_context_target(
            &snapshot.operation.origin,
            &ContextProjectionTarget::component(
                "deepseek-flash",
                ResponsesRoute::DeepSeek,
                ResponsesProtocol::Standard,
            ),
        )
        .unwrap();
    let snapshot = store
        .begin_context(&snapshot.operation, &snapshot.head)
        .unwrap();
    let observed = fields(&snapshot);
    assert_eq!(observed[0].text, "known source");
    assert!(!observed[0].editable);
}

#[test]
fn reasoning_is_readable_but_not_editable_for_other_routes_or_unknown_continuity() {
    for (route, extra) in [
        (ResponsesRoute::ChatGptPlan, None),
        (
            ResponsesRoute::DeepSeek,
            Some(("encrypted_content", json!("secret"))),
        ),
        (ResponsesRoute::DeepSeek, Some(("future", json!(true)))),
        (
            ResponsesRoute::DeepSeek,
            Some(("status", json!("incomplete"))),
        ),
        (
            ResponsesRoute::DeepSeek,
            Some(("status", json!("in_progress"))),
        ),
        (ResponsesRoute::DeepSeek, Some(("id", json!(42)))),
        (ResponsesRoute::DeepSeek, Some(("summary", json!(null)))),
        (
            ResponsesRoute::DeepSeek,
            Some((
                "summary",
                json!([{"type":"summary_text","text":"not empty"}]),
            )),
        ),
        (
            ResponsesRoute::DeepSeek,
            Some(("summary", json!("invalid"))),
        ),
    ] {
        let store = Store::memory().unwrap();
        let mut item = Item(
            json!({"type":"reasoning","content":[{"type":"reasoning_text","text":"readable"}]}),
        );
        if let Some((key, value)) = extra {
            item.0[key] = value;
        }
        let (snapshot, _) = fixture(&store, item, route);
        let fields = fields(&snapshot);
        assert_eq!(fields.len(), 1);
        assert_eq!(fields[0].text, "readable");
        assert!(!fields[0].editable);
        let mut document = snapshot.document.clone();
        for block in &mut document.blocks {
            if let ContextBlock::Native { texts, .. } = block {
                for field in texts {
                    if matches!(field.selector, ContextTextSelector::ReasoningText { .. }) {
                        field.text = "forged revision".into();
                        field.editable = true;
                    }
                }
            }
        }
        assert!(
            store
                .commit_context(ContextCommit {
                    snapshot: &snapshot,
                    draft: &ContextDraft {
                        document,
                        next_model: None,
                        next_effort: None
                    },
                    output: &JobOutput::Completed(Ok(json!("done"))),
                    pending: &[]
                })
                .is_err()
        );
    }
}

proptest::proptest! {
    #[test]
    fn reasoning_overlay_parts_have_independent_canonical_oracle(parts in proptest::collection::vec("[a-zλ]{0,20}", 1..5), selected in 0usize..5, replacement in "[a-zΩ]{0,20}") {
        let store = Store::memory().unwrap();
        let selected = selected % parts.len();
        let original = Item(json!({"type":"reasoning","content":parts.iter().map(|text| json!({"type":"reasoning_text","text":text})).collect::<Vec<_>>()}));
        let (snapshot, original) = fixture(&store, original, ResponsesRoute::DeepSeek);
        let fields = fields(&snapshot);
        let receipt = rewrite(&store, &snapshot, &fields[selected], &replacement);
        let projected = store.context_request_state(&receipt.head, &snapshot.operation.origin).unwrap();
        let actual = &projected.history.iter().find(|(_, _, item)| item.0["type"] == "reasoning").unwrap().2;
        let mut expected = original.clone(); expected.0["content"][selected]["text"] = json!(replacement);
        proptest::prop_assert_eq!(actual, &expected);
        for (part, text) in parts.iter().enumerate() {
            proptest::prop_assert_eq!(store.original_context_text(&snapshot, &fields[part].reference, &fields[part].selector).unwrap().unwrap(), text.clone());
        }
    }
}

#[test]
fn reasoning_overlay_reopens_resets_and_refuses_a_different_model_contract() {
    let path =
        std::env::temp_dir().join(format!("reasoning-overlay-{}.sqlite", uuid::Uuid::new_v4()));
    let original = Item(
        json!({"type":"reasoning","summary":[],"content":[{"type":"reasoning_text","text":"canonical"}]}),
    );
    let (head, identity) = {
        let store = Store::open(&path).unwrap();
        let (snapshot, _) = fixture(&store, original.clone(), ResponsesRoute::DeepSeek);
        let observed = fields(&snapshot);
        let receipt = rewrite(&store, &snapshot, &observed[0], "authored");
        assert!(
            store
                .preflight_context_target(
                    &receipt.head,
                    &snapshot.operation.origin,
                    &ContextProjectionTarget {
                        model: "different-model".into(),
                        route: ResponsesRoute::DeepSeek,
                        protocol: ResponsesProtocol::Standard,
                        native_transport: None,
                    }
                )
                .is_err()
        );
        (receipt.head, snapshot.operation.origin)
    };
    let store = Store::open(&path).unwrap();
    let projected = store.context_request_state(&head, &identity).unwrap();
    assert_eq!(
        projected
            .history
            .iter()
            .find(|(_, _, item)| item.0["type"] == "reasoning")
            .unwrap()
            .2
            .0["content"][0]["text"],
        "authored"
    );
    let next = RequestId("reset".into());
    store
        .write_request(&next, Some(&head), "/root", &[], Usage::default())
        .unwrap();
    record(&store, &next, vec![call("reset")], ResponsesRoute::DeepSeek);
    let operation = store.claim(&CallId("reset".into()), &next).unwrap();
    let snapshot = store.begin_context(&operation, &next).unwrap();
    let observed = fields(&snapshot);
    assert_eq!(observed[0].text, "authored");
    assert_eq!(
        store
            .original_context_text(&snapshot, &observed[0].reference, &observed[0].selector)
            .unwrap()
            .unwrap(),
        "canonical"
    );
    let receipt = rewrite(&store, &snapshot, &observed[0], "canonical");
    let projected = store
        .context_request_state(&receipt.head, &identity)
        .unwrap();
    assert_eq!(
        projected
            .history
            .iter()
            .find(|(_, _, item)| item.0["type"] == "reasoning")
            .unwrap()
            .2,
        original
    );
    let occurrences = store.history_occurrences(&receipt.head).unwrap();
    assert!(
        occurrences
            .iter()
            .filter(|item| item.item.0["type"] == "reasoning")
            .all(|item| item.overlays.is_empty())
    );
    drop(store);
    std::fs::remove_file(path).unwrap();
}

#[test]
fn interrupted_plaintext_reasoning_is_readable_without_edit_authority() {
    use crate::store::provider_response::ProviderResponseState;
    use crate::transport::sse::{ResponseAssembly, StreamEvent};
    let store = Store::memory().unwrap();
    let previous = RequestId("interrupted".into());
    store.create_request(&previous, None, "/root").unwrap();
    let prepared = prepare_native(&store, &previous);
    let issued = prepared.issuer();
    let Some(StreamEvent::CompletedItem(done)) = ResponseAssembly::default().accept(&json!({"type":"response.output_item.done","output_index":0,"item":{"type":"reasoning","summary":[],"content":[{"type":"reasoning_text","text":"partial response"}]}}).to_string()).unwrap() else { panic!("completed item") };
    store.append_provider_done(issued, &done).unwrap();
    store
        .stop_provider_response(&previous, ProviderResponseState::Interrupted)
        .unwrap();
    let head = RequestId("editor".into());
    store
        .write_request(&head, Some(&previous), "/root", &[], Usage::default())
        .unwrap();
    record(&store, &head, vec![call("edit")], ResponsesRoute::DeepSeek);
    let operation = store.claim(&CallId("edit".into()), &head).unwrap();
    store
        .initialize_context_target(&operation.origin, &target())
        .unwrap();
    let snapshot = store.begin_context(&operation, &head).unwrap();
    let observed = fields(&snapshot);
    assert_eq!(observed[0].text, "partial response");
    assert!(!observed[0].editable);
}

#[test]
fn effective_selection_reads_current_owner_head_and_final_positional_pin() {
    let store = Store::memory().unwrap();
    let actor = AgentPath("/root".into());
    let identity = store.standalone_identity(actor.clone());
    assert!(
        store
            .effective_inference_selection(&identity, "default", Effort::High)
            .is_err()
    );
    store
        .admit_agent(&actor, None, None, &json!({}), &json!({}))
        .unwrap();
    let empty = store
        .effective_inference_selection(&identity, "default", Effort::High)
        .unwrap();
    assert_eq!(
        (empty.head, empty.model, empty.effort),
        (None, "default".into(), Effort::High)
    );
    let old = RequestId("old-selection".into());
    store
        .write_request(
            &old,
            None,
            "/root",
            &[Item::configuration_update(Effort::Low)],
            Usage::default(),
        )
        .unwrap();
    store.advance_agent_head(&actor, None, Some(&old)).unwrap();
    store
        .initialize_context_target(
            &identity,
            &ContextProjectionTarget {
                model: "selected".into(),
                route: ResponsesRoute::DeepSeek,
                protocol: ResponsesProtocol::Standard,
                native_transport: None,
            },
        )
        .unwrap();
    let current = RequestId("current-selection".into());
    store
        .write_request(
            &current,
            Some(&old),
            "/root",
            &[
                Item::user_text("unrelated"),
                Item::configuration_update(Effort::High),
                Item::configuration_update(Effort::Max),
            ],
            Usage::default(),
        )
        .unwrap();
    store
        .advance_agent_head(&actor, Some(&old), Some(&current))
        .unwrap();
    let observed = store
        .effective_inference_selection(&identity, "stale-default", Effort::None)
        .unwrap();
    assert_eq!(
        (observed.head, observed.model, observed.effort),
        (Some(current), "selected".into(), Effort::Max)
    );
}

#[test]
fn effective_selection_uses_pending_embedded_frontier_before_and_after_settlement_and_reopen() {
    use crate::embedding::{BindingInitialAuthority, HostIdentity};
    use crate::store::embedded_round::EmbeddedRoundOutcome;
    struct Initial(HostIdentity);
    impl BindingInitialAuthority for Initial {
        fn validate_initial_binding(
            &self,
            identity: &HostIdentity,
        ) -> std::result::Result<bool, String> {
            Ok(identity == &self.0)
        }
    }
    for with_settled in [false, true] {
        let path = std::env::temp_dir().join(format!(
            "effective-embedded-{}.sqlite",
            uuid::Uuid::new_v4()
        ));
        let host = HostIdentity {
            run: "effective-run".into(),
            actor: AgentPath("/root".into()),
            incarnation: "first".into(),
        };
        let identity = ConversationIdentity::Embedded {
            run: host.run.clone(),
            actor: host.actor.clone(),
            incarnation: host.incarnation.clone(),
        };
        let pending = RequestId("active-selection".into());
        {
            let store = Store::open(&path).unwrap();
            store
                .bind_initial_embedded_binding(&host, None, &Initial(host.clone()))
                .unwrap();
            store
                .initialize_context_target(
                    &identity,
                    &ContextProjectionTarget {
                        model: "child-admitted-model".into(),
                        route: ResponsesRoute::DeepSeek,
                        protocol: ResponsesProtocol::Standard,
                        native_transport: None,
                    },
                )
                .unwrap();
            let old = RequestId("settled-selection".into());
            if with_settled {
                store
                    .write_embedded_request(
                        &host,
                        &old,
                        None,
                        &[Item::configuration_update(Effort::Low)],
                        Usage::default(),
                    )
                    .unwrap();
                assert!(
                    store
                        .settle_embedded_round(&host, None, &old, EmbeddedRoundOutcome::Completed)
                        .unwrap()
                );
            }
            store
                .write_embedded_request(
                    &host,
                    &pending,
                    with_settled.then_some(&old),
                    &[
                        Item::configuration_update(Effort::High),
                        Item::configuration_update(Effort::Max),
                    ],
                    Usage::default(),
                )
                .unwrap();
            assert_eq!(
                store.embedded_agent_head(&host).unwrap(),
                with_settled.then_some(old)
            );
            let selected = store
                .effective_inference_selection(&identity, "root-default", Effort::None)
                .unwrap();
            assert_eq!(
                (selected.head, selected.model, selected.effort),
                (
                    Some(pending.clone()),
                    "child-admitted-model".into(),
                    Effort::Max
                )
            );
        }
        let store = Store::open(&path).unwrap();
        let selected = store
            .effective_inference_selection(&identity, "root-default", Effort::Low)
            .unwrap();
        assert_eq!(
            (selected.head, selected.model, selected.effort),
            (Some(pending), "child-admitted-model".into(), Effort::Max)
        );
        drop(store);
        std::fs::remove_file(path).unwrap();
    }
}
