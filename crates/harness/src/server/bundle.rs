//! Composition verifies declared source, schema and asset identities before listening.
use super::{assets, browser_contract};
use axum::{body::Body, http::StatusCode, response::Response};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{collections::BTreeMap, fs, path::Path, sync::Arc};

pub const BROWSER_BUNDLE_MANIFEST: &str = "browser-bundle.json";

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct BrowserBundleIdentity {
    pub runtime_source_sha256: String,
    pub schema_sha256: String,
}
impl BrowserBundleIdentity {
    /// Composition supplies this source identity from its declared build output.
    /// The schema identity always comes from the actual compiled DTO owner.
    pub fn for_runtime_source(runtime_source_sha256: String) -> Self {
        Self {
            runtime_source_sha256,
            schema_sha256: browser_schema_sha256(),
        }
    }

    pub fn from_declared_source(encoded: &[u8]) -> Result<Self, BrowserBundleError> {
        #[derive(Deserialize)]
        #[serde(rename_all = "camelCase", deny_unknown_fields)]
        struct SourceIdentity {
            runtime_source_sha256: String,
        }
        let source: SourceIdentity = serde_json::from_slice(encoded)?;
        if !valid_digest(&source.runtime_source_sha256) {
            return Err(BrowserBundleError::IdentityMismatch);
        }
        Ok(Self::for_runtime_source(source.runtime_source_sha256))
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct BrowserBundleManifest {
    pub version: u32,
    pub identity: BrowserBundleIdentity,
    pub assets: BTreeMap<String, String>,
}

#[derive(Debug, thiserror::Error)]
pub enum BrowserBundleError {
    #[error("browser bundle I/O failed: {0}")]
    Io(#[from] std::io::Error),
    #[error("invalid browser bundle manifest: {0}")]
    Manifest(#[from] serde_json::Error),
    #[error("browser bundle source or schema does not match the runtime")]
    IdentityMismatch,
    #[error("invalid browser bundle asset: {0}")]
    InvalidAsset(String),
    #[error("browser bundle asset hash does not match: {0}")]
    AssetMismatch(String),
}

/// Assets are retained as verified immutable bytes, so file changes after
/// admission cannot alter the release served to a browser.
#[derive(Clone, Debug)]
pub struct VerifiedBrowserBundle {
    identity: BrowserBundleIdentity,
    assets: BTreeMap<String, Arc<[u8]>>,
}
impl VerifiedBrowserBundle {
    pub fn identity(&self) -> &BrowserBundleIdentity {
        &self.identity
    }
    pub(super) fn response(&self, path: &str) -> Response<Body> {
        if !assets::valid_asset_path(path) {
            return assets::response(
                StatusCode::BAD_REQUEST,
                "invalid asset path",
                None,
                Body::empty(),
            );
        }
        match self.assets.get(path) {
            Some(bytes) => assets::response(
                StatusCode::OK,
                "",
                Some(assets::mime_type(Path::new(path))),
                Body::from(bytes.to_vec()),
            ),
            None => assets::response(StatusCode::NOT_FOUND, "not found", None, Body::empty()),
        }
    }
}

fn sha256(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}
fn valid_digest(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}
pub fn browser_schema_sha256() -> String {
    sha256(
        &serde_json::to_vec_pretty(&browser_contract::schemas())
            .expect("browser schemas serialize"),
    )
}

pub fn verify_browser_bundle(
    root: impl AsRef<Path>,
    expected_identity: &BrowserBundleIdentity,
) -> Result<VerifiedBrowserBundle, BrowserBundleError> {
    let root = root.as_ref();
    let manifest_path = root.join(BROWSER_BUNDLE_MANIFEST);
    if fs::symlink_metadata(&manifest_path)?
        .file_type()
        .is_symlink()
    {
        return Err(BrowserBundleError::InvalidAsset(
            BROWSER_BUNDLE_MANIFEST.into(),
        ));
    }
    let manifest: BrowserBundleManifest = serde_json::from_slice(&fs::read(manifest_path)?)?;
    if manifest.version != 1
        || &manifest.identity != expected_identity
        || !valid_digest(&expected_identity.runtime_source_sha256)
        || expected_identity.schema_sha256 != browser_schema_sha256()
    {
        return Err(BrowserBundleError::IdentityMismatch);
    }
    let mut assets = BTreeMap::new();
    fn read_assets(
        root: &Path,
        directory: &Path,
        assets: &mut BTreeMap<String, Arc<[u8]>>,
    ) -> Result<(), BrowserBundleError> {
        for entry in fs::read_dir(directory)? {
            let entry = entry?;
            let path = entry.path();
            let relative = path
                .strip_prefix(root)
                .expect("inventory remains below root");
            let name = relative
                .to_str()
                .ok_or_else(|| BrowserBundleError::InvalidAsset("non-UTF8 path".into()))?;
            if !assets::valid_asset_path(name) {
                return Err(BrowserBundleError::InvalidAsset(name.into()));
            }
            let kind = entry.file_type()?;
            if kind.is_dir() {
                read_assets(root, &path, assets)?;
            } else if kind.is_file() {
                if name != BROWSER_BUNDLE_MANIFEST {
                    assets.insert(name.to_owned(), Arc::from(fs::read(path)?));
                }
            } else {
                return Err(BrowserBundleError::InvalidAsset(name.into()));
            }
        }
        Ok(())
    }
    read_assets(root, root, &mut assets)?;
    if !assets.contains_key("index.html") || assets.len() != manifest.assets.len() {
        return Err(BrowserBundleError::InvalidAsset(
            "incomplete asset inventory".into(),
        ));
    }
    for (name, bytes) in &assets {
        if manifest
            .assets
            .get(name)
            .is_none_or(|digest| !valid_digest(digest) || *digest != sha256(bytes))
        {
            return Err(BrowserBundleError::AssetMismatch(name.clone()));
        }
    }
    Ok(VerifiedBrowserBundle {
        identity: manifest.identity,
        assets,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::to_bytes;

    #[tokio::test]
    async fn bundle_admission_refuses_stale_identity_and_changed_assets_and_serves_verified_bytes()
    {
        let root = std::env::temp_dir().join(format!("harness-bundle-{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(&root).unwrap();
        fs::write(root.join("index.html"), "matched browser").unwrap();
        let expected = BrowserBundleIdentity::for_runtime_source("a".repeat(64));
        let mut manifest = BrowserBundleManifest {
            version: 1,
            identity: expected.clone(),
            assets: BTreeMap::from([("index.html".into(), sha256(b"matched browser"))]),
        };
        let write_manifest = |manifest: &BrowserBundleManifest| {
            fs::write(
                root.join(BROWSER_BUNDLE_MANIFEST),
                serde_json::to_vec(manifest).unwrap(),
            )
            .unwrap()
        };
        write_manifest(&manifest);
        let bundle = verify_browser_bundle(&root, &expected).unwrap();
        // A different declared web source changes the composition identity even
        // when the compiled runtime DTOs and schema are unchanged.
        let revised_source = BrowserBundleIdentity::from_declared_source(
            &serde_json::to_vec(&serde_json::json!({"runtimeSourceSha256": "b".repeat(64)}))
                .unwrap(),
        )
        .unwrap();
        assert_eq!(revised_source.schema_sha256, expected.schema_sha256);
        assert!(matches!(
            verify_browser_bundle(&root, &revised_source),
            Err(BrowserBundleError::IdentityMismatch)
        ));
        manifest.identity.runtime_source_sha256 = "b".repeat(64);
        write_manifest(&manifest);
        assert!(matches!(
            verify_browser_bundle(&root, &expected),
            Err(BrowserBundleError::IdentityMismatch)
        ));
        manifest.identity = expected.clone();
        manifest.identity.schema_sha256 = "c".repeat(64);
        write_manifest(&manifest);
        assert!(matches!(
            verify_browser_bundle(&root, &expected),
            Err(BrowserBundleError::IdentityMismatch)
        ));
        manifest.identity = expected.clone();
        write_manifest(&manifest);
        fs::write(root.join("index.html"), "changed browser").unwrap();
        assert!(matches!(
            verify_browser_bundle(&root, &expected),
            Err(BrowserBundleError::AssetMismatch(_))
        ));
        assert_eq!(
            to_bytes(bundle.response("index.html").into_body(), usize::MAX)
                .await
                .unwrap(),
            "matched browser"
        );
        assert_eq!(
            bundle.response("../outside").status(),
            StatusCode::BAD_REQUEST
        );
        fs::remove_dir_all(root).unwrap();
    }
}
