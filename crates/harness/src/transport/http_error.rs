use super::HttpDiagnostic;
use futures_util::{Stream, StreamExt};
use serde_json::Value;
use std::time::Duration;

const BODY_LIMIT: usize = 16 * 1024;
const READ_TIMEOUT: Duration = Duration::from_millis(500);
const FIELD_LIMIT: usize = 256;
const MESSAGE_LIMIT: usize = 2048;

pub(super) async fn read(
    response: reqwest::Response,
    token: &str,
    account: &str,
) -> Option<HttpDiagnostic> {
    read_stream(response.bytes_stream(), token, account).await
}

async fn read_stream<S, E, B>(stream: S, token: &str, account: &str) -> Option<HttpDiagnostic>
where
    S: Stream<Item = Result<B, E>>,
    B: AsRef<[u8]>,
{
    // The deadline covers the entire body, including a peer that keeps sending
    // small chunks. Partial, oversized and invalid JSON bodies give status only.
    tokio::time::timeout(READ_TIMEOUT, async {
        futures_util::pin_mut!(stream);
        let mut body = Vec::new();
        while let Some(chunk) = stream.next().await {
            let chunk = chunk.ok()?;
            let chunk = chunk.as_ref();
            if chunk.len() > BODY_LIMIT.saturating_sub(body.len()) {
                return None;
            }
            body.extend_from_slice(chunk);
        }
        parse(&body, token, account)
    })
    .await
    .ok()
    .flatten()
}

fn parse(body: &[u8], token: &str, account: &str) -> Option<HttpDiagnostic> {
    if body.len() > BODY_LIMIT {
        return None;
    }
    let document: Value = serde_json::from_slice(body).ok()?;
    let error = document.get("error").and_then(Value::as_object);
    // The Codex backend also reports account/model refusals as a top-level
    // detail string. Both known envelopes use the same bounded redaction path.
    let bounded = |text: &str, limit| redact(text, token, account).chars().take(limit).collect();
    let field = |name: &str, limit| {
        error?.get(name)?.as_str().map(|text| {
            // Redact before truncation, so a cutoff cannot leave a credential
            // prefix visible. Unrecognized fields never enter the diagnostic.
            bounded(text, limit)
        })
    };
    let diagnostic = HttpDiagnostic {
        code: field("code", FIELD_LIMIT),
        error_type: field("type", FIELD_LIMIT),
        param: field("param", FIELD_LIMIT),
        message: field("message", MESSAGE_LIMIT).or_else(|| {
            document
                .get("detail")?
                .as_str()
                .map(|text| bounded(text, MESSAGE_LIMIT))
        }),
    };
    (diagnostic != HttpDiagnostic::default()).then_some(diagnostic)
}

fn redact(text: &str, token: &str, account: &str) -> String {
    let mut text = text.to_owned();
    for secret in [token, account] {
        if !secret.is_empty() {
            text = text.replace(secret, "[redacted]");
        }
    }
    // Common credential shapes are removed even when they differ from the
    // active request's token. Keep punctuation and schema paths readable.
    let mut output = String::new();
    let mut bearer = false;
    for part in text.split_inclusive(|c: char| !c.is_ascii_alphanumeric() && !"_-./+=".contains(c))
    {
        let word =
            part.trim_end_matches(|c: char| !c.is_ascii_alphanumeric() && !"_-./+=".contains(c));
        let delimiter = &part[word.len()..];
        if word.is_empty() {
            output.extend(
                delimiter
                    .chars()
                    .map(|c| if c.is_control() { ' ' } else { c }),
            );
            continue;
        }
        let secret_shape = word.starts_with("sk-")
            || word.starts_with("acct_")
            || word.starts_with("account-")
            || word.starts_with("org-")
            || (word.starts_with("eyJ") && word.matches('.').count() == 2);
        if bearer || secret_shape {
            output.push_str("[redacted]");
        } else {
            output.extend(word.chars().map(|c| if c.is_control() { ' ' } else { c }));
        }
        bearer = word.eq_ignore_ascii_case("bearer");
        output.extend(
            delimiter
                .chars()
                .map(|c| if c.is_control() { ' ' } else { c }),
        );
    }
    output
}

#[cfg(test)]
mod tests {
    use super::*;
    use futures_util::stream;
    use serde_json::json;

