//! Native public-client ChatGPT-plan OAuth, independent of Codex credential files.
use super::super::{Auth, AuthCredentials, ResponsesRoute, TransportError};
use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use jsonwebtoken::{Algorithm, DecodingKey, Validation, decode, decode_header, jwk::JwkSet};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeMap,
    io::Read,
    path::{Path, PathBuf},
    time::{Duration, SystemTime, UNIX_EPOCH},
};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

#[path = "chatgpt_store.rs"]
mod store;

const ISSUER: &str = "https://auth.openai.com";
const AUTHORIZE: &str = "https://auth.openai.com/api/accounts/authorize";
const TOKEN: &str = "https://auth.openai.com/api/accounts/oauth/token";
const DISCOVERY: &str = "https://auth.openai.com/.well-known/openid-configuration";
const JWKS: &str = "https://auth.openai.com/.well-known/jwks.json";
const RESOURCE: &str = "https://api.openai.com/v1";
const DYNAMIC_CLIENT: &str = "dynamic_agent_client";
const SCOPES: &str =
    "openid profile email offline_access resource.invoke chatgpt.tokens.use.direct";
const MAX_RESPONSE: u64 = 256 * 1024;

/// Errors deliberately contain no provider text, credentials, or authorization URLs.
#[derive(Debug, thiserror::Error)]
pub enum LoginError {
    #[error("native ChatGPT credential storage is unavailable or unsafe")]
    Storage,
    #[error("native ChatGPT credentials are invalid; sign in again")]
    InvalidCredentials,
    #[error("could not generate the sign-in transaction")]
    Random,
    #[error("could not bind the sign-in callback listener")]
    Listener,
    #[error("sign-in callback timed out")]
    Timeout,
    #[error("sign-in callback was invalid")]
    Callback,
    #[error("ChatGPT sign-in was denied")]
    Denied,
    #[error("ChatGPT token endpoint rejected the request; sign in again")]
    Rejected,
    #[error("ChatGPT authentication service is unavailable")]
    Network,
    #[error("ChatGPT identity verification failed")]
    Identity,
    #[error("ChatGPT plan permissions were not granted")]
    Permissions,
    #[error("the selected registration changed during sign-in; retry")]
    ConcurrentChange,
}

/// Refreshes only the selected native registration under its process-shared lock.
#[derive(Clone)]
pub struct ChatGptPlanAuth {
    path: PathBuf,
}
impl ChatGptPlanAuth {
    pub fn new(path: PathBuf) -> Self {
        Self { path }
    }
    pub fn default_path() -> Result<PathBuf, TransportError> {
        config_root()
            .map(|p| p.join("accounts/default.json"))
            .map_err(|_| TransportError::Authentication)
    }
}
impl Auth for ChatGptPlanAuth {
    fn access(&self) -> Result<(String, String), TransportError> {
        Err(TransportError::Authentication)
    }
    fn route(&self) -> ResponsesRoute {
        ResponsesRoute::ChatGptPlan
    }
    fn credentials(&self) -> Result<AuthCredentials, TransportError> {
        credentials(&self.path, &OfficialService)
            .map(|access_token| AuthCredentials::ChatGptPlan { access_token })
            .map_err(|_| TransportError::Authentication)
    }
}

