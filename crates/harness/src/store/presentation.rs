//! Canonical serde presentation contract. Haskell lowers once at this boundary.
use super::{Result, StoreError};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashSet};
pub fn validate_svg(source: &str) -> Result<()> {
    let document = roxmltree::Document::parse(source)
        .map_err(|error| StoreError::InvalidSvg(error.to_string()))?;
    let root = document.root_element();
    if root.tag_name().name() != "svg"
        || root
            .tag_name()
            .namespace()
            .is_some_and(|ns| ns != "http://www.w3.org/2000/svg")
    {
        return Err(StoreError::InvalidSvg(
            "root element must be svg in the SVG namespace".into(),
        ));
    }
    Ok(())
}
pub const MAX_PRESENTATION_BYTES: usize = 128 * 1024;
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum View {
    Text {
        text: String,
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        truncated: bool,
    },
    Markdown {
        text: String,
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        truncated: bool,
    },
    Row {
        children: Vec<View>,
    },
    Column {
        children: Vec<View>,
    },
    Caption {
        body: Box<View>,
        text: String,
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        truncated: bool,
    },
    Svg {
        source: String,
    },
    Image {
        source: MediaSource,
        alt: String,
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        truncated: bool,
    },
    Inspection {
        text: String,
        has_more: bool,
        unavailable: bool,
    },
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum MediaSource {
    Data { mime: ImageMime, base64: String },
    Retained { hash: String },
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum ImageMime {
    #[serde(rename = "image/png")]
    Png,
    #[serde(rename = "image/jpeg")]
    Jpeg,
    #[serde(rename = "image/webp")]
    Webp,
}
impl ImageMime {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Png => "image/png",
            Self::Jpeg => "image/jpeg",
            Self::Webp => "image/webp",
        }
    }
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FormSpec {
    pub version: u32,
    pub root: FormNode,
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum FormNode {
    Pure,
    Empty,
    Group {
        children: Vec<FormNode>,
    },
    Section {
        title: String,
        child: Box<FormNode>,
    },
    View {
        presentation: View,
    },
    Text {
        id: String,
        label: String,
        initial: Option<String>,
    },
    Int {
        id: String,
        label: String,
        initial: Option<String>,
    },
    Number {
        id: String,
        label: String,
        initial: Option<f64>,
    },
    Bool {
        id: String,
        label: String,
        initial: Option<bool>,
    },
    Choice {
        id: String,
        label: String,
        options: Vec<FormOption>,
        initial: Option<String>,
    },
    Many {
        id: String,
        label: String,
        options: Vec<FormOption>,
        initial: Option<Vec<String>>,
    },
    Alternatives {
        id: String,
        label: String,
        options: Vec<FormAlternative>,
        initial: Option<String>,
    },
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FormOption {
    pub id: String,
    pub label: String,
    pub presentation: View,
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FormAlternative {
    pub id: String,
    pub label: String,
    pub presentation: View,
    pub form: FormNode,
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum ControlValue {
    Bool(bool),
    Integer(i64),
    Number(f64),
    Text(String),
    Many(Vec<String>),
    Empty,
}
pub type FormDraft = BTreeMap<String, ControlValue>;
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FormError {
    pub field: Option<String>,
    pub message: String,
}
impl View {
    pub fn validate(&self) -> Result<()> {
        if let View::Svg { source } = self {
            validate_svg(source)?;
        }
        fn go(v: &View, depth: usize) -> bool {
            if depth > 32 {
                return false;
            }
            match v {
                View::Row { children } | View::Column { children } => {
                    children.len() <= 1024 && children.iter().all(|c| go(c, depth + 1))
                }
                View::Caption { body, .. } => go(body, depth + 1),
                View::Image {
                    source: MediaSource::Retained { hash },
                    ..
                } => hash.len() == 64 && hash.bytes().all(|b| b.is_ascii_hexdigit()),
                View::Svg { source } => validate_svg(source).is_ok(),
                _ => true,
            }
        }
        if go(self, 0) && serde_json::to_vec(self)?.len() <= MAX_PRESENTATION_BYTES {
            Ok(())
        } else {
            Err(StoreError::InvalidActorOutput)
        }
    }
}
impl FormSpec {
    pub fn validate(&self) -> Result<()> {
        fn options<'a>(
            xs: impl Iterator<Item = (&'a String, &'a View)>,
            initial: impl Iterator<Item = &'a String>,
        ) -> bool {
            let mut ids = HashSet::new();
            let mut n = 0;
            for (id, view) in xs {
                n += 1;
                if id.is_empty() || id.len() > 128 || !ids.insert(id) || view.validate().is_err() {
                    return false;
                }
            }
            n > 0 && n <= 1024 && initial.into_iter().all(|id| ids.contains(id))
        }
        fn go(n: &FormNode, depth: usize, ids: &mut HashSet<String>) -> bool {
            if depth > 32 {
                return false;
            }
            let id = match n {
                FormNode::Text { id, .. }
                | FormNode::Int { id, .. }
                | FormNode::Number { id, .. }
                | FormNode::Bool { id, .. }
                | FormNode::Choice { id, .. }
                | FormNode::Many { id, .. }
                | FormNode::Alternatives { id, .. } => Some(id),
                _ => None,
            };
            if let Some(id) = id {
                if id.is_empty() || id.len() > 128 || !ids.insert(id.clone()) {
                    return false;
                }
            }
            match n {
                FormNode::Pure
                | FormNode::Empty
                | FormNode::Text { .. }
                | FormNode::Int { .. }
                | FormNode::Number { .. }
                | FormNode::Bool { .. } => true,
                FormNode::Group { children } => {
                    children.len() <= 1024 && children.iter().all(|c| go(c, depth + 1, ids))
                }
                FormNode::Section { child, .. } => go(child, depth + 1, ids),
                FormNode::View { presentation } => presentation.validate().is_ok(),
                FormNode::Choice {
                    options: xs,
                    initial,
                    ..
                } => options(xs.iter().map(|o| (&o.id, &o.presentation)), initial.iter()),
                FormNode::Many {
                    options: xs,
                    initial,
                    ..
                } => options(
                    xs.iter().map(|o| (&o.id, &o.presentation)),
                    initial.iter().flatten(),
                ),
                FormNode::Alternatives {
                    options: xs,
                    initial,
                    ..
                } => {
                    options(xs.iter().map(|o| (&o.id, &o.presentation)), initial.iter())
                        && xs.iter().all(|o| go(&o.form, depth + 1, ids))
                }
            }
        }
        if self.version == 1
            && serde_json::to_vec(self)?.len() <= MAX_PRESENTATION_BYTES
            && go(&self.root, 0, &mut HashSet::new())
        {
            Ok(())
        } else {
            Err(StoreError::InvalidForm)
        }
    }
    pub fn validate_draft(&self, draft: &FormDraft) -> Result<()> {
        if draft.len() > 1024 || serde_json::to_vec(draft)?.len() > MAX_PRESENTATION_BYTES {
            return Err(StoreError::InvalidForm);
        }
        fn control<'a>(draft: &'a FormDraft, id: &str) -> Option<&'a ControlValue> {
            draft.get(id).filter(|v| !matches!(v, ControlValue::Empty))
        }
        fn go(n: &FormNode, d: &FormDraft) -> bool {
            match n {
   FormNode::Text{id,..}=>control(d,id).is_none_or(|v|matches!(v,ControlValue::Text(_))),FormNode::Int{id,..}=>control(d,id).is_none_or(|v|matches!(v,ControlValue::Text(_))),FormNode::Number{id,..}=>control(d,id).is_none_or(|v|matches!(v,ControlValue::Integer(_)|ControlValue::Number(_))),FormNode::Bool{id,..}=>control(d,id).is_none_or(|v|matches!(v,ControlValue::Bool(_))),
   FormNode::Choice{id,options,..}=>control(d,id).is_none_or(|v|matches!(v,ControlValue::Text(s) if options.iter().any(|o|&o.id==s))),
   FormNode::Many{id,options,..}=>control(d,id).is_none_or(|v|matches!(v,ControlValue::Many(xs) if xs.iter().collect::<HashSet<_>>().len()==xs.len()&&xs.iter().all(|s|options.iter().any(|o|&o.id==s)))),
   FormNode::Alternatives{id,options,..}=>match control(d,id){None=>true,Some(ControlValue::Text(s))=>options.iter().find(|o|&o.id==s).is_some_and(|o|go(&o.form,d)),_=>false},
   FormNode::Group{children}=>children.iter().all(|c|go(c,d)),FormNode::Section{child,..}=>go(child,d),_=>true}
        }
        if go(&self.root, draft) {
            Ok(())
        } else {
            Err(StoreError::InvalidForm)
        }
    }
}
pub fn parse_errors(errors: &serde_json::Value) -> Result<Vec<FormError>> {
    let errors: Vec<FormError> =
        serde_json::from_value(errors.clone()).map_err(|_| StoreError::InvalidForm)?;
    if errors.len() > 1024 || serde_json::to_vec(&errors)?.len() > 32768 {
        return Err(StoreError::InvalidForm);
    }
    Ok(errors)
}
#[cfg(test)]
mod tests {
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
        let bad: FormDraft =
            serde_json::from_value(json!({"f0":"o0","f1":123})).unwrap();
        assert!(matches!(
            spec.validate_draft(&bad),
            Err(StoreError::InvalidForm)
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
            Err(StoreError::InvalidSvg(_))
        ));
        assert!(validate_svg("<html/>").is_err());
    }
}
