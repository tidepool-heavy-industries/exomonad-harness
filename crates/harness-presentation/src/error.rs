use std::{error::Error, fmt};

#[derive(Debug)]
pub enum PresentationError {
    InvalidView,
    InvalidForm,
    InvalidSvg(String),
    Json(serde_json::Error),
}

impl fmt::Display for PresentationError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidView => {
                formatter.write_str("actor output exceeds its bounds or has invalid identity/keys")
            }
            Self::InvalidForm => {
                formatter.write_str("invalid mounted form or submitted control values")
            }
            Self::InvalidSvg(detail) => write!(formatter, "invalid SVG document: {detail}"),
            Self::Json(error) => fmt::Display::fmt(error, formatter),
        }
    }
}

impl Error for PresentationError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Json(error) => Some(error),
            _ => None,
        }
    }
}

impl From<serde_json::Error> for PresentationError {
    fn from(error: serde_json::Error) -> Self {
        Self::Json(error)
    }
}