fn config_root() -> Result<PathBuf, LoginError> {
    let home = std::env::var_os("HOME").ok_or(LoginError::Storage)?;
    let path = PathBuf::from(home);
    if !path.is_absolute() {
        return Err(LoginError::Storage);
    }
    Ok(path.join(".config/exomonad/chatgpt"))
}
fn now() -> Result<u64, LoginError> {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .map_err(|_| LoginError::InvalidCredentials)
}
fn random() -> Result<String, LoginError> {
    let mut bytes = [0u8; 32];
    getrandom::fill(&mut bytes).map_err(|_| LoginError::Random)?;
    Ok(URL_SAFE_NO_PAD.encode(bytes))
}
fn issued_client(id: &str) -> bool {
    !id.is_empty()
        && id != DYNAMIC_CLIENT
        && id.len() <= 512
        && !id.chars().any(char::is_whitespace)
}
fn required_scopes(scopes: &[String]) -> bool {
    [
        "openid",
        "offline_access",
        "resource.invoke",
        "chatgpt.tokens.use.direct",
    ]
    .iter()
    .all(|s| scopes.iter().any(|v| v == s))
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Record {
    version: u8,
    generation: String,
    issuer: String,
    subject: String,
    email: Option<String>,
    client_id: String,
    ext_agent_host_id: String,
    access_token: String,
    refresh_token: String,
    id_token: String,
    token_type: String,
    scopes: Vec<String>,
    expires_at: u64,
}
impl Record {
    fn valid(&self) -> bool {
        self.version == 1
            && self.issuer == ISSUER
            && !self.subject.is_empty()
            && issued_client(&self.client_id)
            && !self.access_token.is_empty()
            && !self.refresh_token.is_empty()
            && !self.id_token.is_empty()
            && self.token_type.eq_ignore_ascii_case("Bearer")
            && required_scopes(&self.scopes)
            && valid_host_id(&self.ext_agent_host_id)
            && uuid::Uuid::parse_str(&self.generation).is_ok()
    }
}
fn load(path: &Path) -> Result<Option<Record>, LoginError> {
    store::read(path)?
        .map(|bytes| {
            let record: Record =
                serde_json::from_slice(&bytes).map_err(|_| LoginError::InvalidCredentials)?;
            if !record.valid() {
                return Err(LoginError::InvalidCredentials);
            }
            Ok(record)
        })
        .transpose()
}
fn save(path: &Path, record: &Record) -> Result<(), LoginError> {
    if !record.valid() {
        return Err(LoginError::InvalidCredentials);
    }
    store::write(
        path,
        &serde_json::to_vec(record).map_err(|_| LoginError::Storage)?,
    )
}
fn valid_host_id(id: &str) -> bool {
    id.strip_prefix("urn:uuid:")
        .and_then(|v| uuid::Uuid::parse_str(v).ok())
        .is_some_and(|v| v.get_version_num() == 4)
}
fn host_id(path: &Path) -> Result<String, LoginError> {
    let _lock = store::lock(path)?;
    if let Some(bytes) = store::read(path)? {
        let id = String::from_utf8(bytes).map_err(|_| LoginError::Storage)?;
        if !valid_host_id(&id) {
            return Err(LoginError::Storage);
        }
        return Ok(id);
    }
    let id = format!("urn:uuid:{}", uuid::Uuid::new_v4());
    store::write(path, id.as_bytes())?;
    Ok(id)
}

struct Attempt {
    state: String,
    nonce: String,
    verifier: String,
    redirect: String,
    host_id: String,
    selected: Option<Record>,
}
impl Attempt {
    fn new(
        redirect: String,
        host_id: String,
        selected: Option<Record>,
    ) -> Result<Self, LoginError> {
        Ok(Self {
            state: random()?,
            nonce: random()?,
            verifier: random()?,
            redirect,
            host_id,
            selected,
        })
    }
    fn authorization_url(&self) -> String {
        let mut url = reqwest::Url::parse(AUTHORIZE).expect("fixed OAuth URL");
        let mut query = url.query_pairs_mut();
        query
            .append_pair(
                "client_id",
                self.selected
                    .as_ref()
                    .map_or(DYNAMIC_CLIENT, |r| &r.client_id),
            )
            .append_pair("ext_agent_host_id", &self.host_id)
            .append_pair("response_type", "code")
            .append_pair("redirect_uri", &self.redirect)
            .append_pair("scope", SCOPES)
            .append_pair("resource", RESOURCE)
            .append_pair("state", &self.state)
            .append_pair("nonce", &self.nonce)
            .append_pair("code_challenge_method", "S256")
            .append_pair(
                "code_challenge",
                &URL_SAFE_NO_PAD.encode(Sha256::digest(self.verifier.as_bytes())),
            );
        if let Some(record) = &self.selected {
            // The CLI presents this URL as text: a retained ID-token hint must never be printed.
            if let Some(email) = &record.email {
                query.append_pair("login_hint", email);
            }
        } else {
            query.append_pair("agent_name_hint", "Exomonad");
        }
        drop(query);
        url.into()
    }
}
struct Callback {
    code: String,
    client_id: String,
}
fn callback(target: &str, attempt: &Attempt) -> Result<Callback, LoginError> {
    if target.len() > 8192 || target.contains('#') || !target.starts_with("/auth/callback?") {
        return Err(LoginError::Callback);
    }
    let raw = target.as_bytes();
    for (i, b) in raw.iter().enumerate() {
        if *b == b'%'
            && !(raw.get(i + 1).is_some_and(u8::is_ascii_hexdigit)
                && raw.get(i + 2).is_some_and(u8::is_ascii_hexdigit))
        {
            return Err(LoginError::Callback);
        }
    }
    let url = reqwest::Url::parse(&format!("http://127.0.0.1{target}"))
        .map_err(|_| LoginError::Callback)?;
    let mut fields = BTreeMap::new();
    for (key, value) in url.query_pairs() {
        if fields
            .insert(key.into_owned(), value.into_owned())
            .is_some()
        {
            return Err(LoginError::Callback);
        }
    }
    if fields.get("state") != Some(&attempt.state) {
        return Err(LoginError::Callback);
    }
    if fields.contains_key("error") {
        return Err(LoginError::Denied);
    }
    let code = fields
        .remove("code")
        .filter(|v| !v.is_empty())
        .ok_or(LoginError::Callback)?;
    let client_id = match (&attempt.selected, fields.remove("client_id")) {
        (Some(r), None) => r.client_id.clone(),
        (Some(r), Some(id)) if id == r.client_id => id,
        (None, Some(id)) if issued_client(&id) => id,
        _ => return Err(LoginError::Callback),
    };
    Ok(Callback { code, client_id })
}

/// Starts a loopback listener before presenting the URL. No browser is launched.
/// Remote operators must arrange an SSH loopback tunnel or sign in locally and securely transfer native credentials.
pub async fn login(
    credential_file: PathBuf,
    port: u16,
    authorization_url: impl FnOnce(&str),
) -> Result<(), LoginError> {
    let host_path = config_root()?.join("host-id");
    let path = credential_file.clone();
    let (selected, host) = tokio::task::spawn_blocking(move || {
        let _lock = store::lock(&path)?;
        Ok::<_, LoginError>((load(&path)?, host_id(&host_path)?))
    })
    .await
    .map_err(|_| LoginError::Storage)??;
    let listener = tokio::net::TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, port))
        .await
        .map_err(|_| LoginError::Listener)?;
    let port = listener
        .local_addr()
        .map_err(|_| LoginError::Listener)?
        .port();
    let attempt = Attempt::new(
        format!("http://127.0.0.1:{port}/auth/callback"),
        host,
        selected,
    )?;
    authorization_url(&attempt.authorization_url());
    let received = tokio::time::timeout(
        Duration::from_secs(600),
        receive_callback(&listener, port, &attempt),
    )
    .await
    .map_err(|_| LoginError::Timeout)??;
    drop(listener);
    tokio::task::spawn_blocking(move || {
        finish_login(&credential_file, attempt, received, &OfficialService)
    })
    .await
    .map_err(|_| LoginError::Network)?
}
async fn receive_callback(
    listener: &tokio::net::TcpListener,
    port: u16,
    attempt: &Attempt,
) -> Result<Callback, LoginError> {
    let (mut stream, peer) = listener.accept().await.map_err(|_| LoginError::Callback)?;
    if !peer.ip().is_loopback() {
        return Err(LoginError::Callback);
    }
    let result = tokio::time::timeout(Duration::from_secs(5), async {
        let mut bytes = Vec::new();
        loop {
            let mut chunk = [0u8; 1024];
            let n = stream
                .read(&mut chunk)
                .await
                .map_err(|_| LoginError::Callback)?;
            if n == 0 || bytes.len() + n > 8192 {
                return Err(LoginError::Callback);
            }
            bytes.extend_from_slice(&chunk[..n]);
            if bytes.windows(4).any(|w| w == b"\r\n\r\n") {
                break;
            }
        }
        let header_end = bytes
            .windows(4)
            .position(|w| w == b"\r\n\r\n")
            .ok_or(LoginError::Callback)?
            + 4;
        if header_end != bytes.len() {
            return Err(LoginError::Callback);
        }
        let request = std::str::from_utf8(&bytes).map_err(|_| LoginError::Callback)?;
        let mut lines = request.split("\r\n");
        let line = lines.next().ok_or(LoginError::Callback)?;
        let parts: Vec<_> = line.split(' ').collect();
        if parts.len() != 3 || parts[0] != "GET" || parts[2] != "HTTP/1.1" {
            return Err(LoginError::Callback);
        }
        let mut host = None;
        for line in lines.take_while(|l| !l.is_empty()) {
            let (name, value) = line.split_once(':').ok_or(LoginError::Callback)?;
            if name.eq_ignore_ascii_case("host") {
                if host.replace(value.trim()).is_some() {
                    return Err(LoginError::Callback);
                }
            }
            if name.eq_ignore_ascii_case("transfer-encoding")
                || (name.eq_ignore_ascii_case("content-length") && value.trim() != "0")
            {
                return Err(LoginError::Callback);
            }
        }
        if host != Some(format!("127.0.0.1:{port}").as_str()) {
            return Err(LoginError::Callback);
        }
        callback(parts[1], attempt)
    })
    .await
    .map_err(|_| LoginError::Timeout)?;
    let message = if result.is_ok() {
        "Callback received. Return to Exomonad to complete sign-in."
    } else {
        "Sign-in callback rejected. Return to Exomonad."
    };
    let response = format!(
        "HTTP/1.1 {}\r\nContent-Type: text/plain\r\nCache-Control: no-store\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
        if result.is_ok() {
            "200 OK"
        } else {
            "400 Bad Request"
        },
        message.len(),
        message
    );
    let _ = tokio::time::timeout(
        Duration::from_secs(1),
        stream.write_all(response.as_bytes()),
    )
    .await;
    result
}

