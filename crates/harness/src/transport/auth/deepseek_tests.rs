use super::*;
use crate::transport::CredentialFailure;
use std::os::unix::fs::PermissionsExt;

struct Fixture(std::path::PathBuf);
impl Fixture {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!("deepseek-auth-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir(&path).unwrap();
        Self(path)
    }
    fn key(&self, bytes: &[u8], mode: u32) -> PathBuf {
        let path = self.0.join("synthetic-key");
        std::fs::write(&path, bytes).unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(mode)).unwrap();
        path
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        std::fs::remove_dir_all(&self.0).unwrap();
    }
}

#[test]
fn explicit_file_source_is_private_bounded_and_rotation_is_source_authority() {
    let fixture = Fixture::new();
    let path = fixture.key(b"synthetic-first\n", 0o600);
    let auth = DeepSeekAuth::new(path.clone(), InferenceLimits::deepseek_default()).unwrap();
    let contract = auth.admission().unwrap().contract().clone();
    assert_eq!(auth.access().unwrap(), "synthetic-first");
    fixture.key(b"synthetic-rotated\r\n", 0o600);
    let restored = DeepSeekAuth::from_admission(
        NativeTransportAdmission::rehydrate(contract.clone()).unwrap(),
    )
    .unwrap();
    assert_eq!(restored.admission().unwrap().contract(), &contract);
    assert_eq!(restored.access().unwrap(), "synthetic-rotated");
    for (bytes, mode, expected) in [
        (b"synthetic".to_vec(), 0o644, CredentialFailure::Unsafe),
        (b"two\nlines".to_vec(), 0o600, CredentialFailure::Invalid),
        (vec![b'x'; 16_385], 0o600, CredentialFailure::Invalid),
        (vec![0xff], 0o600, CredentialFailure::Invalid),
        (vec![], 0o600, CredentialFailure::Invalid),
    ] {
        fixture.key(&bytes, mode);
        assert!(
            matches!(auth.access(), Err(TransportError::CredentialUnavailable { route: ResponsesRoute::DeepSeek, reason }) if reason == expected)
        );
    }
    std::fs::remove_file(&path).unwrap();
    assert!(matches!(
        auth.access(),
        Err(TransportError::CredentialUnavailable {
            reason: CredentialFailure::Missing,
            ..
        })
    ));
}

#[test]
fn selected_missing_environment_refuses_without_fallback_or_secret_in_errors() {
    let name = format!(
        "DEEPSEEK_SYNTHETIC_ABSENT_{}",
        uuid::Uuid::new_v4().simple()
    );
    let auth = DeepSeekAuth::from_reference(
        CredentialReference::Env { name },
        InferenceLimits::deepseek_default(),
    )
    .unwrap();
    assert!(matches!(
        auth.access(),
        Err(TransportError::CredentialUnavailable {
            route: ResponsesRoute::DeepSeek,
            reason: CredentialFailure::Missing
        })
    ));
    for invalid in [
        InferenceLimits {
            max_output_tokens: None,
            ..InferenceLimits::deepseek_default()
        },
        InferenceLimits {
            max_inflight: 0,
            ..InferenceLimits::deepseek_default()
        },
        InferenceLimits {
            connect_timeout_secs: 301,
            ..InferenceLimits::deepseek_default()
        },
    ] {
        assert!(matches!(
            DeepSeekAuth::new("/offline/key".into(), invalid),
            Err(TransportError::InvalidAdmission)
        ));
    }
    assert_eq!(
        format!(
            "{:?}",
            CredentialReference::File {
                path: "/private/source".into()
            }
        ),
        "CredentialReference([redacted])"
    );
}
