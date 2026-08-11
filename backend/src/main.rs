mod app_paths;
mod config;
mod database;
mod media_root;
mod models;
mod openapi;
mod routes;
mod secrets;
mod services;
mod tasks;
mod training;

use axum::extract::{Request, State};
use axum::http::{header, HeaderValue, Method, StatusCode};
use axum::middleware::{from_fn, from_fn_with_state, Next};
use axum::response::{IntoResponse, Response};
use std::net::SocketAddr;
use std::str::FromStr;
use tokio::net::TcpListener;
use tower::limit::ConcurrencyLimitLayer;
use tower_http::cors::CorsLayer;
use tower_http::limit::RequestBodyLimitLayer;
use tower_http::services::{ServeDir, ServeFile};
use tower_http::trace::TraceLayer;
use tracing_subscriber::{fmt, EnvFilter};

fn host_is_loopback(value: &str) -> bool {
    if value.contains('@') {
        return false;
    }
    let Ok(authority) = axum::http::uri::Authority::from_str(value) else {
        return false;
    };
    let host = authority
        .host()
        .strip_prefix('[')
        .and_then(|host| host.strip_suffix(']'))
        .unwrap_or_else(|| authority.host());
    host.eq_ignore_ascii_case("localhost")
        || host
            .parse::<std::net::IpAddr>()
            .is_ok_and(|address| address.is_loopback())
}

fn build_runtime_router(
    paths: &app_paths::AppPaths,
    api: axum::Router,
    development: bool,
) -> Result<axum::Router, String> {
    use axum::{routing::any, Json};

    let index = paths.static_dir.join("index.html");
    if !index.is_file() {
        return Err(format!("前端入口不存在: {}", index.display()));
    }
    let static_service = ServeDir::new(paths.static_dir.clone()).fallback(ServeFile::new(index));
    let api_not_found = || async {
        (
            axum::http::StatusCode::NOT_FOUND,
            Json(serde_json::json!({
                "error": {
                    "code": "route_not_found",
                    "message": "API 路由不存在",
                    "retryable": false
                },
                "request_id": uuid::Uuid::new_v4().to_string()
            })),
        )
    };
    let policy = RuntimePolicy {
        development,
        application_port: paths.port,
    };
    let mut application = axum::Router::new()
        .merge(api)
        .route("/api", any(api_not_found))
        .route("/api/{*path}", any(api_not_found))
        .fallback_service(static_service)
        .layer(RequestBodyLimitLayer::new(1024 * 1024))
        .layer(ConcurrencyLimitLayer::new(128))
        .layer(TraceLayer::new_for_http())
        .layer(from_fn(normalize_api_error_response))
        .layer(from_fn_with_state(policy, enforce_local_request));

    if development {
        application = application.layer(
            CorsLayer::new()
                .allow_origin([
                    HeaderValue::from_static("http://127.0.0.1:5173"),
                    HeaderValue::from_static("http://localhost:5173"),
                ])
                .allow_methods([Method::GET, Method::POST, Method::PUT, Method::DELETE])
                .allow_headers([
                    header::ACCEPT,
                    header::CONTENT_TYPE,
                    header::RANGE,
                    header::HeaderName::from_static("last-event-id"),
                ])
                .expose_headers([
                    header::ACCEPT_RANGES,
                    header::CONTENT_LENGTH,
                    header::CONTENT_RANGE,
                ]),
        );
    }
    Ok(application)
}

