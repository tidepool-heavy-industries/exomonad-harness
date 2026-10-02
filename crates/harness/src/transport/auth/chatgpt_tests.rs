use super::*;
use std::{
    os::unix::fs::PermissionsExt,
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
};
// Test-only RSA fixture, never used in production.
const KEY: &str = "MIIEogIBAAKCAQEAjplzBdZ_5Ka2BuBSfLR2Gx6r0NMTMHfZ7SV55XDBePm6oVsyThYLFr16knrO9pVhIToiFtRfYqxZcjax8K7uXLHOPrl-IBjGmlcNtWrp2y33nfGbw6xdVwvYqnEkfOYedrZl-Iou0BkE_r7W1AHRwK6uzy0N__i4LNRNOKei5YrVI10rY0MYieu-92WYnZ2rGzda2rPO9M99nsp-n-3waS-sd_-YjaJIp67wP408yzprN-Skqwxb2b2cCL_xsyunUEWHNA_cEuP_uS_4NyQo6kSTHr3Uv4ei31SYUe9nX_ZBJvREwdki3-0wmbN0RVaDC0gwiYXOmRkIk6_9HUsdGwIDAQABAoIBAAmWO2trDlIa-yXfMux7XW1ZVIfHiQS7KgWNfXJ1cCxn7aonbWxegwRWXMU4SG2_gTYwqFYMlRUXLTRGhRP_TrsgoRMKeXZC9CcwLxXBSFOZ9YnC0owHLIdLnpdm3-MdvXKRtK1O7PLEm9dckSjTrnxbwhjfmI9a81bgNwjWDFeOMi3cz78JHyl6yqTyRl-SaKO5pbFbiXem_NSDE4e_G3zhgGOsrxhJnz6nKYcb6lPoH2hEIusasu048pxoqvAu6jQXGZY7UVjxgCra6CNuObZp8pDc-fVBlpwqrfcey6cjajByED1IAMQ8GihzGTJgbdih1FmPjGQEWcdn3GULTTECgYEAxV7Bvv0x4zm1t1nsMeAfL428Lx-enUzR5pKgW2DG4rMYnic5OqgUKjJCRdTTSQaa-IFMUl68fhmL8E_MXMg0GrPHFlmQGhU9wxj8WLRysFAYI0fCbxfJHFl4Yq7B0ZI5R09xeI8eoIKic36FLcWjMUGgQsNHVxoHR8F5rCt60g0CgYEAuPWWkMehDwwAVfPgoYyGuI1257FABpdYyHa67UeZNibAaMVh67oNgcxSgk9zVQrI22Q7Z5uWDWZCLn2c0HtNZwTFOoIhbHLZ4jKOyVMk6aH6pOX_w-JWh5U9lO_yCw77xAwFX1obp2uI4rl-03x37PnmY9_Qp9zriJeMqxE06ccCgYAGkj8VIsz0acl5D09j4bhoFun7D6xyREqAyMT6BeDZT2k0as3m_A2f0giO1qUqO0QRngxyeaEA-czE9YMyW6AQe4fXYKgBlk92HXDZazieUixbkFoS5NHXVctCTds6JQovK5_1iZ5VbcQG4GGCwp_KVgsF7gaECePQKcrpRpFSSQKBgENMaNqJKJs2_LBJqoRdg2-HWap4HhnH2_Ak82L-2EqR0xTMLRL-gYem9qafjhF1eRwK3mqWfASoHpCX-AULuGAxpinhy5OQPqNFThsG-7lezLpPTb7SjjWLIfsdS26mpwjwbswBF2rVf9svL2x4L5K0YxYYC-3oPnNW4UIlYqFlAoGAC8hxVGINTFF5ObiMEJI3z7Km0X8n8_ExiaB2PtsHX8_V6tD_Rt-4RtpiQ4dHVuVial5Gb0h-xF50pUM5SUD4nN_7vv4H6oa5OmqtA3mF6FpO37I4yTMpGxnQUQ842ne4GGkdNto2cuYrbHY-0jPeyn8FWZ1NccjogzzVb8WY0qQ";
const MODULUS: &str = "jplzBdZ_5Ka2BuBSfLR2Gx6r0NMTMHfZ7SV55XDBePm6oVsyThYLFr16knrO9pVhIToiFtRfYqxZcjax8K7uXLHOPrl-IBjGmlcNtWrp2y33nfGbw6xdVwvYqnEkfOYedrZl-Iou0BkE_r7W1AHRwK6uzy0N__i4LNRNOKei5YrVI10rY0MYieu-92WYnZ2rGzda2rPO9M99nsp-n-3waS-sd_-YjaJIp67wP408yzprN-Skqwxb2b2cCL_xsyunUEWHNA_cEuP_uS_4NyQo6kSTHr3Uv4ei31SYUe9nX_ZBJvREwdki3-0wmbN0RVaDC0gwiYXOmRkIk6_9HUsdGw";
struct Temp(PathBuf);
impl Temp {
    fn new() -> Self {
        let p = std::env::temp_dir().join(format!("native-auth-test-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir(&p).unwrap();
        Self(p)
    }
    fn path(&self) -> PathBuf {
        self.0.join("account.json")
    }
}
impl Drop for Temp {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}
fn keys() -> JwkSet {
    serde_json::from_value(serde_json::json!({"keys":[{"kty":"RSA","kid":"test","alg":"RS256","use":"sig","n":MODULUS,"e":"AQAB"}]})).unwrap()
}
fn jwt(subject: &str, nonce: &str, aud: &str, issuer: &str, exp: u64) -> String {
    let mut h = jsonwebtoken::Header::new(Algorithm::RS256);
    h.kid = Some("test".into());
    jsonwebtoken::encode(&h,&serde_json::json!({"iss":issuer,"aud":aud,"sub":subject,"nonce":nonce,"iat":now().unwrap()-10,"exp":exp,"email":"test@example.test"}),&jsonwebtoken::EncodingKey::from_rsa_der(&URL_SAFE_NO_PAD.decode(KEY).unwrap())).unwrap()
}
fn record(exp: u64) -> Record {
    Record {
        version: 1,
        generation: uuid::Uuid::new_v4().to_string(),
        issuer: ISSUER.into(),
        subject: "subject".into(),
        email: None,
        client_id: "oaiapp_test".into(),
        ext_agent_host_id: format!("urn:uuid:{}", uuid::Uuid::new_v4()),
        access_token: "old-access".into(),
        refresh_token: "old-refresh".into(),
        id_token: jwt(
            "subject",
            "nonce",
            "oaiapp_test",
            ISSUER,
            now().unwrap() + 600,
        ),
        token_type: "Bearer".into(),
        scopes: SCOPES.split_whitespace().map(str::to_owned).collect(),
        expires_at: exp,
    }
}
fn attempt(selected: Option<Record>) -> Attempt {
    Attempt {
        state: "state".into(),
        nonce: "nonce".into(),
        verifier: "verifier".into(),
        redirect: "http://127.0.0.1:1455/auth/callback".into(),
        host_id: format!("urn:uuid:{}", uuid::Uuid::new_v4()),
        selected,
    }
}
struct Fake {
    calls: Arc<AtomicUsize>,
    subject: &'static str,
    valid: bool,
}
fn fake(subject: &'static str, valid: bool) -> Fake {
    Fake {
        calls: Arc::new(AtomicUsize::new(0)),
        subject,
        valid,
    }
}
impl Service for Fake {
    fn token(&self, form: &[(&str, &str)]) -> Result<Tokens, LoginError> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        assert!(form.contains(&("resource", RESOURCE)));
        assert!(!form.iter().any(|(k, _)| *k == "scope"));
        if !self.valid {
            return Err(LoginError::Rejected);
        }
        if form.contains(&("grant_type", "authorization_code")) {
            assert!(form.contains(&("redirect_uri", "http://127.0.0.1:1455/auth/callback")));
            assert!(form.contains(&("code_verifier", "verifier")));
        }
        Ok(Tokens {
            access_token: "new-access".into(),
            refresh_token: "new-refresh".into(),
            id_token: Some(jwt(
                self.subject,
                "nonce",
                "oaiapp_test",
                ISSUER,
                now().unwrap() + 3600,
            )),
            token_type: "Bearer".into(),
            expires_in: 3600,
            scope: SCOPES.into(),
        })
    }
    fn keys(&self) -> Result<JwkSet, LoginError> {
        Ok(keys())
    }
}
#[test]
fn callback_rejects_unbound_state_duplicates_errors_and_clients() {
    let a = attempt(None);
    for target in [
        "/auth/callback?code=c&client_id=oaiapp_test",
        "/auth/callback?state=wrong&code=c&client_id=oaiapp_test",
        "/auth/callback?state=state&state=state&code=c&client_id=oaiapp_test",
        "/auth/callback?state=state&code=c",
        "/auth/callback?state=state&code=c&client_id=dynamic_agent_client",
        "/callback?state=state&code=c&client_id=oaiapp_test",
        "/auth/callback?state=state&code=%QQ&client_id=oaiapp_test",
    ] {
        assert!(matches!(callback(target, &a), Err(LoginError::Callback)));
    }
    assert!(matches!(
        callback("/auth/callback?state=state&error=access_denied", &a),
        Err(LoginError::Denied)
    ));
    assert!(
        callback(
            "/auth/callback?state=state&code=c&client_id=oaiapp_test",
            &a
        )
        .is_ok()
    );
    let a = attempt(Some(record(now().unwrap() + 600)));
    assert!(callback("/auth/callback?state=state&code=c", &a).is_ok());
    assert!(callback("/auth/callback?state=state&code=c&client_id=other", &a).is_err());
}
#[test]
fn authorization_pkce_omits_retained_token() {
    let r = record(now().unwrap() + 600);
    let token = r.id_token.clone();
    let a = attempt(Some(r));
    let url = a.authorization_url();
    let parsed = reqwest::Url::parse(&url).unwrap();
    let fields: BTreeMap<_, _> = parsed.query_pairs().collect();
    assert_eq!(fields.get("client_id").unwrap(), "oaiapp_test");
    assert!(!fields.contains_key("agent_name_hint"));
    assert!(!fields.contains_key("id_token_hint"));
    assert!(!url.contains(&token));
    assert_eq!(
        fields.get("code_challenge").unwrap().as_ref(),
        URL_SAFE_NO_PAD.encode(Sha256::digest(a.verifier.as_bytes()))
    );
    let parsed = reqwest::Url::parse(&attempt(None).authorization_url()).unwrap();
    assert!(
        parsed
            .query_pairs()
            .any(|(k, v)| k == "agent_name_hint" && v == "Exomonad")
    );
}
#[test]
fn jwt_requires_signed_issuer_audience_nonce_expiry_and_subject() {
    let t = now().unwrap();
    let good = jwt("subject", "nonce", "oaiapp_test", ISSUER, t + 600);
    assert!(
        verify(
            &good,
            "oaiapp_test",
            Some("nonce"),
            Some("subject"),
            &keys()
        )
        .is_ok()
    );
    for token in [
        jwt("subject", "bad", "oaiapp_test", ISSUER, t + 600),
        jwt("subject", "nonce", "wrong", ISSUER, t + 600),
        jwt(
            "subject",
            "nonce",
            "oaiapp_test",
            "https://invalid.example",
            t + 600,
        ),
        jwt("subject", "nonce", "oaiapp_test", ISSUER, t - 1),
        jwt("other", "nonce", "oaiapp_test", ISSUER, t + 600),
    ] {
        assert!(matches!(
            verify(
                &token,
                "oaiapp_test",
                Some("nonce"),
                Some("subject"),
                &keys()
            ),
            Err(LoginError::Identity)
        ));
    }
    let mut tampered = good.into_bytes();
    let i = tampered.len() - 10;
    tampered[i] = if tampered[i] == b'A' { b'B' } else { b'A' };
    assert!(
        verify(
            std::str::from_utf8(&tampered).unwrap(),
            "oaiapp_test",
            Some("nonce"),
            None,
            &keys()
        )
        .is_err()
    );
    assert!(
        verify(
            &jwt("subject", "nonce", "oaiapp_test", ISSUER, t + 600),
            "oaiapp_test",
            Some("nonce"),
            None,
            &JwkSet { keys: vec![] }
        )
        .is_err()
    );
}
#[test]
fn storage_is_private_atomic_and_rejects_symlinks() {
    let temp = Temp::new();
    let p = temp.path();
    save(&p, &record(now().unwrap() + 600)).unwrap();
    assert_eq!(
        std::fs::metadata(&p).unwrap().permissions().mode() & 0o777,
        0o600
    );
    let host = temp.0.join("host-id");
    let id = host_id(&host).unwrap();
    assert_eq!(host_id(&host).unwrap(), id);
    assert!(valid_host_id(&id));
    std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o644)).unwrap();
    assert!(load(&p).is_err());
    std::fs::remove_file(&p).unwrap();
    std::os::unix::fs::symlink(&host, &p).unwrap();
    assert!(load(&p).is_err());
    assert!(save(&p, &record(now().unwrap() + 600)).is_err());
    assert_eq!(host_id(&host).unwrap(), id);
}
#[test]
fn login_failure_and_identity_mismatch_preserve_selected_record() {
    let temp = Temp::new();
    let p = temp.path();
    let selected = record(now().unwrap() + 600);
    save(&p, &selected).unwrap();
    let before = std::fs::read(&p).unwrap();
    for service in [fake("other", true), fake("subject", false)] {
        assert!(
            finish_login(
                &p,
                attempt(Some(selected.clone())),
                Callback {
                    code: "code".into(),
                    client_id: "oaiapp_test".into()
                },
                &service
            )
            .is_err()
        );
        assert_eq!(std::fs::read(&p).unwrap(), before);
    }
}
#[test]
fn login_verifies_and_refuses_concurrent_replacement() {
    let temp = Temp::new();
    let p = temp.path();
    finish_login(
        &p,
        attempt(None),
        Callback {
            code: "code".into(),
            client_id: "oaiapp_test".into(),
        },
        &fake("subject", true),
    )
    .unwrap();
    let selected = load(&p).unwrap().unwrap();
    assert_eq!(selected.access_token, "new-access");
    let mut next = selected.clone();
    next.generation = uuid::Uuid::new_v4().to_string();
    save(&p, &next).unwrap();
    let before = std::fs::read(&p).unwrap();
    assert!(matches!(
        finish_login(
            &p,
            attempt(Some(selected)),
            Callback {
                code: "code".into(),
                client_id: "oaiapp_test".into()
            },
            &fake("subject", true)
        ),
        Err(LoginError::ConcurrentChange)
    ));
    assert_eq!(std::fs::read(&p).unwrap(), before);
}
#[test]
fn competing_refreshes_replace_tokens_once() {
    let temp = Temp::new();
    let p = temp.path();
    save(&p, &record(now().unwrap() - 1)).unwrap();
    let calls = Arc::new(AtomicUsize::new(0));
    let handles: Vec<_> = (0..4)
        .map(|_| {
            let p = p.clone();
            let calls = calls.clone();
            std::thread::spawn(move || {
                credentials(
                    &p,
                    &Fake {
                        calls,
                        subject: "subject",
                        valid: true,
                    },
                )
                .unwrap()
            })
        })
        .collect();
    for handle in handles {
        assert_eq!(handle.join().unwrap(), "new-access");
    }
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    let r = load(&p).unwrap().unwrap();
    assert_eq!(r.refresh_token, "new-refresh");
    assert_eq!(r.access_token, "new-access");
    assert!(r.expires_at > now().unwrap());
}
#[test]
fn failed_refresh_keeps_previous_record() {
    let temp = Temp::new();
    let p = temp.path();
    save(&p, &record(now().unwrap() - 1)).unwrap();
    let before = std::fs::read(&p).unwrap();
    for service in [fake("other", true), fake("subject", false)] {
        assert!(credentials(&p, &service).is_err());
        assert_eq!(std::fs::read(&p).unwrap(), before);
    }
    assert!(
        !format!("{} {:?}", LoginError::Rejected, LoginError::Rejected).contains("old-refresh")
    );
}
#[test]
fn tokens_require_plan_scope_and_rotating_replacement() {
    let s = fake("subject", true);
    let mut tokens = s.token(&[("resource", RESOURCE)]).unwrap();
    tokens.scope = "openid offline_access resource.invoke".into();
    assert!(matches!(
        apply_tokens(record(now().unwrap()), tokens, None),
        Err(LoginError::Permissions)
    ));
    let mut tokens = s.token(&[("resource", RESOURCE)]).unwrap();
    tokens.refresh_token.clear();
    assert!(apply_tokens(record(now().unwrap()), tokens, None).is_err());
}
#[tokio::test]
async fn callback_listener_requires_exact_loopback_host() {
    for host in ["localhost", "127.0.0.1"] {
        let listener = tokio::net::TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0))
            .await
            .unwrap();
        let port = listener.local_addr().unwrap().port();
        let a = attempt(None);
        let request = format!(
            "GET /auth/callback?state=state&code=c&client_id=oaiapp_test HTTP/1.1\r\nHost: {host}:{port}\r\n\r\n"
        );
        let client = tokio::spawn(async move {
            let mut s = tokio::net::TcpStream::connect((std::net::Ipv4Addr::LOCALHOST, port))
                .await
                .unwrap();
            s.write_all(request.as_bytes()).await.unwrap();
            let mut b = Vec::new();
            s.read_to_end(&mut b).await.unwrap();
            String::from_utf8(b).unwrap()
        });
        assert_eq!(
            receive_callback(&listener, port, &a).await.is_ok(),
            host == "127.0.0.1"
        );
        let reply = client.await.unwrap();
        assert!(!reply.contains("state=state"));
        assert!(!reply.contains("client_id"));
    }
}

#[test]
fn relative_credential_paths_create_no_artifacts() {
    let parent = PathBuf::from(format!("relative-native-auth-{}", uuid::Uuid::new_v4()));
    let path = parent.join("account.json");
    assert!(store::lock(&path).is_err());
    assert!(store::read(&path).is_err());
    assert!(save(&path, &record(now().unwrap() + 600)).is_err());
    assert!(!parent.exists());
}