#[derive(Deserialize)]
struct Tokens {
    access_token: String,
    refresh_token: String,
    id_token: Option<String>,
    token_type: String,
    expires_in: u64,
    scope: String,
}
#[derive(Deserialize)]
struct Identity {
    sub: String,
    email: Option<String>,
    nonce: Option<String>,
    iat: u64,
    exp: u64,
    aud: serde_json::Value,
    azp: Option<String>,
}
trait Service {
    fn token(&self, form: &[(&str, &str)]) -> Result<Tokens, LoginError>;
    fn keys(&self) -> Result<JwkSet, LoginError>;
}
struct OfficialService;
fn http() -> Result<reqwest::blocking::Client, LoginError> {
    reqwest::blocking::Client::builder()
        .timeout(Duration::from_secs(20))
        .connect_timeout(Duration::from_secs(5))
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .map_err(|_| LoginError::Network)
}
fn response<T: serde::de::DeserializeOwned>(
    response: reqwest::blocking::Response,
) -> Result<T, LoginError> {
    if response.status() != reqwest::StatusCode::OK {
        return Err(LoginError::Rejected);
    }
    let mut bytes = Vec::new();
    response
        .take(MAX_RESPONSE + 1)
        .read_to_end(&mut bytes)
        .map_err(|_| LoginError::Network)?;
    if bytes.len() as u64 > MAX_RESPONSE {
        return Err(LoginError::Network);
    }
    serde_json::from_slice(&bytes).map_err(|_| LoginError::InvalidCredentials)
}
impl Service for OfficialService {
    fn token(&self, form: &[(&str, &str)]) -> Result<Tokens, LoginError> {
        response(
            http()?
                .post(TOKEN)
                .form(form)
                .send()
                .map_err(|_| LoginError::Network)?,
        )
    }
    fn keys(&self) -> Result<JwkSet, LoginError> {
        #[derive(Deserialize)]
        struct Discovery {
            issuer: String,
            authorization_endpoint: String,
            token_endpoint: String,
            jwks_uri: String,
        }
        let client = http()?;
        let config: Discovery = response(
            client
                .get(DISCOVERY)
                .send()
                .map_err(|_| LoginError::Network)?,
        )?;
        if config.issuer != ISSUER
            || config.authorization_endpoint != AUTHORIZE
            || config.token_endpoint != TOKEN
            || config.jwks_uri != JWKS
        {
            return Err(LoginError::Identity);
        }
        response(client.get(JWKS).send().map_err(|_| LoginError::Network)?)
    }
}
fn verify(
    token: &str,
    client_id: &str,
    nonce: Option<&str>,
    expected_subject: Option<&str>,
    keys: &JwkSet,
) -> Result<Identity, LoginError> {
    let header = decode_header(token).map_err(|_| LoginError::Identity)?;
    if header.alg != Algorithm::RS256 {
        return Err(LoginError::Identity);
    }
    let kid = header.kid.ok_or(LoginError::Identity)?;
    let matching: Vec<_> = keys
        .keys
        .iter()
        .filter(|k| k.common.key_id.as_ref() == Some(&kid))
        .collect();
    if matching.len() != 1 {
        return Err(LoginError::Identity);
    }
    let key = DecodingKey::from_jwk(matching[0]).map_err(|_| LoginError::Identity)?;
    let mut validation = Validation::new(Algorithm::RS256);
    validation.set_issuer(&[ISSUER]);
    validation.set_audience(&[client_id]);
    validation.set_required_spec_claims(&["iss", "aud", "exp", "iat", "sub"]);
    validation.leeway = 5;
    validation.validate_nbf = true;
    let identity = decode::<Identity>(token, &key, &validation)
        .map_err(|_| LoginError::Identity)?
        .claims;
    let current = now()?;
    let audience_count = match &identity.aud {
        serde_json::Value::String(_) => 1,
        serde_json::Value::Array(a) => a.len(),
        _ => return Err(LoginError::Identity),
    };
    if identity.sub.is_empty()
        || identity.iat > current + 5
        || identity.exp <= current
        || identity.exp <= identity.iat
        || nonce.is_some_and(|n| identity.nonce.as_deref() != Some(n))
        || expected_subject.is_some_and(|s| identity.sub != s)
        || (audience_count > 1 && identity.azp.as_deref() != Some(client_id))
        || identity.azp.as_deref().is_some_and(|a| a != client_id)
    {
        return Err(LoginError::Identity);
    }
    Ok(identity)
}
fn apply_tokens(
    mut record: Record,
    tokens: Tokens,
    identity: Option<Identity>,
    received_at: u64,
) -> Result<Record, LoginError> {
    let current = now()?;
    if tokens.access_token.is_empty()
        || tokens.refresh_token.is_empty()
        || !tokens.token_type.eq_ignore_ascii_case("Bearer")
        || tokens.expires_in == 0
        || tokens.expires_in > 86400
    {
        return Err(LoginError::InvalidCredentials);
    }
    let scopes: Vec<String> = tokens
        .scope
        .split_ascii_whitespace()
        .map(str::to_owned)
        .collect();
    if !required_scopes(&scopes) {
        return Err(LoginError::Permissions);
    }
    let expires_at = received_at
        .checked_add(tokens.expires_in)
        .ok_or(LoginError::InvalidCredentials)?;
    if received_at > current || expires_at <= current {
        return Err(LoginError::InvalidCredentials);
    }
    if let Some(identity) = identity {
        record.subject = identity.sub;
        record.email = identity.email;
    }
    if let Some(id_token) = tokens.id_token {
        record.id_token = id_token;
    }
    record.access_token = tokens.access_token;
    record.refresh_token = tokens.refresh_token;
    record.token_type = tokens.token_type;
    record.scopes = scopes;
    record.expires_at = expires_at;
    record.generation = uuid::Uuid::new_v4().to_string();
    if !record.valid() {
        return Err(LoginError::InvalidCredentials);
    }
    Ok(record)
}
fn finish_login(
    path: &Path,
    attempt: Attempt,
    callback: Callback,
    service: &impl Service,
) -> Result<(), LoginError> {
    let tokens = service.token(&[
        ("grant_type", "authorization_code"),
        ("client_id", &callback.client_id),
        ("code", &callback.code),
        ("code_verifier", &attempt.verifier),
        ("redirect_uri", &attempt.redirect),
        ("resource", RESOURCE),
    ])?;
    let received_at = now()?;
    let identity = verify(
        tokens.id_token.as_deref().ok_or(LoginError::Identity)?,
        &callback.client_id,
        Some(&attempt.nonce),
        attempt.selected.as_ref().map(|r| r.subject.as_str()),
        &service.keys()?,
    )?;
    let record = apply_tokens(
        Record {
            version: 1,
            generation: String::new(),
            issuer: ISSUER.into(),
            subject: String::new(),
            email: None,
            client_id: callback.client_id,
            ext_agent_host_id: attempt.host_id,
            access_token: String::new(),
            refresh_token: String::new(),
            id_token: String::new(),
            token_type: String::new(),
            scopes: vec![],
            expires_at: 0,
        },
        tokens,
        Some(identity),
        received_at,
    )?;
    let _lock = store::lock(path)?;
    let latest = load(path)?;
    if latest.as_ref().map(|r| &r.generation) != attempt.selected.as_ref().map(|r| &r.generation) {
        return Err(LoginError::ConcurrentChange);
    }
    save(path, &record)
}
fn credentials(path: &Path, service: &impl Service) -> Result<String, LoginError> {
    let _lock = store::lock(path)?;
    let mut record = load(path)?.ok_or(LoginError::InvalidCredentials)?;
    let current = now()?;
    // Refresh at expiry: the provider does not document earliest_refresh_at encoding.
    if record.expires_at <= current {
        let tokens = service.token(&[
            ("grant_type", "refresh_token"),
            ("client_id", &record.client_id),
            ("refresh_token", &record.refresh_token),
            ("resource", RESOURCE),
        ])?;
        let received_at = now()?;
        let identity = tokens
            .id_token
            .as_deref()
            .map(|token| {
                verify(
                    token,
                    &record.client_id,
                    None,
                    Some(&record.subject),
                    &service.keys()?,
                )
            })
            .transpose()?;
        record = apply_tokens(record, tokens, identity, received_at)?;
        save(path, &record)?;
    }
    if record.expires_at <= now()? {
        return Err(LoginError::InvalidCredentials);
    }
    Ok(record.access_token)
}

#[cfg(test)]
#[path = "chatgpt_tests.rs"]
mod tests;
