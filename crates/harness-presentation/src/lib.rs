//! Pure serde presentation contract and structural validation.
//! Durable admission, form leases and retained-media authority remain with Harness.
mod error;
pub use error::PresentationError;
pub type PresentationResult<T> = std::result::Result<T, PresentationError>;
use PresentationResult as Result;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashSet};
pub fn validate_svg(source: &str) -> Result<()> {
    let document = roxmltree::Document::parse(source)
        .map_err(|error| PresentationError::InvalidSvg(error.to_string()))?;
    let root = document.root_element();
    if root.tag_name().name() != "svg"
        || root
            .tag_name()
            .namespace()
            .is_some_and(|ns| ns != "http://www.w3.org/2000/svg")
    {
        return Err(PresentationError::InvalidSvg(
            "root element must be svg in the SVG namespace".into(),
        ));
    }
    Ok(())
}
pub const MAX_PRESENTATION_BYTES: usize = 128 * 1024;
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
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
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum MediaSource {
    Data { mime: ImageMime, base64: String },
    Retained { hash: String },
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
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
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct FormSpec {
    #[schemars(schema_with = "form_version_schema")]
    pub version: u32,
    pub root: FormNode,
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
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
        #[schemars(schema_with = "optional_number_schema")]
        initial: Option<f64>,
    },
    Slider {
        id: String,
        label: String,
        #[schemars(schema_with = "number_control_schema")]
        minimum: f64,
        #[schemars(schema_with = "number_control_schema")]
        maximum: f64,
        #[schemars(schema_with = "number_control_schema")]
        initial: f64,
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
    AlternativesMany {
        id: String,
        label: String,
        options: Vec<FormAlternative>,
        initial: Option<Vec<String>>,
    },
    Alternatives {
        id: String,
        label: String,
        options: Vec<FormAlternative>,
        initial: Option<String>,
    },
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct FormOption {
    pub id: String,
    pub label: String,
    pub presentation: View,
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct FormAlternative {
    pub id: String,
    pub label: String,
    pub presentation: View,
    pub form: FormNode,
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(untagged)]
pub enum ControlValue {
    Bool(bool),
    Integer(#[schemars(schema_with = "integer_control_schema")] i64),
    Number(#[schemars(schema_with = "number_control_schema")] f64),
    Text(String),
    Many(Vec<String>),
    Empty,
}
fn form_version_schema(_: &mut schemars::SchemaGenerator) -> schemars::Schema {
    schemars::json_schema!({"type": "integer", "const": 1})
}
fn number_control_schema(_: &mut schemars::SchemaGenerator) -> schemars::Schema {
    schemars::json_schema!({"type": "number"})
}
fn optional_number_schema(_: &mut schemars::SchemaGenerator) -> schemars::Schema {
    schemars::json_schema!({"type": ["number", "null"]})
}
fn integer_control_schema(_: &mut schemars::SchemaGenerator) -> schemars::Schema {
    schemars::json_schema!({"type": "integer", "minimum": i64::MIN, "maximum": i64::MAX})
}
pub type FormDraft = BTreeMap<String, ControlValue>;
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct FormError {
    pub field: Option<String>,
    pub message: String,
}
impl View {
    pub fn validate(&self) -> Result<()> {
        fn go(view: &View, depth: usize) -> Result<()> {
            if depth > 32 {
                return Err(PresentationError::InvalidView);
            }
            match view {
                View::Svg { source } => validate_svg(source)?,
                View::Row { children } | View::Column { children } => {
                    if children.len() > 1024 {
                        return Err(PresentationError::InvalidView);
                    }
                    for child in children {
                        go(child, depth + 1)?;
                    }
                }
                View::Caption { body, .. } => go(body, depth + 1)?,
                View::Image {
                    source: MediaSource::Retained { hash },
                    ..
                } => {
                    if hash.len() != 64 || !hash.bytes().all(|b| b.is_ascii_hexdigit()) {
                        return Err(PresentationError::InvalidView);
                    }
                }
                _ => (),
            };
            Ok(())
        }
        if serde_json::to_vec(self)?.len() > MAX_PRESENTATION_BYTES {
            return Err(PresentationError::InvalidView);
        }
        go(self, 0)
    }
}
impl FormSpec {
    pub fn validate(&self) -> Result<()> {
        fn options<'a>(
            xs: impl Iterator<Item = (&'a String, &'a View)>,
            initial: impl Iterator<Item = &'a String>,
            nonempty: bool,
        ) -> bool {
            let mut ids = HashSet::new();
            let mut n = 0;
            for (id, view) in xs {
                n += 1;
                if id.is_empty() || id.len() > 128 || !ids.insert(id) || view.validate().is_err() {
                    return false;
                }
            }
            let mut selected = HashSet::new();
            (!nonempty || n > 0)
                && n <= 1024
                && initial
                    .into_iter()
                    .all(|id| ids.contains(id) && selected.insert(id))
        }
        fn go(n: &FormNode, depth: usize, ids: &mut HashSet<String>) -> bool {
            if depth > 32 {
                return false;
            }
            let id = match n {
                FormNode::Text { id, .. }
                | FormNode::Int { id, .. }
                | FormNode::Number { id, .. }
                | FormNode::Slider { id, .. }
                | FormNode::AlternativesMany { id, .. }
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
                | FormNode::Number { .. }
                | FormNode::Bool { .. } => true,
                FormNode::Int { initial, .. } => initial.as_ref().is_none_or(|text| {
                    let digits = text.strip_prefix('-').unwrap_or(text);
                    !digits.is_empty() && digits.bytes().all(|b| b.is_ascii_digit())
                }),
                FormNode::Slider {
                    minimum,
                    maximum,
                    initial,
                    ..
                } => {
                    minimum.is_finite()
                        && maximum.is_finite()
                        && initial.is_finite()
                        && minimum < maximum
                        && initial >= minimum
                        && initial <= maximum
                }
                FormNode::AlternativesMany {
                    options: xs,
                    initial,
                    ..
                } => {
                    options(
                        xs.iter().map(|o| (&o.id, &o.presentation)),
                        initial.iter().flatten(),
                        false,
                    ) && xs.iter().all(|o| go(&o.form, depth + 1, ids))
                }
                FormNode::Group { children } => {
                    children.len() <= 1024 && children.iter().all(|c| go(c, depth + 1, ids))
                }
                FormNode::Section { child, .. } => go(child, depth + 1, ids),
                FormNode::View { presentation } => presentation.validate().is_ok(),
                FormNode::Choice {
                    options: xs,
                    initial,
                    ..
                } => options(
                    xs.iter().map(|o| (&o.id, &o.presentation)),
                    initial.iter(),
                    true,
                ),
                FormNode::Many {
                    options: xs,
                    initial,
                    ..
                } => options(
                    xs.iter().map(|o| (&o.id, &o.presentation)),
                    initial.iter().flatten(),
                    false,
                ),
                FormNode::Alternatives {
                    options: xs,
                    initial,
                    ..
                } => {
                    options(
                        xs.iter().map(|o| (&o.id, &o.presentation)),
                        initial.iter(),
                        true,
                    ) && xs.iter().all(|o| go(&o.form, depth + 1, ids))
                }
            }
        }
        if self.version == 1
            && serde_json::to_vec(self)?.len() <= MAX_PRESENTATION_BYTES
            && go(&self.root, 0, &mut HashSet::new())
        {
            Ok(())
        } else {
            Err(PresentationError::InvalidForm)
        }
    }
    pub fn validate_draft(&self, draft: &FormDraft) -> Result<()> {
        if draft.len() > 1024 || serde_json::to_vec(draft)?.len() > MAX_PRESENTATION_BYTES {
            return Err(PresentationError::InvalidForm);
        }
        fn control<'a>(draft: &'a FormDraft, id: &str) -> Option<&'a ControlValue> {
            draft.get(id).filter(|v| !matches!(v, ControlValue::Empty))
        }
        fn go(n: &FormNode, d: &FormDraft) -> bool {
            match n {
   FormNode::Text{id,..}=>control(d,id).is_none_or(|v|matches!(v,ControlValue::Text(_))),FormNode::Int{id,..}=>control(d,id).is_none_or(|v|matches!(v,ControlValue::Text(_))),FormNode::Number{id,..}=>control(d,id).is_none_or(|v|matches!(v,ControlValue::Integer(_)|ControlValue::Number(_))),FormNode::Bool{id,..}=>control(d,id).is_none_or(|v|matches!(v,ControlValue::Bool(_))),
   FormNode::Choice{id,options,..}=>control(d,id).is_none_or(|v|matches!(v,ControlValue::Text(s) if options.iter().any(|o|&o.id==s))),
   FormNode::Many{id,options,..}=>control(d,id).is_none_or(|v|matches!(v,ControlValue::Many(xs) if xs.iter().collect::<HashSet<_>>().len()==xs.len()&&xs.iter().all(|s|options.iter().any(|o|&o.id==s)))),
   FormNode::Slider{id, minimum, maximum, ..} => control(d,id).is_none_or(|v| match v { ControlValue::Integer(n) => (*n as f64) >= *minimum && (*n as f64) <= *maximum, ControlValue::Number(n) => n.is_finite() && n >= minimum && n <= maximum, _ => false }),
   FormNode::AlternativesMany{id,options,..} => control(d,id).is_none_or(|v| matches!(v, ControlValue::Many(xs) if xs.iter().collect::<HashSet<_>>().len()==xs.len() && xs.iter().all(|s|options.iter().find(|o| &o.id==s).is_some_and(|o|go(&o.form,d))))),
   FormNode::Alternatives{id,options,..}=>match control(d,id){None=>true,Some(ControlValue::Text(s))=>options.iter().find(|o|&o.id==s).is_some_and(|o|go(&o.form,d)),_=>false},
   FormNode::Group{children}=>children.iter().all(|c|go(c,d)),FormNode::Section{child,..}=>go(child,d),_=>true}
        }
        if go(&self.root, draft) {
            Ok(())
        } else {
            Err(PresentationError::InvalidForm)
        }
    }
}
pub fn parse_errors(errors: &serde_json::Value) -> Result<Vec<FormError>> {
    let errors: Vec<FormError> =
        serde_json::from_value(errors.clone()).map_err(|_| PresentationError::InvalidForm)?;
    if errors.len() > 1024 || serde_json::to_vec(&errors)?.len() > 32768 {
        return Err(PresentationError::InvalidForm);
    }
    Ok(errors)
}

#[cfg(test)]
mod tests;
