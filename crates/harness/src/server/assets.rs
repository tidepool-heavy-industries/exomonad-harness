//! Safe, self-contained static-asset responses for the Axum server boundary.
//!
//! An Axum handler can pass its configured asset root and extracted request
//! path to [`asset_response`].

use std::path::{Component, Path};

use axum::{
    body::Body,
    http::{HeaderValue, StatusCode, header},
    response::Response,
};

/// Read one static asset below `root` and return it as an HTTP response.
///
/// `path` is relative to `root` (for example, `"assets/app.js"`). Traversal,
/// absolute paths, and symlinks resolving outside the root are rejected. A
/// missing/non-file asset returns 404; filesystem errors return 500. Routing
/// and index-file policy remain the caller's responsibility.
pub async fn asset_response(root: impl AsRef<Path>, path: &str) -> Response<Body> {
    let relative = Path::new(path);
    if path.is_empty()
        || relative.is_absolute()
        || relative
            .components()
            .any(|part| !matches!(part, Component::Normal(_)))
        || path.contains('\\')
        || path.contains('\0')
    {
        return response(
            StatusCode::BAD_REQUEST,
            "invalid asset path",
            None,
            Body::empty(),
        );
    }

    let root = match tokio::fs::canonicalize(root).await {
        Ok(root) => root,
        Err(_) => return response(StatusCode::NOT_FOUND, "not found", None, Body::empty()),
    };
    let candidate = root.join(relative);
    let canonical = match tokio::fs::canonicalize(candidate).await {
        Ok(path) if path.starts_with(&root) && path.is_file() => path,
        Ok(_) => return response(StatusCode::NOT_FOUND, "not found", None, Body::empty()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return response(StatusCode::NOT_FOUND, "not found", None, Body::empty());
        }
        Err(_) => {
            return response(
                StatusCode::INTERNAL_SERVER_ERROR,
                "asset read failed",
                None,
                Body::empty(),
            );
        }
    };
    let bytes = match tokio::fs::read(&canonical).await {
        Ok(bytes) => bytes,
        Err(_) => {
            return response(
                StatusCode::INTERNAL_SERVER_ERROR,
                "asset read failed",
                None,
                Body::empty(),
            );
        }
    };
    response(
        StatusCode::OK,
        "",
        Some(mime_type(&canonical)),
        Body::from(bytes),
    )
}

fn response(
    status: StatusCode,
    text: &'static str,
    mime: Option<&'static str>,
    body: Body,
) -> Response<Body> {
    let mut response = Response::new(if mime.is_some() || text.is_empty() {
        body
    } else {
        Body::from(text)
    });
    *response.status_mut() = status;
    if let Some(mime) = mime {
        response
            .headers_mut()
            .insert(header::CONTENT_TYPE, HeaderValue::from_static(mime));
        response.headers_mut().insert(
            header::X_CONTENT_TYPE_OPTIONS,
            HeaderValue::from_static("nosniff"),
        );
    }
    response
}

fn mime_type(path: &Path) -> &'static str {
    match path
        .extension()
        .and_then(|ext| ext.to_str())
        .unwrap_or("")
        .to_ascii_lowercase()
        .as_str()
    {
        "html" | "htm" => "text/html; charset=utf-8",
        "css" => "text/css; charset=utf-8",
        "js" | "mjs" => "text/javascript; charset=utf-8",
        "json" | "map" => "application/json",
        "txt" => "text/plain; charset=utf-8",
        "xml" => "application/xml",
        "svg" => "image/svg+xml",
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "webp" => "image/webp",
        "ico" => "image/x-icon",
        "woff" => "font/woff",
        "woff2" => "font/woff2",
        "wasm" => "application/wasm",
        "pdf" => "application/pdf",
        _ => "application/octet-stream",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::to_bytes;

    #[test]
    fn maps_common_mime_types_and_unknowns() {
        assert_eq!(
            mime_type(Path::new("index.HTML")),
            "text/html; charset=utf-8"
        );
        assert_eq!(
            mime_type(Path::new("bundle.js")),
            "text/javascript; charset=utf-8"
        );
        assert_eq!(mime_type(Path::new("icon.svg")), "image/svg+xml");
        assert_eq!(mime_type(Path::new("data.bin")), "application/octet-stream");
    }

    #[tokio::test]
    async fn serves_asset_and_rejects_traversal() {
        let root = std::env::temp_dir().join(format!("harness-assets-{}", uuid::Uuid::new_v4()));
        tokio::fs::create_dir_all(&root).await.unwrap();
        tokio::fs::write(root.join("app.css"), "body{}")
            .await
            .unwrap();

        let response = asset_response(&root, "app.css").await;
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(
            response.headers()[header::CONTENT_TYPE],
            "text/css; charset=utf-8"
        );
        assert_eq!(
            to_bytes(response.into_body(), usize::MAX).await.unwrap(),
            "body{}"
        );

        for path in ["../secret", "sub/../../secret", "/etc/passwd", "..\\secret"] {
            assert_eq!(
                asset_response(&root, path).await.status(),
                StatusCode::BAD_REQUEST,
                "{path}"
            );
        }
        tokio::fs::remove_dir_all(root).await.unwrap();
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn rejects_symlink_escape() {
        use std::os::unix::fs::symlink;
        let root = std::env::temp_dir().join(format!("harness-assets-{}", uuid::Uuid::new_v4()));
        let outside = root.with_extension("outside");
        tokio::fs::create_dir_all(&root).await.unwrap();
        tokio::fs::write(&outside, "secret").await.unwrap();
        symlink(&outside, root.join("escape.txt")).unwrap();
        assert_eq!(
            asset_response(&root, "escape.txt").await.status(),
            StatusCode::NOT_FOUND
        );
        tokio::fs::remove_dir_all(root).await.unwrap();
        tokio::fs::remove_file(outside).await.unwrap();
    }
}