async fn normalize_api_error_response(request: Request, next: Next) -> Response {
    let is_api_request =
        request.uri().path() == "/api" || request.uri().path().starts_with("/api/");
    let response = next.run(request).await;
    if !is_api_request
        || !response.status().is_client_error() && !response.status().is_server_error()
    {
        return response;
    }
    if response
        .headers()
        .get(header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .is_some_and(|value| value.starts_with("application/json"))
    {
        return response;
    }

    let status = response.status();
    let (code, message) = match status {
        StatusCode::BAD_REQUEST | StatusCode::UNPROCESSABLE_ENTITY => {
            ("invalid_request", "请求格式或字段无效")
        }
        StatusCode::METHOD_NOT_ALLOWED => ("method_not_allowed", "请求方法不受支持"),
        StatusCode::PAYLOAD_TOO_LARGE => ("payload_too_large", "请求体超过 1 MiB 限制"),
        StatusCode::UNSUPPORTED_MEDIA_TYPE => {
            ("unsupported_media_type", "请求 Content-Type 不受支持")
        }
        _ => ("http_error", "请求失败"),
    };
    let mut normalized = api_security_error(status, code, message);
    for (name, value) in response.headers() {
        if name != header::CONTENT_TYPE && name != header::CONTENT_LENGTH {
            normalized.headers_mut().insert(name, value.clone());
        }
    }
    normalized
}

#[derive(Debug, Clone, Copy)]
struct RuntimePolicy {
    development: bool,
    application_port: u16,
}

async fn enforce_local_request(
    State(policy): State<RuntimePolicy>,
    request: Request,
    next: Next,
) -> Response {
    let host_allowed = request
        .headers()
        .get(header::HOST)
        .and_then(|value| value.to_str().ok())
        .is_some_and(host_is_loopback);
    if !host_allowed {
        return with_security_headers(api_security_error(
            StatusCode::MISDIRECTED_REQUEST,
            "invalid_host",
            "仅接受本机 Host",
        ));
    }
    if let Some(origin) = request.headers().get(header::ORIGIN) {
        let origin_allowed = origin
            .to_str()
            .ok()
            .and_then(|value| url::Url::parse(value).ok())
            .is_some_and(|url| {
                url.scheme() == "http"
                    && url.host_str().is_some_and(host_name_is_loopback)
                    && url.port_or_known_default().is_some_and(|port| {
                        port == policy.application_port || (policy.development && port == 5173)
                    })
            });
        if !origin_allowed {
            return with_security_headers(api_security_error(
                StatusCode::FORBIDDEN,
                "invalid_origin",
                "仅接受本机应用来源",
            ));
        }
    }
    with_security_headers(next.run(request).await)
}

fn host_name_is_loopback(host: &str) -> bool {
    let host = host
        .strip_prefix('[')
        .and_then(|host| host.strip_suffix(']'))
        .unwrap_or(host);
    host.eq_ignore_ascii_case("localhost")
        || host
            .parse::<std::net::IpAddr>()
            .is_ok_and(|address| address.is_loopback())
}

fn api_security_error(status: StatusCode, code: &str, message: &str) -> Response {
    (
        status,
        axum::Json(serde_json::json!({
            "error": {
                "code": code,
                "message": message,
                "retryable": false
            },
            "request_id": uuid::Uuid::new_v4().to_string()
        })),
    )
        .into_response()
}

fn with_security_headers(mut response: Response) -> Response {
    let headers = response.headers_mut();
    headers.insert(
        header::CONTENT_SECURITY_POLICY,
        HeaderValue::from_static(
            "default-src 'self'; base-uri 'self'; frame-ancestors 'none'; object-src 'none'; script-src 'self'; style-src 'self' 'unsafe-inline'; img-src 'self' data: blob:; media-src 'self' blob:; connect-src 'self'; form-action 'self'",
        ),
    );
    headers.insert(
        header::X_CONTENT_TYPE_OPTIONS,
        HeaderValue::from_static("nosniff"),
    );
    headers.insert(header::X_FRAME_OPTIONS, HeaderValue::from_static("DENY"));
    headers.insert(
        header::REFERRER_POLICY,
        HeaderValue::from_static("no-referrer"),
    );
    headers.insert(
        header::HeaderName::from_static("permissions-policy"),
        HeaderValue::from_static("camera=(), microphone=(), geolocation=(), payment=()"),
    );
    headers.insert(
        header::HeaderName::from_static("cross-origin-opener-policy"),
        HeaderValue::from_static("same-origin"),
    );
    headers.insert(
        header::HeaderName::from_static("cross-origin-resource-policy"),
        HeaderValue::from_static("same-origin"),
    );
    response
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "danbooru_download_tool_pro=info".into()),
        )
        .init();

    if std::env::args_os().nth(1).as_deref() == Some(std::ffi::OsStr::new("--export-openapi")) {
        let output = std::env::args_os().nth(2).ok_or_else(|| {
            std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "--export-openapi requires an output path",
            )
        })?;
        openapi::export_document(std::path::Path::new(&output))?;
        return Ok(());
    }

    let paths = app_paths::AppPaths::from_env()
        .map_err(|error| std::io::Error::new(std::io::ErrorKind::InvalidInput, error))?;
    let development = std::env::var("DEV_CORS").as_deref() == Ok("1");
    let app = build_runtime_router(&paths, routes::api::router(), development)
        .map_err(|error| std::io::Error::new(std::io::ErrorKind::NotFound, error))?;
    let address = SocketAddr::new(paths.host, paths.port);
    tracing::info!(%address, "DanbooruDownload Tool Pro 已启动");
    let listener = TcpListener::bind(address).await?;
    axum::serve(listener, app).await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{app_paths::AppPaths, build_runtime_router, host_is_loopback};
    use axum::{
        body::to_bytes,
        extract::{Path as AxumPath, Query},
        http::{Request, StatusCode},
        routing::{get, put},
        Json, Router,
    };
    use tower::{Service, ServiceExt};

    #[derive(serde::Deserialize)]
    struct RequiredQuery {
        query: String,
    }

    #[test]
    fn rejects_non_loopback_host_header() {
        assert!(!host_is_loopback("attacker.example:8888"));
        assert!(host_is_loopback("[::1]:8888"));
    }

    #[tokio::test]
    async fn spa_fallback_does_not_hide_unknown_api_routes() {
        let directory = tempfile::tempdir().unwrap();
        let static_dir = directory.path().join("static");
        std::fs::create_dir_all(&static_dir).unwrap();
        std::fs::write(
            static_dir.join("index.html"),
            "<main>application shell</main>",
        )
        .unwrap();
        let paths = AppPaths::from_values(
            Some("127.0.0.1"),
            Some("8888"),
            Some(directory.path().to_str().unwrap()),
            Some(static_dir.to_str().unwrap()),
        )
        .unwrap();
        let api = Router::new().route("/api/health", get(|| async { "ok" }));
        let mut app = build_runtime_router(&paths, api, false).unwrap();

        let spa_response = app
            .call(
                Request::builder()
                    .uri("/explore")
                    .header("host", "localhost:8888")
                    .body(axum::body::Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(spa_response.status(), StatusCode::OK);
        let spa_body = to_bytes(spa_response.into_body(), usize::MAX)
            .await
            .unwrap();
        assert!(String::from_utf8_lossy(&spa_body).contains("application shell"));

        let api_response = app
            .call(
                Request::builder()
                    .uri("/api/does-not-exist")
                    .header("host", "localhost:8888")
                    .body(axum::body::Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(api_response.status(), StatusCode::NOT_FOUND);
        assert_eq!(
            api_response
                .headers()
                .get("content-type")
                .and_then(|value| value.to_str().ok()),
            Some("application/json")
        );
        let api_body = to_bytes(api_response.into_body(), 64 * 1024).await.unwrap();
        let api_json: serde_json::Value = serde_json::from_slice(&api_body).unwrap();
        assert_eq!(api_json["error"]["code"], "route_not_found");
        assert_eq!(api_json["error"]["retryable"], false);
        assert!(api_json["request_id"].is_string());
    }

    #[tokio::test]
    async fn runtime_rejects_remote_host_and_sets_browser_security_headers() {
        let directory = tempfile::tempdir().unwrap();
        let static_dir = directory.path().join("static");
        std::fs::create_dir_all(&static_dir).unwrap();
        std::fs::write(static_dir.join("index.html"), "<main>shell</main>").unwrap();
        let paths = AppPaths::from_values(
            Some("127.0.0.1"),
            Some("8888"),
            Some(directory.path().to_str().unwrap()),
            Some(static_dir.to_str().unwrap()),
        )
        .unwrap();
        let api = Router::new().route("/api/health", get(|| async { "ok" }));
        let mut app = build_runtime_router(&paths, api, false).unwrap();

        let rejected = app
            .call(
                Request::builder()
                    .uri("/api/health")
                    .header("host", "attacker.example:8888")
                    .body(axum::body::Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(rejected.status(), StatusCode::MISDIRECTED_REQUEST);

        let accepted = app
            .call(
                Request::builder()
                    .uri("/explore")
                    .header("host", "localhost:8888")
                    .body(axum::body::Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert!(accepted.headers().contains_key("content-security-policy"));
        assert_eq!(accepted.headers()["x-content-type-options"], "nosniff");
        assert!(accepted
            .headers()
            .get("access-control-allow-origin")
            .is_none());
    }

    #[tokio::test]
    async fn malformed_json_uses_the_api_error_envelope() {
        let directory = tempfile::tempdir().unwrap();
        let static_dir = directory.path().join("static");
        std::fs::create_dir_all(&static_dir).unwrap();
        std::fs::write(static_dir.join("index.html"), "<main>shell</main>").unwrap();
        let paths = AppPaths::from_values(
            Some("127.0.0.1"),
            Some("8888"),
            Some(directory.path().to_str().unwrap()),
            Some(static_dir.to_str().unwrap()),
        )
        .unwrap();
        let api = Router::new().route(
            "/api/config",
            put(|Json(_body): Json<serde_json::Value>| async { StatusCode::NO_CONTENT }),
        );
        let app = build_runtime_router(&paths, api, false).unwrap();

        let response = app
            .oneshot(
                Request::builder()
                    .method("PUT")
                    .uri("/api/config")
                    .header("host", "localhost:8888")
                    .header("content-type", "application/json")
                    .body(axum::body::Body::from(r#"{"broken": }"#))
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
        assert_eq!(response.headers()["content-type"], "application/json");
        let body = to_bytes(response.into_body(), 64 * 1024).await.unwrap();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(json["error"]["code"], "invalid_request");
        assert_eq!(json["error"]["retryable"], false);
        assert!(json["error"]["message"].is_string());
        assert!(json["request_id"].is_string());
    }

    #[tokio::test]
    async fn invalid_json_fields_use_the_api_error_envelope() {
        let directory = tempfile::tempdir().unwrap();
        let static_dir = directory.path().join("static");
        std::fs::create_dir_all(&static_dir).unwrap();
        std::fs::write(static_dir.join("index.html"), "<main>shell</main>").unwrap();
        let paths = AppPaths::from_values(
            Some("127.0.0.1"),
            Some("8888"),
            Some(directory.path().to_str().unwrap()),
            Some(static_dir.to_str().unwrap()),
        )
        .unwrap();
        let api = Router::new().route(
            "/api/config",
            put(|Json(_body): Json<String>| async { StatusCode::NO_CONTENT }),
        );
        let app = build_runtime_router(&paths, api, false).unwrap();

        let response = app
            .oneshot(
                Request::builder()
                    .method("PUT")
                    .uri("/api/config")
                    .header("host", "localhost:8888")
                    .header("content-type", "application/json")
                    .body(axum::body::Body::from("{}"))
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::UNPROCESSABLE_ENTITY);
        assert_eq!(response.headers()["content-type"], "application/json");
        let body = to_bytes(response.into_body(), 64 * 1024).await.unwrap();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(json["error"]["code"], "invalid_request");
        assert!(json["request_id"].is_string());
    }

    #[tokio::test]
    async fn missing_required_query_uses_the_api_error_envelope() {
        let directory = tempfile::tempdir().unwrap();
        let static_dir = directory.path().join("static");
        std::fs::create_dir_all(&static_dir).unwrap();
        std::fs::write(static_dir.join("index.html"), "<main>shell</main>").unwrap();
        let paths = AppPaths::from_values(
            Some("127.0.0.1"),
            Some("8888"),
            Some(directory.path().to_str().unwrap()),
            Some(static_dir.to_str().unwrap()),
        )
        .unwrap();
        let api = Router::new().route(
            "/api/search",
            get(|Query(query): Query<RequiredQuery>| async move { query.query }),
        );
        let app = build_runtime_router(&paths, api, false).unwrap();

        let response = app
            .oneshot(
                Request::builder()
                    .uri("/api/search")
                    .header("host", "localhost:8888")
                    .body(axum::body::Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
        assert_eq!(response.headers()["content-type"], "application/json");
        let body = to_bytes(response.into_body(), 64 * 1024).await.unwrap();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(json["error"]["code"], "invalid_request");
        assert!(json["request_id"].is_string());
    }

    #[tokio::test]
    async fn invalid_path_parameter_uses_the_api_error_envelope() {
        let directory = tempfile::tempdir().unwrap();
        let static_dir = directory.path().join("static");
        std::fs::create_dir_all(&static_dir).unwrap();
        std::fs::write(static_dir.join("index.html"), "<main>shell</main>").unwrap();
        let paths = AppPaths::from_values(
            Some("127.0.0.1"),
            Some("8888"),
            Some(directory.path().to_str().unwrap()),
            Some(static_dir.to_str().unwrap()),
        )
        .unwrap();
        let api = Router::new().route(
            "/api/items/{id}",
            get(|AxumPath(id): AxumPath<u64>| async move { id.to_string() }),
        );
        let app = build_runtime_router(&paths, api, false).unwrap();

        let response = app
            .oneshot(
                Request::builder()
                    .uri("/api/items/not-a-number")
                    .header("host", "localhost:8888")
                    .body(axum::body::Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
        assert_eq!(response.headers()["content-type"], "application/json");
        let body = to_bytes(response.into_body(), 64 * 1024).await.unwrap();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(json["error"]["code"], "invalid_request");
        assert!(json["request_id"].is_string());
    }

    #[tokio::test]
    async fn wrong_method_uses_the_api_error_envelope_and_keeps_allow_header() {
        let directory = tempfile::tempdir().unwrap();
        let static_dir = directory.path().join("static");
        std::fs::create_dir_all(&static_dir).unwrap();
        std::fs::write(static_dir.join("index.html"), "<main>shell</main>").unwrap();
        let paths = AppPaths::from_values(
            Some("127.0.0.1"),
            Some("8888"),
            Some(directory.path().to_str().unwrap()),
            Some(static_dir.to_str().unwrap()),
        )
        .unwrap();
        let api = Router::new().route("/api/health", get(|| async { "ok" }));
        let app = build_runtime_router(&paths, api, false).unwrap();

        let response = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/health")
                    .header("host", "localhost:8888")
                    .body(axum::body::Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::METHOD_NOT_ALLOWED);
        assert_eq!(response.headers()["content-type"], "application/json");
        assert_eq!(response.headers()["allow"], "GET,HEAD");
        let body = to_bytes(response.into_body(), 64 * 1024).await.unwrap();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(json["error"]["code"], "method_not_allowed");
        assert!(json["request_id"].is_string());
    }

    #[tokio::test]
    async fn oversized_request_body_uses_the_api_error_envelope() {
        let directory = tempfile::tempdir().unwrap();
        let static_dir = directory.path().join("static");
        std::fs::create_dir_all(&static_dir).unwrap();
        std::fs::write(static_dir.join("index.html"), "<main>shell</main>").unwrap();
        let paths = AppPaths::from_values(
            Some("127.0.0.1"),
            Some("8888"),
            Some(directory.path().to_str().unwrap()),
            Some(static_dir.to_str().unwrap()),
        )
        .unwrap();
        let api = Router::new().route(
            "/api/config",
            put(|Json(_body): Json<serde_json::Value>| async { StatusCode::NO_CONTENT }),
        );
        let app = build_runtime_router(&paths, api, false).unwrap();
        let oversized = serde_json::json!({ "value": "a".repeat(1024 * 1024) }).to_string();

        let response = app
            .oneshot(
                Request::builder()
                    .method("PUT")
                    .uri("/api/config")
                    .header("host", "localhost:8888")
                    .header("content-type", "application/json")
                    .body(axum::body::Body::from(oversized))
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::PAYLOAD_TOO_LARGE);
        assert_eq!(response.headers()["content-type"], "application/json");
        let body = to_bytes(response.into_body(), 64 * 1024).await.unwrap();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(json["error"]["code"], "payload_too_large");
        assert!(json["request_id"].is_string());
    }

    #[tokio::test]
    async fn missing_json_content_type_uses_the_api_error_envelope() {
        let directory = tempfile::tempdir().unwrap();
        let static_dir = directory.path().join("static");
        std::fs::create_dir_all(&static_dir).unwrap();
        std::fs::write(static_dir.join("index.html"), "<main>shell</main>").unwrap();
        let paths = AppPaths::from_values(
            Some("127.0.0.1"),
            Some("8888"),
            Some(directory.path().to_str().unwrap()),
            Some(static_dir.to_str().unwrap()),
        )
        .unwrap();
        let api = Router::new().route(
            "/api/config",
            put(|Json(_body): Json<serde_json::Value>| async { StatusCode::NO_CONTENT }),
        );
        let app = build_runtime_router(&paths, api, false).unwrap();

        let response = app
            .oneshot(
                Request::builder()
                    .method("PUT")
                    .uri("/api/config")
                    .header("host", "localhost:8888")
                    .body(axum::body::Body::from("{}"))
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::UNSUPPORTED_MEDIA_TYPE);
        assert_eq!(response.headers()["content-type"], "application/json");
        let body = to_bytes(response.into_body(), 64 * 1024).await.unwrap();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(json["error"]["code"], "unsupported_media_type");
        assert!(json["request_id"].is_string());
    }

    #[tokio::test]
    async fn successful_sse_response_passes_through_unchanged() {
        let directory = tempfile::tempdir().unwrap();
        let static_dir = directory.path().join("static");
        std::fs::create_dir_all(&static_dir).unwrap();
        std::fs::write(static_dir.join("index.html"), "<main>shell</main>").unwrap();
        let paths = AppPaths::from_values(
            Some("127.0.0.1"),
            Some("8888"),
            Some(directory.path().to_str().unwrap()),
            Some(static_dir.to_str().unwrap()),
        )
        .unwrap();
        let api = Router::new().route(
            "/api/tasks/events",
            get(|| async { ([("content-type", "text/event-stream")], "data: alive\n\n") }),
        );
        let app = build_runtime_router(&paths, api, false).unwrap();

        let response = app
            .oneshot(
                Request::builder()
                    .uri("/api/tasks/events")
                    .header("host", "localhost:8888")
                    .body(axum::body::Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(response.headers()["content-type"], "text/event-stream");
        let body = to_bytes(response.into_body(), 64 * 1024).await.unwrap();
        assert_eq!(body.as_ref(), b"data: alive\n\n");
    }

    #[tokio::test]
    async fn successful_media_response_passes_through_unchanged() {
        let directory = tempfile::tempdir().unwrap();
        let static_dir = directory.path().join("static");
        std::fs::create_dir_all(&static_dir).unwrap();
        std::fs::write(static_dir.join("index.html"), "<main>shell</main>").unwrap();
        let paths = AppPaths::from_values(
            Some("127.0.0.1"),
            Some("8888"),
            Some(directory.path().to_str().unwrap()),
            Some(static_dir.to_str().unwrap()),
        )
        .unwrap();
        let api = Router::new().route(
            "/api/library/media/1/file",
            get(|| async {
                (
                    [("content-type", "image/jpeg")],
                    vec![0xff, 0xd8, 0xff, 0xd9],
                )
            }),
        );
        let app = build_runtime_router(&paths, api, false).unwrap();

        let response = app
            .oneshot(
                Request::builder()
                    .uri("/api/library/media/1/file")
                    .header("host", "localhost:8888")
                    .body(axum::body::Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(response.headers()["content-type"], "image/jpeg");
        let body = to_bytes(response.into_body(), 64 * 1024).await.unwrap();
        assert_eq!(body.as_ref(), [0xff, 0xd8, 0xff, 0xd9]);
    }
}