    #[test]
    fn diagnostics_read_observed_codex_account_model_detail_without_unknown_fields() {
        let body = br#"{"detail":"The 'gpt-6.1-sol' model is not supported when using Codex with a ChatGPT account."}"#;
        let diagnostic = parse(body, "", "").unwrap();
        assert_eq!(diagnostic, HttpDiagnostic {
            message:Some("The 'gpt-6.1-sol' model is not supported when using Codex with a ChatGPT account.".into()),
            ..HttpDiagnostic::default()
        });
        let token = "test-request-token";
        let account = "test-request-account";
        let body = serde_json::to_vec(&json!({
            "detail":format!("{token} {account} Bearer other-token sk-other {}{token}","λ".repeat(2048)),
            "unknown":token,"request":{"account":account}
        })).unwrap();
        let diagnostic = parse(&body, token, account).unwrap();
        assert_eq!(
            diagnostic.message.as_ref().unwrap().chars().count(),
            MESSAGE_LIMIT
        );
        let serialized = serde_json::to_string(&diagnostic).unwrap();
        for secret in [token, account, "other-token", "sk-other"] {
            assert!(!serialized.contains(secret), "credential was retained");
        }
        let nested = serde_json::to_vec(&json!({"error":{"message":"specific provider message","code":"provider_code"},"detail":"fallback detail"})).unwrap();
        let diagnostic = parse(&nested, "", "").unwrap();
        assert_eq!(
            diagnostic.message.as_deref(),
            Some("specific provider message")
        );
        assert_eq!(diagnostic.code.as_deref(), Some("provider_code"));
        for body in [
            br#"{"detail":{"message":"unknown nested shape"}}"#.as_slice(),
            br#"{"detail":["unknown array"]}"#,
            br#"{"detail":42}"#,
            br#"{"unknown":"unrecognized text"}"#,
        ] {
            assert!(parse(body, "", "").is_none());
        }
    }

    #[test]
    fn diagnostics_allowlist_fields_and_redact_before_bounds() {
        let token = "test-request-token";
        let account = "test-request-account";
        let body = serde_json::to_vec(&json!({"error": {
            "code": "invalid_function_parameters",
            "type": "invalid_request_error",
            "param": "tools[1].parameters",
            "message": format!("view missing required; {token} {account} Bearer alternate-token sk-alternate eyJheader.payload.signature acct_other {}{token}", "x".repeat(2040)),
            "request": {"token": token}, "unknown": account
        }})).unwrap();
        let diagnostic = parse(&body, token, account).unwrap();
        assert_eq!(
            diagnostic.code.as_deref(),
            Some("invalid_function_parameters")
        );
        assert_eq!(diagnostic.param.as_deref(), Some("tools[1].parameters"));
        let serialized = serde_json::to_string(&diagnostic).unwrap();
        assert_eq!(
            serde_json::to_value(&diagnostic).unwrap()["error_type"],
            "invalid_request_error"
        );
        for secret in [
            token,
            account,
            "alternate-token",
            "sk-alternate",
            "eyJheader",
            "acct_other",
        ] {
            assert!(!serialized.contains(secret), "credential was retained");
        }
        assert_eq!(
            diagnostic.message.as_ref().unwrap().chars().count(),
            MESSAGE_LIMIT
        );
        assert!(!serialized.contains("unknown"));
        assert!(
            serde_json::to_value(&diagnostic)
                .unwrap()
                .get("request")
                .is_none()
        );
        let error = crate::transport::TransportError::Http {
            status: 400,
            diagnostic: Some(diagnostic),
        };
        assert!(error.to_string().contains("invalid_function_parameters"));
        let status_only = crate::transport::TransportError::Http {
            status: 503,
            diagnostic: None,
        };
        assert_eq!(status_only.to_string(), "terminal HTTP status 503");
    }

    #[test]
    fn diagnostics_fall_back_without_retaining_unstructured_bodies() {
        for body in [
            b"secret HTML page".as_slice(),
            b"{\"error\":\"secret\"}",
            b"{\"error\":{\"unknown\":\"secret\",\"message\":42}}",
            b"{\"error\":{\"message\":\"partial\"}",
        ] {
            assert!(parse(body, "", "").is_none());
        }
        assert!(parse(&vec![b'x'; BODY_LIMIT + 1], "", "").is_none());
        let body = serde_json::to_vec(
            &json!({"error":{"message":"λ\n".repeat(1500), "param":"p".repeat(400)}}),
        )
        .unwrap();
        let diagnostic = parse(&body, "", "").unwrap();
        assert_eq!(diagnostic.param.unwrap().chars().count(), FIELD_LIMIT);
        assert_eq!(
            diagnostic.message.as_ref().unwrap().chars().count(),
            MESSAGE_LIMIT
        );
        assert!(!diagnostic.message.unwrap().contains('\n'));
    }

    #[tokio::test]
    async fn diagnostics_bound_stream_size_errors_and_read_time() {
        let bytes = b"{\"error\":{\"code\":\"bad\"}}".to_vec();
        let diagnostic = read_stream(stream::iter([Ok::<_, ()>(bytes)]), "", "")
            .await
            .unwrap();
        assert_eq!(diagnostic.code.as_deref(), Some("bad"));
        assert!(
            read_stream(
                stream::iter([Ok::<_, ()>(vec![b'x'; BODY_LIMIT + 1])]),
                "",
                ""
            )
            .await
            .is_none()
        );
        assert!(
            read_stream(stream::iter([Err::<Vec<u8>, _>(())]), "", "")
                .await
                .is_none()
        );
        assert!(
            read_stream(stream::pending::<Result<Vec<u8>, ()>>(), "", "")
                .await
                .is_none()
        );
    }
}
