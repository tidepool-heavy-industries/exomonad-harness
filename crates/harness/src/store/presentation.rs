//! Presentation types are pure; Store owns durable admission and error mapping.
pub use harness_presentation::{
    ControlValue, FormAlternative, FormDraft, FormError, FormNode, FormOption, FormSpec, ImageMime,
    MAX_PRESENTATION_BYTES, MediaSource, PresentationError, View, parse_errors, validate_svg,
};

impl From<PresentationError> for super::StoreError {
    fn from(error: PresentationError) -> Self {
        match error {
            PresentationError::InvalidView => Self::InvalidActorOutput,
            PresentationError::InvalidForm => Self::InvalidForm,
            PresentationError::InvalidSvg(detail) => Self::InvalidSvg(detail),
            PresentationError::Json(error) => Self::Json(error),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn emitted_haskell_descriptors_and_drafts_cross_store_admission() {
        use crate::store::{
            Store,
            actor_output::{ActorOutputExecution, ActorOutputOrigin},
            forms::{ActorFormAuthority, ActorFormOpen, ActorFormState},
        };
        struct Authority;
        impl ActorFormAuthority for Authority {
            fn validate_form(&self, _: &ActorFormOpen) -> std::result::Result<bool, String> {
                Ok(true)
            }
        }
        let fixture: serde_json::Value =
            serde_json::from_str(include_str!("fixtures/prepared-form-boundaries.json")).unwrap();
        let store = Store::memory().unwrap();
        for case in fixture["cases"].as_array().unwrap() {
            let form: FormSpec = serde_json::from_value(case["descriptor"].clone()).unwrap();
            let opening = ActorFormOpen {
                origin: ActorOutputOrigin {
                    run: "prepared-fixtures".into(),
                    native_actor: 1,
                    incarnation: 1,
                },
                mount_id: case["name"].as_str().unwrap().into(),
                execution: ActorOutputExecution::ActorProgram,
                conversation: None,
                form,
            };
            let card = store.open_actor_form(&Authority, &opening).unwrap();
            let submitted = store
                .submit_actor_form(
                    &opening.origin,
                    &opening.mount_id,
                    "attempt",
                    &case["values"],
                )
                .unwrap();
            assert_eq!(submitted.sequence, card.sequence);
            assert_eq!(
                serde_json::to_value(submitted.draft.as_ref().unwrap()).unwrap(),
                case["values"]
            );
            if case["accepted"] == false {
                assert!(
                    store
                        .reject_actor_form(
                            &opening.origin,
                            &opening.mount_id,
                            "attempt",
                            &case["errors"]
                        )
                        .unwrap()
                );
                let rejected = store
                    .actor_form(&opening.origin, &opening.mount_id)
                    .unwrap();
                assert_eq!(rejected.state, ActorFormState::Open);
                assert_eq!(rejected.sequence, card.sequence);
                assert_eq!(
                    serde_json::to_value(rejected.errors).unwrap(),
                    case["errors"]
                );
            }
        }
    }
}
