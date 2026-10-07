//! Immutable bounded assets owned by the output Store, independent of callbacks.
use super::{Result, Store, StoreError};
use rusqlite::{OptionalExtension, params};
pub const MAX_MEDIA_BYTES: usize = 4 * 1024 * 1024;
#[derive(Clone, Debug)]
pub struct RetainedActorMedia {
    pub hash: String,
    pub mime: String,
    pub bytes: Vec<u8>,
}
impl Store {
    pub fn retain_actor_media(&self, mime: &str, bytes: &[u8]) -> Result<RetainedActorMedia> {
        if bytes.is_empty()
            || bytes.len() > MAX_MEDIA_BYTES
            || !matches!(mime, "image/png" | "image/jpeg" | "image/webp")
        {
            return Err(StoreError::InvalidMedia);
        }
        let magic = match mime {
            "image/png" => bytes.starts_with(b"\x89PNG\r\n\x1a\n"),
            "image/jpeg" => bytes.starts_with(b"\xff\xd8\xff"),
            "image/webp" => bytes.starts_with(b"RIFF") && bytes.get(8..12) == Some(b"WEBP"),
            _ => false,
        };
        if !magic {
            return Err(StoreError::InvalidMedia);
        }
        let mut hasher = blake3::Hasher::new();
        hasher.update(mime.as_bytes());
        hasher.update(&[0]);
        hasher.update(bytes);
        let hash = hasher.finalize().to_hex().to_string();
        self.lock().execute(
            "INSERT OR IGNORE INTO actor_media(hash,mime,bytes) VALUES(?1,?2,?3)",
            params![hash, mime, bytes],
        )?;
        Ok(RetainedActorMedia {
            hash,
            mime: mime.into(),
            bytes: bytes.into(),
        })
    }
    pub fn actor_media(&self, hash: &str) -> Result<Option<RetainedActorMedia>> {
        if hash.len() != 64 || !hash.bytes().all(|b| b.is_ascii_hexdigit()) {
            return Err(StoreError::InvalidMedia);
        }
        Ok(self
            .lock()
            .query_row(
                "SELECT mime,bytes FROM actor_media WHERE hash=?1",
                [hash],
                |r| {
                    Ok(RetainedActorMedia {
                        hash: hash.to_owned(),
                        mime: r.get(0)?,
                        bytes: r.get(1)?,
                    })
                },
            )
            .optional()?)
    }
    /// Lower data sources to durable immutable references once at publication.
    pub fn retain_view_media(
        &self,
        view: &super::presentation::View,
    ) -> Result<super::presentation::View> {
        use super::presentation::{MediaSource, View};
        use base64::Engine;
        view.validate()?;
        let mut view = view.clone();
        match &mut view {
            View::Image {
                source: MediaSource::Data { mime, base64 },
                alt,
                truncated,
            } => {
                if base64.len() > MAX_MEDIA_BYTES * 4 / 3 + 4 {
                    return Err(StoreError::InvalidMedia);
                }
                let bytes = base64::engine::general_purpose::STANDARD
                    .decode(base64.as_bytes())
                    .map_err(|_| StoreError::InvalidMedia)?;
                let retained = self.retain_actor_media(mime.as_str(), &bytes)?;
                view = View::Image {
                    source: MediaSource::Retained {
                        hash: retained.hash,
                    },
                    alt: alt.clone(),
                    truncated: *truncated,
                };
            }
            View::Row { children } | View::Column { children } => {
                for child in children {
                    *child = self.retain_view_media(child)?;
                }
            }
            View::Caption { body, .. } => **body = self.retain_view_media(body)?,
            _ => (),
        }
        Ok(view)
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn assets_are_retained_independently_and_reject_format_and_size_errors() {
        let path =
            std::env::temp_dir().join(format!("harness-media-{}.sqlite", uuid::Uuid::new_v4()));
        let hash;
        {
            let store = Store::open(&path).unwrap();
            let bytes = b"\x89PNG\r\n\x1a\nretained";
            let first = store.retain_actor_media("image/png", bytes).unwrap();
            let duplicate = store.retain_actor_media("image/png", bytes).unwrap();
            assert_eq!(first.hash, duplicate.hash);
            hash = first.hash;
            assert!(store.retain_actor_media("image/png", b"not png").is_err());
            assert!(store.retain_actor_media("text/html", bytes).is_err());
            assert!(
                store
                    .retain_actor_media("image/png", &vec![0; MAX_MEDIA_BYTES + 1])
                    .is_err()
            );
        }
        {
            let store = Store::open(&path).unwrap();
            assert_eq!(
                store.actor_media(&hash).unwrap().unwrap().bytes,
                b"\x89PNG\r\n\x1a\nretained"
            );
            assert!(store.actor_media("../source").is_err());
        }
        std::fs::remove_file(path).unwrap();
    }
}
