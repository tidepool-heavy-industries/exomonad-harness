use super::*;
use serde_json::json;
#[test]
fn serde_contract_preserves_rich_choice_identity_and_ignores_inactive_values() {
    let spec:FormSpec=serde_json::from_value(json!({"version":1,"root":{"kind":"alternatives","id":"f0","label":"Same","initial":null,"options":[{"id":"o0","label":"Same","presentation":{"kind":"markdown","text":"**first**"},"form":{"kind":"int","id":"f1","label":"Value","initial":null}},{"id":"o1","label":"Same","presentation":{"kind":"text","text":"second"},"form":{"kind":"text","id":"f2","label":"Value","initial":null}}]}})).unwrap();
    spec.validate().unwrap();
    let draft: FormDraft = serde_json::from_value(
        json!({"f0":"o1","f1":"invalid but inactive","f2":"original selected"}),
    )
    .unwrap();
    spec.validate_draft(&draft).unwrap();
    let bad: FormDraft = serde_json::from_value(json!({"f0":"o0","f1":123})).unwrap();
    assert!(matches!(
        spec.validate_draft(&bad),
        Err(PresentationError::InvalidForm)
    ));
    assert!(
        serde_json::from_value::<FormSpec>(json!({"version":1,"root":{"kind":"invented"}}))
            .is_err()
    );
}
#[test]
fn standard_svg_parser_accepts_general_documents_and_reports_invalid_xml() {
    validate_svg("<svg xmlns='http://www.w3.org/2000/svg'><defs><linearGradient id='a'><stop offset='0'/></linearGradient></defs><path d='M0 0 L10 10'/><text>x &amp; y</text></svg>").unwrap();
    assert!(matches!(
        validate_svg("<svg><path></svg>"),
        Err(PresentationError::InvalidSvg(_))
    ));
    assert!(validate_svg("<html/>").is_err());
}

mod integer_wire_tests {
    use super::*;
    use serde_json::json;
    #[test]
    fn large_int_seeds_and_submissions_are_exact_lexemes() {
        let spec:FormSpec=serde_json::from_value(json!({"version":1,"root":{"kind":"int","id":"f0","label":"Integer","initial":"9007199254740993"}})).unwrap();
        spec.validate().unwrap();
        assert_eq!(
            serde_json::to_value(&spec).unwrap()["root"]["initial"],
            "9007199254740993"
        );
        for raw in ["9007199254740993", "1.5", "-"] {
            let draft: FormDraft = serde_json::from_value(json!({"f0":raw})).unwrap();
            spec.validate_draft(&draft).unwrap();
            assert_eq!(serde_json::to_value(draft).unwrap()["f0"], raw);
        }
        let numeric: FormDraft = serde_json::from_value(json!({"f0":123})).unwrap();
        assert!(spec.validate_draft(&numeric).is_err());
    }
}

mod companion_forms_tests {
    use super::*;
    fn form() -> FormSpec {
        serde_json::from_value(serde_json::json!({"version":1,"root": {
            "kind":"alternatives_many", "id":"doors", "label":"Doors", "initial":[], "options":[
                {"id":"a","label":"A","presentation":{"kind":"text","text":"A"},"form":{"kind":"slider","id":"s","label":"Strangeness","minimum":0,"maximum":10,"initial":4}},
                {"id":"b","label":"B","presentation":{"kind":"text","text":"B"},"form":{"kind":"text","id":"t","label":"Thought","initial":null}}
            ]
        }})).unwrap()
    }
    #[test]
    fn validates_selected_branches_and_slider_bounds() {
        let form = form();
        form.validate().unwrap();
        for value in [
            serde_json::json!({"doors":[],"s":999}),
            serde_json::json!({"doors":["a","b"],"s":0,"t":"hello"}),
            serde_json::json!({"doors":["a"],"s":10}),
        ] {
            form.validate_draft(&serde_json::from_value(value).unwrap())
                .unwrap();
        }
        for value in [
            serde_json::json!({"doors":["a"],"s":11}),
            serde_json::json!({"doors":["a","a"],"s":4}),
            serde_json::json!({"doors":["missing"]}),
        ] {
            assert!(
                form.validate_draft(&serde_json::from_value(value).unwrap())
                    .is_err()
            );
        }
    }
    #[test]
    fn initial_answers_obey_selection_and_integer_syntax() {
        let mut form = form();
        if let FormNode::AlternativesMany { initial, .. } = &mut form.root {
            *initial = Some(vec!["a".into(), "a".into()]);
        }
        assert!(form.validate().is_err());
        for initial in ["-", "1.5", "", "+1"] {
            let integer = FormSpec {
                version: 1,
                root: FormNode::Int {
                    id: "i".into(),
                    label: "i".into(),
                    initial: Some(initial.into()),
                },
            };
            assert!(integer.validate().is_err());
            // Intermediate typing still reaches Haskell's field-level validator.
            integer
                .validate_draft(&[("i".into(), ControlValue::Text(initial.into()))].into())
                .unwrap();
        }
    }
}

#[test]
fn single_choices_require_options_and_number_drafts_require_numbers() {
    let presentation = View::Text {
        text: "option".into(),
        truncated: false,
    };
    for mut root in [
        FormNode::Choice {
            id: "f0".into(),
            label: "Value".into(),
            initial: None,
            options: vec![FormOption {
                id: "o0".into(),
                label: "Option".into(),
                presentation: presentation.clone(),
            }],
        },
        FormNode::Alternatives {
            id: "f0".into(),
            label: "Value".into(),
            initial: None,
            options: vec![FormAlternative {
                id: "o0".into(),
                label: "Option".into(),
                presentation: presentation.clone(),
                form: FormNode::Pure,
            }],
        },
    ] {
        FormSpec {
            version: 1,
            root: root.clone(),
        }
        .validate()
        .unwrap();
        match &mut root {
            FormNode::Choice { options, .. } => options.clear(),
            FormNode::Alternatives { options, .. } => options.clear(),
            _ => unreachable!(),
        }
        assert!(matches!(
            FormSpec { version: 1, root }.validate(),
            Err(PresentationError::InvalidForm)
        ));
    }
    let number = FormSpec {
        version: 1,
        root: FormNode::Number {
            id: "f0".into(),
            label: "Value".into(),
            initial: None,
        },
    };
    number.validate().unwrap();
    number
        .validate_draft(&[("f0".into(), ControlValue::Number(1.25))].into())
        .unwrap();
    assert!(matches!(
        number.validate_draft(&[("f0".into(), ControlValue::Text(String::new()))].into()),
        Err(PresentationError::InvalidForm)
    ));
}
