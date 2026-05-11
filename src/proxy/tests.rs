use super::*;
use axum::{
    Json, Router,
    body::Bytes,
    routing::{get, post},
};
use serde_json::json;
use std::{
    collections::HashSet,
    convert::Infallible,
    fs,
    net::SocketAddr,
    sync::atomic::{AtomicUsize, Ordering},
};
use tokio::sync::Mutex;

async fn spawn_upstream(app: Router) -> String {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    format!("http://{addr}")
}

async fn spawn_upstream_with_connect_info(app: Router) -> String {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(
            listener,
            app.into_make_service_with_connect_info::<SocketAddr>(),
        )
        .await
        .unwrap();
    });
    format!("http://{addr}")
}

fn test_app_state(config: Config) -> Arc<AppState> {
    Arc::new(AppState::new(config))
}

fn test_config(upstream_url: String) -> Arc<AppState> {
    test_app_state(Config {
        port: 0,
        auth_key: None,
        performance: crate::config::PerformanceConfig::default(),
        routing: crate::config::RoutingConfig::default(),
        health: crate::config::HealthConfig::default(),
        rectifier: crate::config::RectifierConfig::default(),
        upstream: Some(crate::config::UpstreamConfig {
            name: None,
            url: upstream_url,
            api_key: String::new(),
            auth_header: "Authorization".into(),
            proxy: None,
            proxy_type: crate::config::UpstreamProxyType::Http,
            api_format: crate::config::UpstreamApiFormat::ChatCompletions,
            max_concurrent_requests: None,
            rpm: None,
            tpm: None,
        }),
        upstreams: Vec::new(),
    })
}

fn test_multi_config(upstream_urls: Vec<String>) -> Arc<AppState> {
    test_app_state(Config {
        port: 0,
        auth_key: None,
        performance: crate::config::PerformanceConfig::default(),
        routing: crate::config::RoutingConfig::default(),
        health: crate::config::HealthConfig::default(),
        rectifier: crate::config::RectifierConfig::default(),
        upstream: None,
        upstreams: upstream_urls
            .into_iter()
            .map(|url| crate::config::UpstreamConfig {
                name: None,
                url,
                api_key: String::new(),
                auth_header: "Authorization".into(),
                proxy: None,
                proxy_type: crate::config::UpstreamProxyType::Http,
                api_format: crate::config::UpstreamApiFormat::ChatCompletions,
                max_concurrent_requests: None,
                rpm: None,
                tpm: None,
            })
            .collect(),
    })
}

fn test_auth_config(upstream_url: String) -> Arc<AppState> {
    test_app_state(Config {
        port: 0,
        auth_key: Some("proxy-secret".into()),
        performance: crate::config::PerformanceConfig::default(),
        routing: crate::config::RoutingConfig::default(),
        health: crate::config::HealthConfig::default(),
        rectifier: crate::config::RectifierConfig::default(),
        upstream: Some(crate::config::UpstreamConfig {
            name: None,
            url: upstream_url,
            api_key: String::new(),
            auth_header: "Authorization".into(),
            proxy: None,
            proxy_type: crate::config::UpstreamProxyType::Http,
            api_format: crate::config::UpstreamApiFormat::ChatCompletions,
            max_concurrent_requests: None,
            rpm: None,
            tpm: None,
        }),
        upstreams: Vec::new(),
    })
}

fn responses_request(stream: bool) -> Request<Body> {
    Request::builder()
        .method("POST")
        .uri("/v1/responses")
        .header("content-type", "application/json")
        .body(Body::from(
            json!({
                "model": "test-model",
                "stream": stream,
                "input": "hello"
            })
            .to_string(),
        ))
        .unwrap()
}

fn responses_model_request(model: &str) -> Request<Body> {
    Request::builder()
        .method("POST")
        .uri("/v1/responses")
        .header("content-type", "application/json")
        .body(Body::from(
            json!({
                "model": model,
                "input": "hello"
            })
            .to_string(),
        ))
        .unwrap()
}

fn chat_model_request(model: &str) -> Request<Body> {
    Request::builder()
        .method("POST")
        .uri("/v1/chat/completions")
        .header("content-type", "application/json")
        .body(Body::from(
            json!({
                "model": model,
                "messages": [{"role": "user", "content": "hello"}]
            })
            .to_string(),
        ))
        .unwrap()
}

fn chat_request_without_model() -> Request<Body> {
    Request::builder()
        .method("POST")
        .uri("/v1/chat/completions")
        .header("content-type", "application/json")
        .body(Body::from(
            json!({
                "messages": [{"role": "user", "content": "hello"}]
            })
            .to_string(),
        ))
        .unwrap()
}

fn chat_request() -> Request<Body> {
    Request::builder()
        .method("POST")
        .uri("/v1/chat/completions")
        .header("content-type", "application/json")
        .body(Body::from(
            json!({
                "model": "test-model",
                "messages": [{"role": "user", "content": "hello"}]
            })
            .to_string(),
        ))
        .unwrap()
}

#[tokio::test]
async fn chat_completions_should_reuse_upstream_connection_across_requests() {
    let remote_ports = Arc::new(Mutex::new(HashSet::new()));
    let app = Router::new().route(
        "/chat/completions",
        post({
            let remote_ports = remote_ports.clone();
            move |axum::extract::ConnectInfo(addr): axum::extract::ConnectInfo<SocketAddr>| {
                let remote_ports = remote_ports.clone();
                async move {
                    remote_ports.lock().await.insert(addr.port());
                    Json(json!({
                        "id": "chatcmpl_1",
                        "model": "test-model",
                        "choices": [{
                            "index": 0,
                            "message": {"role": "assistant", "content": "ok"},
                            "finish_reason": "stop"
                        }]
                    }))
                }
            }
        }),
    );
    let config = test_config(spawn_upstream_with_connect_info(app).await);

    let first = chat_completions(State(config.clone()), chat_request()).await;
    let second = chat_completions(State(config), chat_request()).await;

    assert_eq!(first.status(), StatusCode::OK);
    assert_eq!(second.status(), StatusCode::OK);
    assert_eq!(remote_ports.lock().await.len(), 1);
}

#[tokio::test]
async fn chat_completions_should_limit_concurrent_requests_per_upstream_when_configured() {
    let active = Arc::new(AtomicUsize::new(0));
    let max_active = Arc::new(AtomicUsize::new(0));
    let app = Router::new().route(
        "/chat/completions",
        post({
            let active = active.clone();
            let max_active = max_active.clone();
            move || {
                let active = active.clone();
                let max_active = max_active.clone();
                async move {
                    let current = active.fetch_add(1, Ordering::SeqCst) + 1;
                    max_active.fetch_max(current, Ordering::SeqCst);
                    tokio::time::sleep(std::time::Duration::from_millis(100)).await;
                    active.fetch_sub(1, Ordering::SeqCst);
                    Json(json!({
                        "id": "chatcmpl_1",
                        "model": "test-model",
                        "choices": [{
                            "index": 0,
                            "message": {"role": "assistant", "content": "ok"},
                            "finish_reason": "stop"
                        }]
                    }))
                }
            }
        }),
    );
    let upstream_url = spawn_upstream(app).await;
    let config: Config = toml::from_str(&format!(
        r#"
[performance]
upstream_max_concurrent_requests = 1

[upstream]
url = "{upstream_url}"
"#
    ))
    .unwrap();
    let state = test_app_state(config);

    let (first, second) = tokio::join!(
        chat_completions(State(state.clone()), chat_request()),
        chat_completions(State(state), chat_request())
    );

    assert_eq!(first.status(), StatusCode::OK);
    assert_eq!(second.status(), StatusCode::OK);
    assert_eq!(max_active.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn chat_completions_should_reject_request_when_global_rpm_limit_is_exhausted() {
    let attempts = Arc::new(AtomicUsize::new(0));
    let app = Router::new().route(
        "/chat/completions",
        post({
            let attempts = attempts.clone();
            move || {
                let attempts = attempts.clone();
                async move {
                    attempts.fetch_add(1, Ordering::SeqCst);
                    Json(json!({
                        "id": "chatcmpl_1",
                        "model": "test-model",
                        "choices": [{
                            "index": 0,
                            "message": {"role": "assistant", "content": "ok"},
                            "finish_reason": "stop"
                        }]
                    }))
                }
            }
        }),
    );
    let upstream_url = spawn_upstream(app).await;
    let config: Config = toml::from_str(&format!(
        r#"
[performance]
global_rpm = 1

[upstream]
url = "{upstream_url}"
"#
    ))
    .unwrap();
    let state = test_app_state(config);

    let first = chat_completions(State(state.clone()), chat_request()).await;
    let second = chat_completions(State(state), chat_request()).await;

    assert_eq!(first.status(), StatusCode::OK);
    assert_eq!(second.status(), StatusCode::TOO_MANY_REQUESTS);
    assert_eq!(attempts.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn chat_completions_should_reject_request_when_upstream_tpm_limit_would_be_exceeded() {
    let attempts = Arc::new(AtomicUsize::new(0));
    let app = Router::new().route(
        "/chat/completions",
        post({
            let attempts = attempts.clone();
            move || {
                let attempts = attempts.clone();
                async move {
                    attempts.fetch_add(1, Ordering::SeqCst);
                    Json(json!({}))
                }
            }
        }),
    );
    let upstream_url = spawn_upstream(app).await;
    let config: Config = toml::from_str(&format!(
        r#"
[upstream]
url = "{upstream_url}"
tpm = 1
"#
    ))
    .unwrap();

    let resp = chat_completions(State(test_app_state(config)), chat_request()).await;

    assert_eq!(resp.status(), StatusCode::TOO_MANY_REQUESTS);
    assert_eq!(attempts.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn chat_completions_should_send_upstream_request_through_configured_http_proxy() {
    let proxy_attempts = Arc::new(AtomicUsize::new(0));
    let proxy_app = Router::new().route(
        "/v1/chat/completions",
        post({
            let proxy_attempts = proxy_attempts.clone();
            move || {
                let proxy_attempts = proxy_attempts.clone();
                async move {
                    proxy_attempts.fetch_add(1, Ordering::SeqCst);
                    Json(json!({
                        "id": "chatcmpl_1",
                        "model": "test-model",
                        "choices": [{
                            "index": 0,
                            "message": {"role": "assistant", "content": "ok"},
                            "finish_reason": "stop"
                        }]
                    }))
                }
            }
        }),
    );
    let proxy_url = spawn_upstream(proxy_app).await;
    let proxy_addr = proxy_url.trim_start_matches("http://");
    let config: Config = toml::from_str(&format!(
        r#"
[[upstreams]]
url = "http://127.0.0.1:9/v1"
proxy = "{proxy_addr}"
proxy_type = "http"
"#
    ))
    .unwrap();
    let req = Request::builder()
        .method("POST")
        .uri("/v1/chat/completions")
        .header("content-type", "application/json")
        .body(Body::from(
            json!({
                "model": "test-model",
                "messages": [{"role": "user", "content": "hello"}]
            })
            .to_string(),
        ))
        .unwrap();

    let resp = chat_completions(State(test_app_state(config)), req).await;

    assert_eq!(resp.status(), StatusCode::OK);
    assert_eq!(proxy_attempts.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn responses_should_reject_request_when_proxy_auth_key_is_missing() {
    let attempts = Arc::new(AtomicUsize::new(0));
    let app = Router::new().route(
        "/chat/completions",
        post({
            let attempts = attempts.clone();
            move || {
                let attempts = attempts.clone();
                async move {
                    attempts.fetch_add(1, Ordering::SeqCst);
                    Json(json!({})).into_response()
                }
            }
        }),
    );
    let config = test_auth_config(spawn_upstream(app).await);

    let resp = responses(State(config), responses_request(false)).await;

    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
    assert_eq!(attempts.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn responses_should_accept_request_when_proxy_auth_key_matches() {
    let app = Router::new().route(
        "/chat/completions",
        post(|| async {
            Json(json!({
                "id": "chatcmpl_1",
                "model": "test-model",
                "choices": [{
                    "index": 0,
                    "message": {"role": "assistant", "content": "ok"},
                    "finish_reason": "stop"
                }],
                "usage": {
                    "prompt_tokens": 1,
                    "completion_tokens": 1,
                    "total_tokens": 2
                }
            }))
        }),
    );
    let config = test_auth_config(spawn_upstream(app).await);
    let req = Request::builder()
        .method("POST")
        .uri("/v1/responses")
        .header("content-type", "application/json")
        .header("authorization", "Bearer proxy-secret")
        .body(Body::from(
            json!({
                "model": "test-model",
                "stream": false,
                "input": "hello"
            })
            .to_string(),
        ))
        .unwrap();

    let resp = responses(State(config), req).await;

    assert_eq!(resp.status(), StatusCode::OK);
}

#[tokio::test]
async fn responses_should_convert_to_anthropic_messages_upstream_when_configured() {
    let seen_request = Arc::new(Mutex::new(None::<serde_json::Value>));
    let seen_headers = Arc::new(Mutex::new(None::<HeaderMap>));
    let app = Router::new().route(
            "/messages",
            post({
                let seen_request = seen_request.clone();
                let seen_headers = seen_headers.clone();
                move |headers: HeaderMap, Json(body): Json<serde_json::Value>| {
                    let seen_request = seen_request.clone();
                    let seen_headers = seen_headers.clone();
                    async move {
                        *seen_headers.lock().await = Some(headers);
                        *seen_request.lock().await = Some(body.clone());
                        Json(json!({
                            "id": "msg_1",
                            "type": "message",
                            "role": "assistant",
                            "model": body["model"],
                            "content": [
                                {"type": "text", "text": "anthropic ok"},
                                {"type": "tool_use", "id": "toolu_1", "name": "lookup", "input": {"q": "rust"}}
                            ],
                            "stop_reason": "tool_use",
                            "stop_sequence": null,
                            "usage": {"input_tokens": 3, "output_tokens": 5}
                        }))
                    }
                }
            }),
        );
    let upstream_url = spawn_upstream(app).await;
    let config: Config = toml::from_str(&format!(
        r#"
[upstream]
url = "{upstream_url}"
api_key = "anthropic-key"
auth_header = "x-api-key"
api_format = "anthropic_messages"
"#
    ))
    .unwrap();
    let req = Request::builder()
        .method("POST")
        .uri("/v1/responses")
        .header("content-type", "application/json")
        .body(Body::from(
            json!({
                "model": "claude-test",
                "instructions": "system prompt",
                "max_output_tokens": 123,
                "input": [
                    {
                        "type": "message",
                        "role": "user",
                        "content": [{"type": "input_text", "text": "hello"}]
                    }
                ],
                "tools": [{
                    "type": "function",
                    "name": "lookup",
                    "description": "Lookup",
                    "parameters": {"type": "object"}
                }],
                "tool_choice": {
                    "type": "function",
                    "name": "lookup"
                }
            })
            .to_string(),
        ))
        .unwrap();

    let resp = responses(State(test_app_state(config)), req).await;
    let status = resp.status();
    let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX)
        .await
        .unwrap();
    let body: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    let seen_request = seen_request.lock().await.clone().unwrap();
    let seen_headers = seen_headers.lock().await.clone().unwrap();

    assert_eq!(status, StatusCode::OK);
    assert_eq!(seen_headers["x-api-key"], "anthropic-key");
    assert_eq!(seen_headers["anthropic-version"], "2023-06-01");
    assert_eq!(seen_request["model"], "claude-test");
    assert_eq!(seen_request["system"], "system prompt");
    assert_eq!(seen_request["max_tokens"], 123);
    assert_eq!(seen_request["messages"][0]["role"], "user");
    assert_eq!(seen_request["messages"][0]["content"], "hello");
    assert_eq!(
        seen_request["tools"][0]["input_schema"],
        json!({"type": "object"})
    );
    assert_eq!(
        seen_request["tool_choice"],
        json!({"type": "tool", "name": "lookup"})
    );
    assert_eq!(body["output"][0]["content"][0]["text"], "anthropic ok");
    assert_eq!(body["output"][1]["type"], "function_call");
    assert_eq!(body["output"][1]["arguments"], "{\"q\":\"rust\"}");
    assert_eq!(body["usage"]["total_tokens"], 8);
}

#[tokio::test]
async fn responses_should_rectify_anthropic_thinking_signature_error_and_retry_same_upstream() {
    let attempts = Arc::new(AtomicUsize::new(0));
    let seen_requests = Arc::new(Mutex::new(Vec::<serde_json::Value>::new()));
    let app = Router::new().route(
        "/messages",
        post({
            let attempts = attempts.clone();
            let seen_requests = seen_requests.clone();
            move |Json(body): Json<serde_json::Value>| {
                let attempts = attempts.clone();
                let seen_requests = seen_requests.clone();
                async move {
                    let attempt = attempts.fetch_add(1, Ordering::SeqCst);
                    seen_requests.lock().await.push(body.clone());
                    if attempt == 0 {
                        return (
                            StatusCode::BAD_REQUEST,
                            Json(json!({
                                "error": {
                                    "message": "messages.1.content.0: Invalid `signature` in `thinking` block"
                                }
                            })),
                        )
                            .into_response();
                    }

                    Json(json!({
                        "id": "msg_1",
                        "type": "message",
                        "role": "assistant",
                        "model": body["model"],
                        "content": [{"type": "text", "text": "rectified ok"}],
                        "usage": {"input_tokens": 1, "output_tokens": 1}
                    }))
                    .into_response()
                }
            }
        }),
    );
    let upstream_url = spawn_upstream(app).await;
    let config: Config = toml::from_str(&format!(
        r#"
[upstream]
url = "{upstream_url}"
api_format = "anthropic_messages"
"#
    ))
    .unwrap();
    let req = Request::builder()
        .method("POST")
        .uri("/v1/responses")
        .header("content-type", "application/json")
        .body(Body::from(
            json!({
                "model": "claude-test",
                "input": [
                    {
                        "type": "message",
                        "role": "user",
                        "content": [{"type": "input_text", "text": "first"}]
                    },
                    {
                        "type": "message",
                        "role": "assistant",
                        "content": [
                            {"type": "thinking", "thinking": "old reasoning", "signature": "bad"},
                            {"type": "output_text", "text": "previous"}
                        ]
                    },
                    {
                        "type": "message",
                        "role": "user",
                        "content": [{"type": "input_text", "text": "next"}]
                    }
                ]
            })
            .to_string(),
        ))
        .unwrap();

    let resp = responses(State(test_app_state(config)), req).await;
    let status = resp.status();
    let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX)
        .await
        .unwrap();
    let body: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    let seen_requests = seen_requests.lock().await.clone();

    assert_eq!(status, StatusCode::OK);
    assert_eq!(attempts.load(Ordering::SeqCst), 2);
    assert_eq!(body["output"][0]["content"][0]["text"], "rectified ok");
    assert_eq!(
        seen_requests[0]["messages"][1]["content"][0]["type"],
        "thinking"
    );
    assert!(
        seen_requests[1]["messages"][1]["content"]
            .as_array()
            .unwrap()
            .iter()
            .all(|block| block.get("type").and_then(|value| value.as_str()) != Some("thinking"))
    );
}

#[tokio::test]
async fn responses_should_rectify_anthropic_thinking_budget_error_and_retry_same_upstream() {
    let attempts = Arc::new(AtomicUsize::new(0));
    let seen_requests = Arc::new(Mutex::new(Vec::<serde_json::Value>::new()));
    let app = Router::new().route(
        "/messages",
        post({
            let attempts = attempts.clone();
            let seen_requests = seen_requests.clone();
            move |Json(body): Json<serde_json::Value>| {
                let attempts = attempts.clone();
                let seen_requests = seen_requests.clone();
                async move {
                    let attempt = attempts.fetch_add(1, Ordering::SeqCst);
                    seen_requests.lock().await.push(body.clone());
                    if attempt == 0 {
                        return (
                            StatusCode::BAD_REQUEST,
                            Json(json!({
                                "error": {
                                    "message": "thinking.budget_tokens: Input should be greater than or equal to 1024"
                                }
                            })),
                        )
                            .into_response();
                    }

                    Json(json!({
                        "id": "msg_1",
                        "type": "message",
                        "role": "assistant",
                        "model": body["model"],
                        "content": [{"type": "text", "text": "budget rectified ok"}],
                        "usage": {"input_tokens": 1, "output_tokens": 1}
                    }))
                    .into_response()
                }
            }
        }),
    );
    let upstream_url = spawn_upstream(app).await;
    let config: Config = toml::from_str(&format!(
        r#"
[upstream]
url = "{upstream_url}"
api_format = "anthropic_messages"
"#
    ))
    .unwrap();
    let req = Request::builder()
        .method("POST")
        .uri("/v1/responses")
        .header("content-type", "application/json")
        .body(Body::from(
            json!({
                "model": "claude-test",
                "max_output_tokens": 1024,
                "thinking": {"type": "enabled", "budget_tokens": 512},
                "input": "hello"
            })
            .to_string(),
        ))
        .unwrap();

    let resp = responses(State(test_app_state(config)), req).await;
    let status = resp.status();
    let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX)
        .await
        .unwrap();
    let body: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    let seen_requests = seen_requests.lock().await.clone();

    assert_eq!(status, StatusCode::OK);
    assert_eq!(attempts.load(Ordering::SeqCst), 2);
    assert_eq!(
        body["output"][0]["content"][0]["text"],
        "budget rectified ok"
    );
    assert_eq!(seen_requests[0]["thinking"]["budget_tokens"], 512);
    assert_eq!(seen_requests[0]["max_tokens"], 1024);
    assert_eq!(seen_requests[1]["thinking"]["type"], "enabled");
    assert_eq!(seen_requests[1]["thinking"]["budget_tokens"], 32000);
    assert_eq!(seen_requests[1]["max_tokens"], 64000);
}

#[tokio::test]
async fn responses_stream_should_rectify_anthropic_error_and_retry_same_upstream_streaming() {
    let attempts = Arc::new(AtomicUsize::new(0));
    let app = Router::new().route(
        "/messages",
        post({
            let attempts = attempts.clone();
            move || {
                let attempts = attempts.clone();
                async move {
                    let attempt = attempts.fetch_add(1, Ordering::SeqCst);
                    if attempt == 0 {
                        return (
                            StatusCode::BAD_REQUEST,
                            Json(json!({
                                "error": {
                                    "message": "messages.1.content.0: Invalid `signature` in `thinking` block"
                                }
                            })),
                        )
                            .into_response();
                    }

                    let chunks = vec![
                        r#"data: {"type":"message_start","message":{"id":"msg_1","type":"message","role":"assistant","model":"claude-test"}}

data: {"type":"content_block_start","index":0,"content_block":{"type":"text"}}

data: {"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":"rectified stream"}}

data: {"type":"message_delta","delta":{"stop_reason":"end_turn"},"usage":{"input_tokens":1,"output_tokens":2}}

data: {"type":"message_stop"}

"#,
                    ];
                    let stream = futures::stream::iter(
                        chunks
                            .into_iter()
                            .map(|chunk| Ok::<_, Infallible>(Bytes::from(chunk))),
                    );

                    Response::builder()
                        .status(StatusCode::OK)
                        .header("content-type", "text/event-stream")
                        .body(Body::from_stream(stream))
                        .unwrap()
                }
            }
        }),
    );
    let upstream_url = spawn_upstream(app).await;
    let config: Config = toml::from_str(&format!(
        r#"
[upstream]
url = "{upstream_url}"
api_format = "anthropic_messages"
"#
    ))
    .unwrap();
    let req = Request::builder()
        .method("POST")
        .uri("/v1/responses")
        .header("content-type", "application/json")
        .body(Body::from(
            json!({
                "model": "claude-test",
                "stream": true,
                "input": [
                    {
                        "type": "message",
                        "role": "assistant",
                        "content": [{"type": "thinking", "thinking": "old", "signature": "bad"}]
                    },
                    {
                        "type": "message",
                        "role": "user",
                        "content": [{"type": "input_text", "text": "next"}]
                    }
                ]
            })
            .to_string(),
        ))
        .unwrap();

    let resp = responses(State(test_app_state(config)), req).await;
    let status = resp.status();
    let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX)
        .await
        .unwrap();
    let text = String::from_utf8(bytes.to_vec()).unwrap();

    assert_eq!(status, StatusCode::OK);
    assert_eq!(attempts.load(Ordering::SeqCst), 2);
    assert!(text.contains("response.output_text.delta"));
    assert!(text.contains("rectified stream"));
    assert!(text.contains("response.completed"));
}

#[tokio::test]
async fn chat_completions_should_reject_request_when_proxy_auth_key_is_missing() {
    let attempts = Arc::new(AtomicUsize::new(0));
    let app = Router::new().route(
        "/chat/completions",
        post({
            let attempts = attempts.clone();
            move || {
                let attempts = attempts.clone();
                async move {
                    attempts.fetch_add(1, Ordering::SeqCst);
                    Json(json!({})).into_response()
                }
            }
        }),
    );
    let config = test_auth_config(spawn_upstream(app).await);
    let req = Request::builder()
        .method("POST")
        .uri("/v1/chat/completions")
        .header("content-type", "application/json")
        .body(Body::from(
            json!({
                "model": "test-model",
                "messages": [{"role": "user", "content": "hello"}]
            })
            .to_string(),
        ))
        .unwrap();

    let resp = chat_completions(State(config), req).await;

    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
    assert_eq!(attempts.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn models_should_reject_request_when_proxy_auth_key_is_missing() {
    let attempts = Arc::new(AtomicUsize::new(0));
    let app = Router::new().route(
        "/models",
        get({
            let attempts = attempts.clone();
            move || {
                let attempts = attempts.clone();
                async move {
                    attempts.fetch_add(1, Ordering::SeqCst);
                    Json(json!({})).into_response()
                }
            }
        }),
    );
    let config = test_auth_config(spawn_upstream(app).await);

    let resp = models(State(config), HeaderMap::new()).await;

    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
    assert_eq!(attempts.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn responses_should_retry_retryable_upstream_status_before_success() {
    let attempts = Arc::new(AtomicUsize::new(0));
    let app = Router::new().route(
        "/chat/completions",
        post({
            let attempts = attempts.clone();
            move || {
                let attempts = attempts.clone();
                async move {
                    if attempts.fetch_add(1, Ordering::SeqCst) == 0 {
                        return (
                            StatusCode::TOO_MANY_REQUESTS,
                            Json(json!({"error": "rate limited"})),
                        )
                            .into_response();
                    }

                    Json(json!({
                        "id": "chatcmpl_1",
                        "model": "test-model",
                        "choices": [{
                            "index": 0,
                            "message": {"role": "assistant", "content": "ok"},
                            "finish_reason": "stop"
                        }],
                        "usage": {
                            "prompt_tokens": 1,
                            "completion_tokens": 1,
                            "total_tokens": 2
                        }
                    }))
                    .into_response()
                }
            }
        }),
    );
    let config = test_config(spawn_upstream(app).await);

    let resp = responses(State(config), responses_request(false)).await;

    assert_eq!(resp.status(), StatusCode::OK);
    assert_eq!(attempts.load(Ordering::SeqCst), 2);
}

#[tokio::test]
async fn responses_should_make_initial_request_then_retry_three_times() {
    let attempts = Arc::new(AtomicUsize::new(0));
    let app = Router::new().route(
        "/chat/completions",
        post({
            let attempts = attempts.clone();
            move || {
                let attempts = attempts.clone();
                async move {
                    if attempts.fetch_add(1, Ordering::SeqCst) < 3 {
                        return (
                            StatusCode::BAD_GATEWAY,
                            Json(json!({"error": "temporary failure"})),
                        )
                            .into_response();
                    }

                    Json(json!({
                        "id": "chatcmpl_1",
                        "model": "test-model",
                        "choices": [{
                            "index": 0,
                            "message": {"role": "assistant", "content": "ok"},
                            "finish_reason": "stop"
                        }],
                        "usage": {
                            "prompt_tokens": 1,
                            "completion_tokens": 1,
                            "total_tokens": 2
                        }
                    }))
                    .into_response()
                }
            }
        }),
    );
    let config = test_config(spawn_upstream(app).await);

    let resp = responses(State(config), responses_request(false)).await;

    assert_eq!(resp.status(), StatusCode::OK);
    assert_eq!(attempts.load(Ordering::SeqCst), 4);
}

#[tokio::test]
async fn responses_stream_should_convert_split_sse_and_complete_output() {
    let chunks = vec![
        r#"data: {"id":"chunk_1","model":"test-model","choices":[{"index":0,"delta":{"content":"hel"#,
        r#"lo"},"finish_reason":null}]}

data: {"id":"chunk_2","model":"test-model","choices":[{"index":0,"delta":{},"finish_reason":"stop"}],"usage":{"prompt_tokens":1,"completion_tokens":1,"total_tokens":2}}

data: [DONE]

"#,
    ];
    let app = Router::new().route(
        "/chat/completions",
        post(move || {
            let chunks = chunks.clone();
            async move {
                let stream = futures::stream::iter(
                    chunks
                        .into_iter()
                        .map(|chunk| Ok::<_, Infallible>(Bytes::from(chunk))),
                );

                Response::builder()
                    .status(StatusCode::OK)
                    .header("content-type", "text/event-stream")
                    .body(Body::from_stream(stream))
                    .unwrap()
            }
        }),
    );
    let config = test_config(spawn_upstream(app).await);

    let resp = responses(State(config), responses_request(true)).await;
    let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX)
        .await
        .unwrap();
    let text = String::from_utf8(bytes.to_vec()).unwrap();

    assert!(text.contains("response.output_text.delta"));
    assert!(text.contains("\"delta\":\"hello\""));
    assert!(text.contains("response.completed"));
    assert!(text.contains("\"output\":[{\"content\""));
    assert!(text.contains("\"text\":\"hello\""));
}

#[tokio::test]
async fn responses_stream_should_complete_output_when_done_arrives_without_finish_reason() {
    let chunks = vec![
        r#"data: {"id":"chunk_1","model":"test-model","choices":[{"index":0,"delta":{"content":"hello"},"finish_reason":null}]}

data: [DONE]

"#,
    ];
    let app = Router::new().route(
        "/chat/completions",
        post(move || {
            let chunks = chunks.clone();
            async move {
                let stream = futures::stream::iter(
                    chunks
                        .into_iter()
                        .map(|chunk| Ok::<_, Infallible>(Bytes::from(chunk))),
                );

                Response::builder()
                    .status(StatusCode::OK)
                    .header("content-type", "text/event-stream")
                    .body(Body::from_stream(stream))
                    .unwrap()
            }
        }),
    );
    let config = test_config(spawn_upstream(app).await);

    let resp = responses(State(config), responses_request(true)).await;
    let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX)
        .await
        .unwrap();
    let text = String::from_utf8(bytes.to_vec()).unwrap();

    assert!(text.contains("response.output_item.done"));
    assert!(text.contains("\"output\":[{\"content\""));
    assert!(text.contains("\"text\":\"hello\""));
}

#[tokio::test]
async fn responses_stream_should_convert_anthropic_messages_events() {
    let chunks = vec![
        r#"data: {"type":"message_start","message":{"id":"msg_1","type":"message","role":"assistant","model":"claude-test"}}

data: {"type":"content_block_start","index":0,"content_block":{"type":"text"}}

data: {"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":"hel"}}

data: {"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":"lo"}}

data: {"type":"content_block_stop","index":0}

data: {"type":"message_delta","delta":{"stop_reason":"end_turn"},"usage":{"input_tokens":2,"output_tokens":4}}

data: {"type":"message_stop"}

"#,
    ];
    let seen_request = Arc::new(Mutex::new(None::<serde_json::Value>));
    let app = Router::new().route(
        "/messages",
        post({
            let seen_request = seen_request.clone();
            move |Json(body): Json<serde_json::Value>| {
                let chunks = chunks.clone();
                let seen_request = seen_request.clone();
                async move {
                    *seen_request.lock().await = Some(body);
                    let stream = futures::stream::iter(
                        chunks
                            .into_iter()
                            .map(|chunk| Ok::<_, Infallible>(Bytes::from(chunk))),
                    );

                    Response::builder()
                        .status(StatusCode::OK)
                        .header("content-type", "text/event-stream")
                        .body(Body::from_stream(stream))
                        .unwrap()
                }
            }
        }),
    );
    let upstream_url = spawn_upstream(app).await;
    let config: Config = toml::from_str(&format!(
        r#"
[upstream]
url = "{upstream_url}"
api_format = "anthropic_messages"
"#
    ))
    .unwrap();

    let resp = responses(State(test_app_state(config)), responses_request(true)).await;
    let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX)
        .await
        .unwrap();
    let text = String::from_utf8(bytes.to_vec()).unwrap();
    let seen_request = seen_request.lock().await.clone().unwrap();

    assert_eq!(seen_request["stream"], true);
    assert_eq!(seen_request["max_tokens"], 4096);
    assert!(text.contains("response.output_text.delta"));
    assert!(text.contains("\"delta\":\"hel\""));
    assert!(text.contains("\"delta\":\"lo\""));
    assert!(text.contains("response.completed"));
    assert!(text.contains("\"text\":\"hello\""));
    assert!(text.contains("\"total_tokens\":6"));
}
#[tokio::test]
async fn models_should_proxy_upstream_model_list() {
    let app = Router::new().route(
        "/models",
        get(|| async {
            Json(json!({
                "object": "list",
                "data": [{
                    "id": "test-model",
                    "object": "model"
                }]
            }))
        }),
    );
    let config = test_config(spawn_upstream(app).await);

    let resp = models(State(config), HeaderMap::new()).await;
    let status = resp.status();
    let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX)
        .await
        .unwrap();
    let body: serde_json::Value = serde_json::from_slice(&bytes).unwrap();

    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["data"][0]["id"], "test-model");
}

#[tokio::test]
async fn models_should_merge_model_lists_from_multiple_upstreams() {
    let upstream_a = Router::new().route(
        "/models",
        get(|| async {
            Json(json!({
                "object": "list",
                "data": [{"id": "model-a", "object": "model"}]
            }))
        }),
    );
    let upstream_b = Router::new().route(
        "/models",
        get(|| async {
            Json(json!({
                "object": "list",
                "data": [{"id": "model-b", "object": "model"}]
            }))
        }),
    );
    let config = test_multi_config(vec![
        spawn_upstream(upstream_a).await,
        spawn_upstream(upstream_b).await,
    ]);

    let resp = models(State(config), HeaderMap::new()).await;
    let status = resp.status();
    let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX)
        .await
        .unwrap();
    let body: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    let ids: Vec<_> = body["data"]
        .as_array()
        .unwrap()
        .iter()
        .map(|model| model["id"].as_str().unwrap())
        .collect();

    assert_eq!(status, StatusCode::OK);
    assert_eq!(ids, vec!["model-a", "model-b"]);
}

#[tokio::test]
async fn models_should_use_prefetched_cache_for_repeated_multi_upstream_model_lists() {
    let upstream_a_model_calls = Arc::new(AtomicUsize::new(0));
    let upstream_b_model_calls = Arc::new(AtomicUsize::new(0));
    let upstream_a = Router::new().route(
        "/models",
        get({
            let upstream_a_model_calls = upstream_a_model_calls.clone();
            move || {
                let upstream_a_model_calls = upstream_a_model_calls.clone();
                async move {
                    upstream_a_model_calls.fetch_add(1, Ordering::SeqCst);
                    Json(json!({
                        "object": "list",
                        "data": [{"id": "model-a", "object": "model"}]
                    }))
                }
            }
        }),
    );
    let upstream_b = Router::new().route(
        "/models",
        get({
            let upstream_b_model_calls = upstream_b_model_calls.clone();
            move || {
                let upstream_b_model_calls = upstream_b_model_calls.clone();
                async move {
                    upstream_b_model_calls.fetch_add(1, Ordering::SeqCst);
                    Json(json!({
                        "object": "list",
                        "data": [{"id": "model-b", "object": "model"}]
                    }))
                }
            }
        }),
    );
    let state = test_multi_config(vec![
        spawn_upstream(upstream_a).await,
        spawn_upstream(upstream_b).await,
    ]);

    for _ in 0..50 {
        if upstream_a_model_calls.load(Ordering::SeqCst) == 1
            && upstream_b_model_calls.load(Ordering::SeqCst) == 1
        {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }

    assert_eq!(upstream_a_model_calls.load(Ordering::SeqCst), 1);
    assert_eq!(upstream_b_model_calls.load(Ordering::SeqCst), 1);

    let first = models(State(state.clone()), HeaderMap::new()).await;
    let second = models(State(state), HeaderMap::new()).await;

    assert_eq!(first.status(), StatusCode::OK);
    assert_eq!(second.status(), StatusCode::OK);
    assert_eq!(upstream_a_model_calls.load(Ordering::SeqCst), 1);
    assert_eq!(upstream_b_model_calls.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn models_should_prefix_model_ids_with_upstream_names_when_configured() {
    let upstream_a = Router::new().route(
        "/models",
        get(|| async {
            Json(json!({
                "object": "list",
                "data": [{"id": "shared-model", "object": "model"}]
            }))
        }),
    );
    let upstream_b = Router::new().route(
        "/models",
        get(|| async {
            Json(json!({
                "object": "list",
                "data": [{"id": "shared-model", "object": "model"}]
            }))
        }),
    );
    let config: Config = toml::from_str(&format!(
        r#"
[[upstreams]]
name = "openai"
url = "{}"

[[upstreams]]
name = "local"
url = "{}"
"#,
        spawn_upstream(upstream_a).await,
        spawn_upstream(upstream_b).await
    ))
    .unwrap();

    let resp = models(State(test_app_state(config)), HeaderMap::new()).await;
    let status = resp.status();
    let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX)
        .await
        .unwrap();
    let body: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    let ids: Vec<_> = body["data"]
        .as_array()
        .unwrap()
        .iter()
        .map(|model| model["id"].as_str().unwrap())
        .collect();

    assert_eq!(status, StatusCode::OK);
    assert_eq!(ids, vec!["openai:shared-model", "local:shared-model"]);
}

#[tokio::test]
async fn models_should_hide_configured_model_aliases_by_default() {
    let app = Router::new().route(
        "/models",
        get(|| async {
            Json(json!({
                "object": "list",
                "data": [{"id": "qwen3-coder", "object": "model"}]
            }))
        }),
    );
    let upstream_url = spawn_upstream(app).await;
    let config: Config = toml::from_str(&format!(
        r#"
[routing.model_aliases]
"gpt-5.5" = "qwen3-coder"
"gpt-5.4-mini" = "qwen3-coder-mini"

[upstream]
url = "{upstream_url}"
"#
    ))
    .unwrap();

    let resp = models(State(test_app_state(config)), HeaderMap::new()).await;
    let status = resp.status();
    let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX)
        .await
        .unwrap();
    let body: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    let ids: Vec<_> = body["data"]
        .as_array()
        .unwrap()
        .iter()
        .map(|item| item["id"].as_str().unwrap().to_string())
        .collect();

    assert_eq!(status, StatusCode::OK);
    assert_eq!(ids, vec!["qwen3-coder"]);
}

#[tokio::test]
async fn models_should_include_configured_model_aliases_when_exposure_is_enabled() {
    let app = Router::new().route(
        "/models",
        get(|| async {
            Json(json!({
                "object": "list",
                "data": [{"id": "qwen3-coder", "object": "model"}]
            }))
        }),
    );
    let upstream_url = spawn_upstream(app).await;
    let config: Config = toml::from_str(&format!(
        r#"
[routing]
expose_model_aliases = true

[routing.model_aliases]
"gpt-5.5" = "qwen3-coder"
"gpt-5.4-mini" = "qwen3-coder-mini"

[upstream]
url = "{upstream_url}"
"#
    ))
    .unwrap();

    let resp = models(State(test_app_state(config)), HeaderMap::new()).await;
    let status = resp.status();
    let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX)
        .await
        .unwrap();
    let body: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    let ids: Vec<_> = body["data"]
        .as_array()
        .unwrap()
        .iter()
        .map(|item| item["id"].as_str().unwrap().to_string())
        .collect();

    assert_eq!(status, StatusCode::OK);
    assert!(ids.contains(&"gpt-5.5".to_string()));
    assert!(ids.contains(&"gpt-5.4-mini".to_string()));
}

#[tokio::test]
async fn responses_should_route_prefixed_model_and_strip_prefix_before_upstream() {
    let upstream_a_called = Arc::new(AtomicUsize::new(0));
    let upstream_b_called = Arc::new(AtomicUsize::new(0));
    let upstream_a = Router::new()
        .route(
            "/models",
            get(|| async {
                Json(json!({
                    "object": "list",
                    "data": [{"id": "shared-model", "object": "model"}]
                }))
            }),
        )
        .route(
            "/chat/completions",
            post({
                let upstream_a_called = upstream_a_called.clone();
                move || {
                    let upstream_a_called = upstream_a_called.clone();
                    async move {
                        upstream_a_called.fetch_add(1, Ordering::SeqCst);
                        Json(json!({"error": "wrong upstream"})).into_response()
                    }
                }
            }),
        );
    let upstream_b = Router::new()
        .route(
            "/models",
            get(|| async {
                Json(json!({
                    "object": "list",
                    "data": [{"id": "shared-model", "object": "model"}]
                }))
            }),
        )
        .route(
            "/chat/completions",
            post({
                let upstream_b_called = upstream_b_called.clone();
                move |Json(body): Json<serde_json::Value>| {
                    let upstream_b_called = upstream_b_called.clone();
                    async move {
                        upstream_b_called.fetch_add(1, Ordering::SeqCst);
                        Json(json!({
                            "id": "chatcmpl_b",
                            "model": body["model"],
                            "choices": [{
                                "index": 0,
                                "message": {"role": "assistant", "content": "from local"},
                                "finish_reason": "stop"
                            }]
                        }))
                        .into_response()
                    }
                }
            }),
        );
    let config: Config = toml::from_str(&format!(
        r#"
[[upstreams]]
name = "openai"
url = "{}"

[[upstreams]]
name = "local"
url = "{}"
"#,
        spawn_upstream(upstream_a).await,
        spawn_upstream(upstream_b).await
    ))
    .unwrap();
    let req = Request::builder()
        .method("POST")
        .uri("/v1/responses")
        .header("content-type", "application/json")
        .body(Body::from(
            json!({
                "model": "local:shared-model",
                "input": "hello"
            })
            .to_string(),
        ))
        .unwrap();

    let resp = responses(State(test_app_state(config)), req).await;
    let status = resp.status();
    let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX)
        .await
        .unwrap();
    let body: serde_json::Value = serde_json::from_slice(&bytes).unwrap();

    assert_eq!(status, StatusCode::OK);
    assert_eq!(upstream_a_called.load(Ordering::SeqCst), 0);
    assert_eq!(upstream_b_called.load(Ordering::SeqCst), 1);
    assert_eq!(body["model"], "shared-model");
    assert_eq!(body["output"][0]["content"][0]["text"], "from local");
}

#[tokio::test]
async fn responses_should_forward_deepseek_reasoning_content_without_disabling_thinking() {
    let seen_request = Arc::new(Mutex::new(None::<serde_json::Value>));
    let app = Router::new()
            .route(
                "/models",
                get(|| async {
                    Json(json!({
                        "object": "list",
                        "data": [{"id": "deepseek-v4-pro", "object": "model"}]
                    }))
                }),
            )
            .route(
                "/chat/completions",
                post({
                    let seen_request = seen_request.clone();
                    move |Json(body): Json<serde_json::Value>| {
                        let seen_request = seen_request.clone();
                        async move {
                            *seen_request.lock().await = Some(body.clone());
                            if body["messages"][1]["reasoning_content"] != "previous reasoning" {
                                return (
                                    StatusCode::BAD_REQUEST,
                                    Json(json!({
                                        "error": {
                                            "message": "The `reasoning_content` in the thinking mode must be passed back to the API."
                                        }
                                    })),
                                )
                                    .into_response();
                            }

                            Json(json!({
                                "id": "chatcmpl_deepseek",
                                "model": body["model"],
                                "thinking": body.get("thinking").cloned(),
                                "choices": [{
                                    "index": 0,
                                    "message": {
                                        "role": "assistant",
                                        "reasoning_content": "new reasoning",
                                        "content": "ok"
                                    },
                                    "finish_reason": "stop"
                                }]
                            }))
                            .into_response()
                        }
                    }
                }),
            );
    let upstream_url = spawn_upstream(app).await;
    let config: Config = toml::from_str(&format!(
        r#"
[[upstreams]]
name = "deepseek"
url = "{upstream_url}"
"#
    ))
    .unwrap();
    let req = Request::builder()
        .method("POST")
        .uri("/v1/responses")
        .header("content-type", "application/json")
        .body(Body::from(
            json!({
                "model": "deepseek:deepseek-v4-pro",
                "input": [
                    {
                        "type": "message",
                        "role": "user",
                        "content": [{"type": "input_text", "text": "first"}]
                    },
                    {
                        "type": "message",
                        "role": "assistant",
                        "reasoning_content": "previous reasoning",
                        "content": [{"type": "output_text", "text": "previous"}]
                    },
                    {
                        "type": "message",
                        "role": "user",
                        "content": [{"type": "input_text", "text": "next"}]
                    }
                ]
            })
            .to_string(),
        ))
        .unwrap();

    let resp = responses(State(test_app_state(config)), req).await;

    assert_eq!(resp.status(), StatusCode::OK);
    let seen_request = seen_request.lock().await.clone().unwrap();
    assert!(seen_request.get("thinking").is_none());
    assert_eq!(
        seen_request["messages"][1]["reasoning_content"],
        "previous reasoning"
    );
}

#[tokio::test]
async fn responses_should_dump_request_input_when_upstream_returns_error() {
    let debug_dir =
        std::env::temp_dir().join(format!("open-promux-debug-{}", uuid::Uuid::new_v4()));
    unsafe {
        std::env::set_var("OPEN_PROMUX_DEBUG_DIR", &debug_dir);
    }

    let app = Router::new().route(
        "/chat/completions",
        post(|| async {
            (
                StatusCode::BAD_REQUEST,
                Json(json!({
                    "error": {
                        "message": "Model context limit reached. Conversation size exceeds model capacity."
                    }
                })),
            )
                .into_response()
        }),
    );
    let config = test_config(spawn_upstream(app).await);
    let req = Request::builder()
        .method("POST")
        .uri("/v1/responses")
        .header("content-type", "application/json")
        .body(Body::from(
            json!({
                "model": "test-model",
                "input": "long prompt sentinel"
            })
            .to_string(),
        ))
        .unwrap();

    let resp = responses(State(config), req).await;

    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
    let dump = fs::read_dir(&debug_dir)
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .filter_map(|path| serde_json::from_slice::<serde_json::Value>(&fs::read(path).ok()?).ok())
        .find(|dump| dump["original_request"]["input"] == "long prompt sentinel")
        .unwrap();
    assert_eq!(dump["original_request"]["input"], "long prompt sentinel");
    assert_eq!(
        dump["upstream_error"]["error"]["message"],
        "Model context limit reached. Conversation size exceeds model capacity."
    );

    unsafe {
        std::env::remove_var("OPEN_PROMUX_DEBUG_DIR");
    }
    let _ = fs::remove_dir_all(debug_dir);
}

#[tokio::test]
async fn responses_should_route_to_upstream_that_lists_requested_model() {
    let upstream_a_called = Arc::new(AtomicUsize::new(0));
    let upstream_b_called = Arc::new(AtomicUsize::new(0));
    let upstream_a = Router::new()
        .route(
            "/models",
            get(|| async {
                Json(json!({
                    "object": "list",
                    "data": [{"id": "model-a", "object": "model"}]
                }))
            }),
        )
        .route(
            "/chat/completions",
            post({
                let upstream_a_called = upstream_a_called.clone();
                move || {
                    let upstream_a_called = upstream_a_called.clone();
                    async move {
                        upstream_a_called.fetch_add(1, Ordering::SeqCst);
                        Json(json!({"error": "wrong upstream"})).into_response()
                    }
                }
            }),
        );
    let upstream_b = Router::new()
        .route(
            "/models",
            get(|| async {
                Json(json!({
                    "object": "list",
                    "data": [{"id": "model-b", "object": "model"}]
                }))
            }),
        )
        .route(
            "/chat/completions",
            post({
                let upstream_b_called = upstream_b_called.clone();
                move || {
                    let upstream_b_called = upstream_b_called.clone();
                    async move {
                        upstream_b_called.fetch_add(1, Ordering::SeqCst);
                        Json(json!({
                            "id": "chatcmpl_b",
                            "model": "model-b",
                            "choices": [{
                                "index": 0,
                                "message": {"role": "assistant", "content": "from b"},
                                "finish_reason": "stop"
                            }]
                        }))
                        .into_response()
                    }
                }
            }),
        );
    let config = test_multi_config(vec![
        spawn_upstream(upstream_a).await,
        spawn_upstream(upstream_b).await,
    ]);
    let req = Request::builder()
        .method("POST")
        .uri("/v1/responses")
        .header("content-type", "application/json")
        .body(Body::from(
            json!({
                "model": "model-b",
                "input": "hello"
            })
            .to_string(),
        ))
        .unwrap();

    let resp = responses(State(config), req).await;
    let status = resp.status();
    let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX)
        .await
        .unwrap();
    let body: serde_json::Value = serde_json::from_slice(&bytes).unwrap();

    assert_eq!(status, StatusCode::OK);
    assert_eq!(upstream_a_called.load(Ordering::SeqCst), 0);
    assert_eq!(upstream_b_called.load(Ordering::SeqCst), 1);
    assert_eq!(body["output"][0]["content"][0]["text"], "from b");
}

#[tokio::test]
async fn chat_completions_should_route_model_alias_and_send_target_model_upstream() {
    let upstream_a_called = Arc::new(AtomicUsize::new(0));
    let upstream_b_called = Arc::new(AtomicUsize::new(0));
    let upstream_b_seen_model = Arc::new(Mutex::new(None::<String>));
    let upstream_a = Router::new()
        .route(
            "/models",
            get(|| async {
                Json(json!({
                    "object": "list",
                    "data": [{"id": "wrong-model", "object": "model"}]
                }))
            }),
        )
        .route(
            "/chat/completions",
            post({
                let upstream_a_called = upstream_a_called.clone();
                move || {
                    let upstream_a_called = upstream_a_called.clone();
                    async move {
                        upstream_a_called.fetch_add(1, Ordering::SeqCst);
                        Json(json!({"error": "wrong upstream"})).into_response()
                    }
                }
            }),
        );
    let upstream_b = Router::new()
        .route(
            "/models",
            get(|| async {
                Json(json!({
                    "object": "list",
                    "data": [{"id": "qwen3-coder", "object": "model"}]
                }))
            }),
        )
        .route(
            "/chat/completions",
            post({
                let upstream_b_called = upstream_b_called.clone();
                let upstream_b_seen_model = upstream_b_seen_model.clone();
                move |Json(body): Json<serde_json::Value>| {
                    let upstream_b_called = upstream_b_called.clone();
                    let upstream_b_seen_model = upstream_b_seen_model.clone();
                    async move {
                        upstream_b_called.fetch_add(1, Ordering::SeqCst);
                        *upstream_b_seen_model.lock().await =
                            body["model"].as_str().map(ToString::to_string);
                        Json(json!({
                            "id": "chatcmpl_b",
                            "model": body["model"],
                            "choices": [{
                                "index": 0,
                                "message": {"role": "assistant", "content": "from alias"},
                                "finish_reason": "stop"
                            }]
                        }))
                        .into_response()
                    }
                }
            }),
        );
    let upstream_a_url = spawn_upstream(upstream_a).await;
    let upstream_b_url = spawn_upstream(upstream_b).await;
    let config: Config = toml::from_str(&format!(
        r#"
[routing.model_aliases]
"gpt-5.5" = "local:qwen3-coder"

[[upstreams]]
name = "openai"
url = "{upstream_a_url}"

[[upstreams]]
name = "local"
url = "{upstream_b_url}"
"#
    ))
    .unwrap();

    let resp = chat_completions(State(test_app_state(config)), chat_model_request("gpt-5.5")).await;

    assert_eq!(resp.status(), StatusCode::OK);
    assert_eq!(upstream_a_called.load(Ordering::SeqCst), 0);
    assert_eq!(upstream_b_called.load(Ordering::SeqCst), 1);
    assert_eq!(
        upstream_b_seen_model.lock().await.as_deref(),
        Some("qwen3-coder")
    );
}

#[tokio::test]
async fn chat_completions_should_use_fallback_model_when_requested_model_is_missing() {
    let upstream_a_called = Arc::new(AtomicUsize::new(0));
    let upstream_b_called = Arc::new(AtomicUsize::new(0));
    let upstream_b_seen_model = Arc::new(Mutex::new(None::<String>));
    let upstream_a = Router::new()
        .route(
            "/models",
            get(|| async {
                Json(json!({
                    "object": "list",
                    "data": [{"id": "wrong-model", "object": "model"}]
                }))
            }),
        )
        .route(
            "/chat/completions",
            post({
                let upstream_a_called = upstream_a_called.clone();
                move || {
                    let upstream_a_called = upstream_a_called.clone();
                    async move {
                        upstream_a_called.fetch_add(1, Ordering::SeqCst);
                        Json(json!({"error": "wrong upstream"})).into_response()
                    }
                }
            }),
        );
    let upstream_b = Router::new()
        .route(
            "/models",
            get(|| async {
                Json(json!({
                    "object": "list",
                    "data": [{"id": "qwen3-coder", "object": "model"}]
                }))
            }),
        )
        .route(
            "/chat/completions",
            post({
                let upstream_b_called = upstream_b_called.clone();
                let upstream_b_seen_model = upstream_b_seen_model.clone();
                move |Json(body): Json<serde_json::Value>| {
                    let upstream_b_called = upstream_b_called.clone();
                    let upstream_b_seen_model = upstream_b_seen_model.clone();
                    async move {
                        upstream_b_called.fetch_add(1, Ordering::SeqCst);
                        *upstream_b_seen_model.lock().await =
                            body["model"].as_str().map(ToString::to_string);
                        Json(json!({
                            "id": "chatcmpl_b",
                            "model": body["model"],
                            "choices": [{
                                "index": 0,
                                "message": {"role": "assistant", "content": "from fallback"},
                                "finish_reason": "stop"
                            }]
                        }))
                        .into_response()
                    }
                }
            }),
        );
    let upstream_a_url = spawn_upstream(upstream_a).await;
    let upstream_b_url = spawn_upstream(upstream_b).await;
    let config: Config = toml::from_str(&format!(
        r#"
[routing]
fallback_model = "local:qwen3-coder"

[[upstreams]]
name = "openai"
url = "{upstream_a_url}"

[[upstreams]]
name = "local"
url = "{upstream_b_url}"
"#
    ))
    .unwrap();

    let resp = chat_completions(
        State(test_app_state(config)),
        chat_model_request("missing-model"),
    )
    .await;

    assert_eq!(resp.status(), StatusCode::OK);
    assert_eq!(upstream_a_called.load(Ordering::SeqCst), 0);
    assert_eq!(upstream_b_called.load(Ordering::SeqCst), 1);
    assert_eq!(
        upstream_b_seen_model.lock().await.as_deref(),
        Some("qwen3-coder")
    );
}

#[tokio::test]
async fn responses_should_reuse_cached_model_lists_for_repeated_plain_model_routing() {
    let upstream_a_model_calls = Arc::new(AtomicUsize::new(0));
    let upstream_b_model_calls = Arc::new(AtomicUsize::new(0));
    let upstream_b_called = Arc::new(AtomicUsize::new(0));
    let upstream_a = Router::new().route(
        "/models",
        get({
            let upstream_a_model_calls = upstream_a_model_calls.clone();
            move || {
                let upstream_a_model_calls = upstream_a_model_calls.clone();
                async move {
                    upstream_a_model_calls.fetch_add(1, Ordering::SeqCst);
                    Json(json!({
                        "object": "list",
                        "data": [{"id": "model-a", "object": "model"}]
                    }))
                }
            }
        }),
    );
    let upstream_b = Router::new()
        .route(
            "/models",
            get({
                let upstream_b_model_calls = upstream_b_model_calls.clone();
                move || {
                    let upstream_b_model_calls = upstream_b_model_calls.clone();
                    async move {
                        upstream_b_model_calls.fetch_add(1, Ordering::SeqCst);
                        Json(json!({
                            "object": "list",
                            "data": [{"id": "model-b", "object": "model"}]
                        }))
                    }
                }
            }),
        )
        .route(
            "/chat/completions",
            post({
                let upstream_b_called = upstream_b_called.clone();
                move || {
                    let upstream_b_called = upstream_b_called.clone();
                    async move {
                        upstream_b_called.fetch_add(1, Ordering::SeqCst);
                        Json(json!({
                            "id": "chatcmpl_b",
                            "model": "model-b",
                            "choices": [{
                                "index": 0,
                                "message": {"role": "assistant", "content": "from b"},
                                "finish_reason": "stop"
                            }]
                        }))
                    }
                }
            }),
        );
    let config = test_multi_config(vec![
        spawn_upstream(upstream_a).await,
        spawn_upstream(upstream_b).await,
    ]);

    let first = responses(State(config.clone()), responses_model_request("model-b")).await;
    let second = responses(State(config), responses_model_request("model-b")).await;

    assert_eq!(first.status(), StatusCode::OK);
    assert_eq!(second.status(), StatusCode::OK);
    assert_eq!(upstream_a_model_calls.load(Ordering::SeqCst), 1);
    assert_eq!(upstream_b_model_calls.load(Ordering::SeqCst), 1);
    assert_eq!(upstream_b_called.load(Ordering::SeqCst), 2);
}

#[tokio::test]
async fn startup_should_prefetch_model_lists_before_first_routed_request() {
    let upstream_a_model_calls = Arc::new(AtomicUsize::new(0));
    let upstream_b_model_calls = Arc::new(AtomicUsize::new(0));
    let upstream_b_seen_model = Arc::new(Mutex::new(None));
    let upstream_a = Router::new().route(
        "/models",
        get({
            let upstream_a_model_calls = upstream_a_model_calls.clone();
            move || {
                let upstream_a_model_calls = upstream_a_model_calls.clone();
                async move {
                    upstream_a_model_calls.fetch_add(1, Ordering::SeqCst);
                    Json(json!({
                        "object": "list",
                        "data": [{"id": "model-a", "object": "model"}]
                    }))
                }
            }
        }),
    );
    let upstream_b = Router::new()
        .route(
            "/models",
            get({
                let upstream_b_model_calls = upstream_b_model_calls.clone();
                move || {
                    let upstream_b_model_calls = upstream_b_model_calls.clone();
                    async move {
                        upstream_b_model_calls.fetch_add(1, Ordering::SeqCst);
                        Json(json!({
                            "object": "list",
                            "data": [{"id": "model-b", "object": "model"}]
                        }))
                    }
                }
            }),
        )
        .route(
            "/chat/completions",
            post({
                let upstream_b_seen_model = upstream_b_seen_model.clone();
                move |Json(body): Json<serde_json::Value>| {
                    let upstream_b_seen_model = upstream_b_seen_model.clone();
                    async move {
                        *upstream_b_seen_model.lock().await = body
                            .get("model")
                            .and_then(|model| model.as_str())
                            .map(str::to_string);
                        Json(json!({
                            "id": "chatcmpl_b",
                            "model": "model-b",
                            "choices": [{
                                "index": 0,
                                "message": {"role": "assistant", "content": "from b"},
                                "finish_reason": "stop"
                            }]
                        }))
                    }
                }
            }),
        );
    let state = test_multi_config(vec![
        spawn_upstream(upstream_a).await,
        spawn_upstream(upstream_b).await,
    ]);

    for _ in 0..20 {
        if upstream_a_model_calls.load(Ordering::SeqCst) == 1
            && upstream_b_model_calls.load(Ordering::SeqCst) == 1
        {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }

    assert_eq!(upstream_a_model_calls.load(Ordering::SeqCst), 1);
    assert_eq!(upstream_b_model_calls.load(Ordering::SeqCst), 1);

    let resp = chat_completions(State(state), chat_model_request("model-b")).await;

    assert_eq!(resp.status(), StatusCode::OK);
    assert_eq!(upstream_a_model_calls.load(Ordering::SeqCst), 1);
    assert_eq!(upstream_b_model_calls.load(Ordering::SeqCst), 1);
    assert_eq!(
        upstream_b_seen_model.lock().await.as_deref(),
        Some("model-b")
    );
}

#[tokio::test]
async fn chat_completions_should_round_robin_between_upstreams_that_expose_the_same_model() {
    let upstream_a_called = Arc::new(AtomicUsize::new(0));
    let upstream_b_called = Arc::new(AtomicUsize::new(0));
    let upstream_a = Router::new()
        .route(
            "/models",
            get(|| async {
                Json(json!({
                    "object": "list",
                    "data": [{"id": "shared-model", "object": "model"}]
                }))
            }),
        )
        .route(
            "/chat/completions",
            post({
                let upstream_a_called = upstream_a_called.clone();
                move || {
                    let upstream_a_called = upstream_a_called.clone();
                    async move {
                        upstream_a_called.fetch_add(1, Ordering::SeqCst);
                        Json(json!({
                            "id": "chatcmpl_a",
                            "model": "shared-model",
                            "choices": [{
                                "index": 0,
                                "message": {"role": "assistant", "content": "from a"},
                                "finish_reason": "stop"
                            }]
                        }))
                    }
                }
            }),
        );
    let upstream_b = Router::new()
        .route(
            "/models",
            get(|| async {
                Json(json!({
                    "object": "list",
                    "data": [{"id": "shared-model", "object": "model"}]
                }))
            }),
        )
        .route(
            "/chat/completions",
            post({
                let upstream_b_called = upstream_b_called.clone();
                move || {
                    let upstream_b_called = upstream_b_called.clone();
                    async move {
                        upstream_b_called.fetch_add(1, Ordering::SeqCst);
                        Json(json!({
                            "id": "chatcmpl_b",
                            "model": "shared-model",
                            "choices": [{
                                "index": 0,
                                "message": {"role": "assistant", "content": "from b"},
                                "finish_reason": "stop"
                            }]
                        }))
                    }
                }
            }),
        );
    let upstream_a_url = spawn_upstream(upstream_a).await;
    let upstream_b_url = spawn_upstream(upstream_b).await;
    let config: Config = toml::from_str(&format!(
        r#"
[routing]
load_balance = "round_robin"

[[upstreams]]
url = "{upstream_a_url}"

[[upstreams]]
url = "{upstream_b_url}"
"#
    ))
    .unwrap();
    let state = test_app_state(config);

    let first = chat_completions(State(state.clone()), chat_model_request("shared-model")).await;
    let second = chat_completions(State(state), chat_model_request("shared-model")).await;

    assert_eq!(first.status(), StatusCode::OK);
    assert_eq!(second.status(), StatusCode::OK);
    assert_eq!(upstream_a_called.load(Ordering::SeqCst), 1);
    assert_eq!(upstream_b_called.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn chat_completions_should_fail_over_to_next_matching_upstream_when_enabled() {
    let upstream_a_called = Arc::new(AtomicUsize::new(0));
    let upstream_b_called = Arc::new(AtomicUsize::new(0));
    let upstream_a = Router::new()
        .route(
            "/models",
            get(|| async {
                Json(json!({
                    "object": "list",
                    "data": [{"id": "shared-model", "object": "model"}]
                }))
            }),
        )
        .route(
            "/chat/completions",
            post({
                let upstream_a_called = upstream_a_called.clone();
                move || {
                    let upstream_a_called = upstream_a_called.clone();
                    async move {
                        upstream_a_called.fetch_add(1, Ordering::SeqCst);
                        (
                            StatusCode::BAD_GATEWAY,
                            Json(json!({"error": "temporary failure"})),
                        )
                    }
                }
            }),
        );
    let upstream_b = Router::new()
        .route(
            "/models",
            get(|| async {
                Json(json!({
                    "object": "list",
                    "data": [{"id": "shared-model", "object": "model"}]
                }))
            }),
        )
        .route(
            "/chat/completions",
            post({
                let upstream_b_called = upstream_b_called.clone();
                move || {
                    let upstream_b_called = upstream_b_called.clone();
                    async move {
                        upstream_b_called.fetch_add(1, Ordering::SeqCst);
                        Json(json!({
                            "id": "chatcmpl_b",
                            "model": "shared-model",
                            "choices": [{
                                "index": 0,
                                "message": {"role": "assistant", "content": "from b"},
                                "finish_reason": "stop"
                            }]
                        }))
                        .into_response()
                    }
                }
            }),
        );
    let upstream_a_url = spawn_upstream(upstream_a).await;
    let upstream_b_url = spawn_upstream(upstream_b).await;
    let config: Config = toml::from_str(&format!(
        r#"
[routing]
automatic_failover = true

[[upstreams]]
url = "{upstream_a_url}"

[[upstreams]]
url = "{upstream_b_url}"
"#
    ))
    .unwrap();

    let resp = chat_completions(
        State(test_app_state(config)),
        chat_model_request("shared-model"),
    )
    .await;

    assert_eq!(resp.status(), StatusCode::OK);
    assert_eq!(upstream_a_called.load(Ordering::SeqCst), 4);
    assert_eq!(upstream_b_called.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn responses_should_fail_over_to_next_matching_upstream_when_enabled() {
    let upstream_a_called = Arc::new(AtomicUsize::new(0));
    let upstream_b_called = Arc::new(AtomicUsize::new(0));
    let upstream_a = Router::new()
        .route(
            "/models",
            get(|| async {
                Json(json!({
                    "object": "list",
                    "data": [{"id": "test-model", "object": "model"}]
                }))
            }),
        )
        .route(
            "/chat/completions",
            post({
                let upstream_a_called = upstream_a_called.clone();
                move || {
                    let upstream_a_called = upstream_a_called.clone();
                    async move {
                        upstream_a_called.fetch_add(1, Ordering::SeqCst);
                        (
                            StatusCode::BAD_GATEWAY,
                            Json(json!({"error": "temporary failure"})),
                        )
                    }
                }
            }),
        );
    let upstream_b = Router::new()
        .route(
            "/models",
            get(|| async {
                Json(json!({
                    "object": "list",
                    "data": [{"id": "test-model", "object": "model"}]
                }))
            }),
        )
        .route(
            "/chat/completions",
            post({
                let upstream_b_called = upstream_b_called.clone();
                move || {
                    let upstream_b_called = upstream_b_called.clone();
                    async move {
                        upstream_b_called.fetch_add(1, Ordering::SeqCst);
                        Json(json!({
                            "id": "chatcmpl_b",
                            "model": "test-model",
                            "choices": [{
                                "index": 0,
                                "message": {"role": "assistant", "content": "from b"},
                                "finish_reason": "stop"
                            }]
                        }))
                        .into_response()
                    }
                }
            }),
        );
    let upstream_a_url = spawn_upstream(upstream_a).await;
    let upstream_b_url = spawn_upstream(upstream_b).await;
    let config: Config = toml::from_str(&format!(
        r#"
[routing]
automatic_failover = true

[[upstreams]]
url = "{upstream_a_url}"

[[upstreams]]
url = "{upstream_b_url}"
"#
    ))
    .unwrap();

    let resp = responses(State(test_app_state(config)), responses_request(false)).await;
    let status = resp.status();
    let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX)
        .await
        .unwrap();
    let body: serde_json::Value = serde_json::from_slice(&bytes).unwrap();

    assert_eq!(status, StatusCode::OK);
    assert_eq!(upstream_a_called.load(Ordering::SeqCst), 4);
    assert_eq!(upstream_b_called.load(Ordering::SeqCst), 1);
    assert_eq!(body["output"][0]["content"][0]["text"], "from b");
}

#[tokio::test]
async fn chat_completions_should_skip_unhealthy_upstreams_when_health_check_is_enabled() {
    let upstream_a_called = Arc::new(AtomicUsize::new(0));
    let upstream_b_called = Arc::new(AtomicUsize::new(0));
    let upstream_a = Router::new()
        .route(
            "/models",
            get(|| async { (StatusCode::BAD_GATEWAY, Json(json!({"error": "unhealthy"}))) }),
        )
        .route(
            "/chat/completions",
            post({
                let upstream_a_called = upstream_a_called.clone();
                move || {
                    let upstream_a_called = upstream_a_called.clone();
                    async move {
                        upstream_a_called.fetch_add(1, Ordering::SeqCst);
                        Json(json!({"error": "wrong upstream"})).into_response()
                    }
                }
            }),
        );
    let upstream_b = Router::new()
        .route(
            "/models",
            get(|| async {
                Json(json!({
                    "object": "list",
                    "data": [{"id": "test-model", "object": "model"}]
                }))
            }),
        )
        .route(
            "/chat/completions",
            post({
                let upstream_b_called = upstream_b_called.clone();
                move || {
                    let upstream_b_called = upstream_b_called.clone();
                    async move {
                        upstream_b_called.fetch_add(1, Ordering::SeqCst);
                        Json(json!({
                            "id": "chatcmpl_b",
                            "model": "test-model",
                            "choices": [{
                                "index": 0,
                                "message": {"role": "assistant", "content": "from b"},
                                "finish_reason": "stop"
                            }]
                        }))
                        .into_response()
                    }
                }
            }),
        );
    let upstream_a_url = spawn_upstream(upstream_a).await;
    let upstream_b_url = spawn_upstream(upstream_b).await;
    let config: Config = toml::from_str(&format!(
        r#"
[health]
enabled = true
interval_millis = 25
unhealthy_after_failures = 1

[[upstreams]]
url = "{upstream_a_url}"

[[upstreams]]
url = "{upstream_b_url}"
"#
    ))
    .unwrap();
    let state = test_app_state(config);
    tokio::time::sleep(std::time::Duration::from_millis(80)).await;

    let resp = chat_completions(State(state), chat_request_without_model()).await;

    assert_eq!(resp.status(), StatusCode::OK);
    assert_eq!(upstream_a_called.load(Ordering::SeqCst), 0);
    assert_eq!(upstream_b_called.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn models_should_send_bearer_authorization_when_using_openai_auth_header() {
    let app = Router::new().route(
        "/models",
        get(|headers: axum::http::HeaderMap| async move {
            Json(json!({
                "authorization": headers
                    .get("authorization")
                    .and_then(|value| value.to_str().ok())
                    .unwrap_or("")
            }))
        }),
    );
    let config = test_app_state(Config {
        port: 0,
        auth_key: None,
        performance: crate::config::PerformanceConfig::default(),
        routing: crate::config::RoutingConfig::default(),
        health: crate::config::HealthConfig::default(),
        rectifier: crate::config::RectifierConfig::default(),
        upstream: Some(crate::config::UpstreamConfig {
            name: None,
            url: spawn_upstream(app).await,
            api_key: "test-key".into(),
            auth_header: "Authorization".into(),
            proxy: None,
            proxy_type: crate::config::UpstreamProxyType::Http,
            api_format: crate::config::UpstreamApiFormat::ChatCompletions,
            max_concurrent_requests: None,
            rpm: None,
            tpm: None,
        }),
        upstreams: Vec::new(),
    });

    let resp = models(State(config), HeaderMap::new()).await;
    let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX)
        .await
        .unwrap();
    let body: serde_json::Value = serde_json::from_slice(&bytes).unwrap();

    assert_eq!(body["authorization"], "Bearer test-key");
}

#[tokio::test]
async fn models_should_not_duplicate_existing_bearer_authorization_prefix() {
    let app = Router::new().route(
        "/models",
        get(|headers: axum::http::HeaderMap| async move {
            Json(json!({
                "authorization": headers
                    .get("authorization")
                    .and_then(|value| value.to_str().ok())
                    .unwrap_or("")
            }))
        }),
    );
    let config = test_app_state(Config {
        port: 0,
        auth_key: None,
        performance: crate::config::PerformanceConfig::default(),
        routing: crate::config::RoutingConfig::default(),
        health: crate::config::HealthConfig::default(),
        rectifier: crate::config::RectifierConfig::default(),
        upstream: Some(crate::config::UpstreamConfig {
            name: None,
            url: spawn_upstream(app).await,
            api_key: "Bearer test-key".into(),
            auth_header: "Authorization".into(),
            proxy: None,
            proxy_type: crate::config::UpstreamProxyType::Http,
            api_format: crate::config::UpstreamApiFormat::ChatCompletions,
            max_concurrent_requests: None,
            rpm: None,
            tpm: None,
        }),
        upstreams: Vec::new(),
    });

    let resp = models(State(config), HeaderMap::new()).await;
    let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX)
        .await
        .unwrap();
    let body: serde_json::Value = serde_json::from_slice(&bytes).unwrap();

    assert_eq!(body["authorization"], "Bearer test-key");
}

#[tokio::test]
async fn models_should_make_initial_request_then_retry_three_times_and_return_final_upstream_error_body()
 {
    let attempts = Arc::new(AtomicUsize::new(0));
    let app = Router::new().route(
        "/models",
        get({
            let attempts = attempts.clone();
            move || {
                let attempts = attempts.clone();
                async move {
                    attempts.fetch_add(1, Ordering::SeqCst);
                    (
                        StatusCode::BAD_GATEWAY,
                        Json(json!({"error": "upstream failed"})),
                    )
                        .into_response()
                }
            }
        }),
    );
    let config = test_config(spawn_upstream(app).await);

    let resp = models(State(config), HeaderMap::new()).await;
    let status = resp.status();
    let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX)
        .await
        .unwrap();

    assert_eq!(status, StatusCode::BAD_GATEWAY);
    assert_eq!(attempts.load(Ordering::SeqCst), 4);
    assert_eq!(
        serde_json::from_slice::<serde_json::Value>(&bytes).unwrap(),
        json!({"error": "upstream failed"})
    );
}

// ── /v1/messages (Anthropic Messages downstream protocol) ──

fn anthropic_messages_request(stream: bool) -> Request<Body> {
    Request::builder()
        .method("POST")
        .uri("/v1/messages")
        .header("content-type", "application/json")
        .body(Body::from(
            serde_json::to_vec(&json!({
                "model": "claude-test",
                "max_tokens": 256,
                "stream": stream,
                "messages": [
                    {"role": "user", "content": "Hello"}
                ]
            }))
            .unwrap(),
        ))
        .unwrap()
}

#[tokio::test]
async fn messages_should_passthrough_to_anthropic_messages_upstream() {
    let seen = Arc::new(Mutex::new(None::<serde_json::Value>));
    let seen_clone = seen.clone();
    let app = Router::new().route(
        "/messages",
        post(move |Json(body): Json<serde_json::Value>| {
            let seen = seen_clone.clone();
            async move {
                *seen.lock().await = Some(body);
                Json(json!({
                    "id": "msg_1",
                    "type": "message",
                    "role": "assistant",
                    "model": "claude-test",
                    "content": [{"type": "text", "text": "Hi back"}],
                    "stop_reason": "end_turn"
                }))
            }
        }),
    );
    let config = test_app_state(Config {
        port: 0,
        auth_key: None,
        performance: crate::config::PerformanceConfig::default(),
        routing: crate::config::RoutingConfig::default(),
        health: crate::config::HealthConfig::default(),
        rectifier: crate::config::RectifierConfig::default(),
        upstream: Some(crate::config::UpstreamConfig {
            name: None,
            url: spawn_upstream(app).await,
            api_key: "anthropic-key".into(),
            auth_header: "x-api-key".into(),
            proxy: None,
            proxy_type: crate::config::UpstreamProxyType::Http,
            api_format: crate::config::UpstreamApiFormat::AnthropicMessages,
            max_concurrent_requests: None,
            rpm: None,
            tpm: None,
        }),
        upstreams: Vec::new(),
    });

    let resp = messages(State(config), anthropic_messages_request(false)).await;
    let status = resp.status();
    let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX)
        .await
        .unwrap();
    let body: serde_json::Value = serde_json::from_slice(&bytes).unwrap();

    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["role"], "assistant");
    assert_eq!(body["content"][0]["text"], "Hi back");

    let seen = seen.lock().await.clone().unwrap();
    assert_eq!(seen["model"], "claude-test");
    assert_eq!(seen["messages"][0]["role"], "user");
    assert_eq!(seen["messages"][0]["content"], "Hello");
}

#[tokio::test]
async fn messages_should_translate_anthropic_request_to_chat_completions_upstream() {
    let seen = Arc::new(Mutex::new(None::<serde_json::Value>));
    let seen_clone = seen.clone();
    let app = Router::new().route(
        "/chat/completions",
        post(move |Json(body): Json<serde_json::Value>| {
            let seen = seen_clone.clone();
            async move {
                *seen.lock().await = Some(body);
                Json(json!({
                    "id": "chatcmpl_1",
                    "model": "claude-test",
                    "choices": [{
                        "index": 0,
                        "message": {
                            "role": "assistant",
                            "content": "Hello from chat"
                        },
                        "finish_reason": "stop"
                    }],
                    "usage": {
                        "prompt_tokens": 5,
                        "completion_tokens": 3,
                        "total_tokens": 8
                    }
                }))
            }
        }),
    );
    let config = test_config(spawn_upstream(app).await);

    let resp = messages(State(config), anthropic_messages_request(false)).await;
    let status = resp.status();
    let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX)
        .await
        .unwrap();
    let body: serde_json::Value = serde_json::from_slice(&bytes).unwrap();

    assert_eq!(status, StatusCode::OK);
    // Anthropic-shaped response
    assert_eq!(body["type"], "message");
    assert_eq!(body["role"], "assistant");
    assert_eq!(body["content"][0]["type"], "text");
    assert_eq!(body["content"][0]["text"], "Hello from chat");
    assert_eq!(body["stop_reason"], "end_turn");
    assert_eq!(body["usage"]["input_tokens"], 5);
    assert_eq!(body["usage"]["output_tokens"], 3);

    // Chat upstream saw an OpenAI-shaped request
    let seen = seen.lock().await.clone().unwrap();
    assert_eq!(seen["model"], "claude-test");
    assert_eq!(seen["messages"][0]["role"], "user");
    assert_eq!(seen["messages"][0]["content"], "Hello");
}

// ── Responses upstream (api_format = "responses") ──

fn responses_upstream_config(upstream_url: String) -> Arc<AppState> {
    test_app_state(Config {
        port: 0,
        auth_key: None,
        performance: crate::config::PerformanceConfig::default(),
        routing: crate::config::RoutingConfig::default(),
        health: crate::config::HealthConfig::default(),
        rectifier: crate::config::RectifierConfig::default(),
        upstream: Some(crate::config::UpstreamConfig {
            name: None,
            url: upstream_url,
            api_key: String::new(),
            auth_header: "Authorization".into(),
            proxy: None,
            proxy_type: crate::config::UpstreamProxyType::Http,
            api_format: crate::config::UpstreamApiFormat::Responses,
            max_concurrent_requests: None,
            rpm: None,
            tpm: None,
        }),
        upstreams: Vec::new(),
    })
}

#[tokio::test]
async fn responses_should_passthrough_to_responses_upstream() {
    let seen = Arc::new(Mutex::new(None::<serde_json::Value>));
    let seen_clone = seen.clone();
    let app = Router::new().route(
        "/responses",
        post(move |Json(body): Json<serde_json::Value>| {
            let seen = seen_clone.clone();
            async move {
                *seen.lock().await = Some(body);
                Json(json!({
                    "id": "resp_123",
                    "object": "response",
                    "created_at": 1_700_000_000_u64,
                    "model": "test-model",
                    "status": "completed",
                    "output": [{
                        "type": "message",
                        "id": "msg_1",
                        "status": "completed",
                        "role": "assistant",
                        "content": [{"type": "output_text", "text": "Hello", "annotations": []}]
                    }],
                    "usage": {"input_tokens": 1, "output_tokens": 1, "total_tokens": 2}
                }))
            }
        }),
    );
    let config = responses_upstream_config(spawn_upstream(app).await);

    let resp = responses(State(config), responses_request(false)).await;
    let status = resp.status();
    let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX)
        .await
        .unwrap();
    let body: serde_json::Value = serde_json::from_slice(&bytes).unwrap();

    assert_eq!(status, StatusCode::OK);
    // Passthrough preserves the upstream id verbatim.
    assert_eq!(body["id"], "resp_123");
    assert_eq!(body["output"][0]["content"][0]["text"], "Hello");

    let seen = seen.lock().await.clone().unwrap();
    // The downstream Responses-shaped body reached the upstream unchanged.
    assert_eq!(seen["model"], "test-model");
    assert!(seen.get("messages").is_none());
    assert!(seen.get("input").is_some());
}

#[tokio::test]
async fn messages_should_translate_to_responses_upstream() {
    let seen = Arc::new(Mutex::new(None::<serde_json::Value>));
    let seen_clone = seen.clone();
    let app = Router::new().route(
        "/responses",
        post(move |Json(body): Json<serde_json::Value>| {
            let seen = seen_clone.clone();
            async move {
                *seen.lock().await = Some(body);
                Json(json!({
                    "id": "resp_abc",
                    "object": "response",
                    "created_at": 1_700_000_000_u64,
                    "model": "claude-test",
                    "status": "completed",
                    "output": [{
                        "type": "message",
                        "id": "msg_1",
                        "status": "completed",
                        "role": "assistant",
                        "content": [{"type": "output_text", "text": "Howdy", "annotations": []}]
                    }],
                    "usage": {"input_tokens": 4, "output_tokens": 2, "total_tokens": 6}
                }))
            }
        }),
    );
    let config = responses_upstream_config(spawn_upstream(app).await);

    let resp = messages(State(config), anthropic_messages_request(false)).await;
    let status = resp.status();
    let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX)
        .await
        .unwrap();
    let body: serde_json::Value = serde_json::from_slice(&bytes).unwrap();

    assert_eq!(status, StatusCode::OK);
    // Anthropic-shaped response built from the Responses upstream's output[].
    assert_eq!(body["type"], "message");
    assert_eq!(body["role"], "assistant");
    assert_eq!(body["content"][0]["type"], "text");
    assert_eq!(body["content"][0]["text"], "Howdy");
    assert_eq!(body["stop_reason"], "end_turn");
    assert_eq!(body["usage"]["input_tokens"], 4);
    assert_eq!(body["usage"]["output_tokens"], 2);

    // Upstream saw a Responses-shaped request translated from the Anthropic
    // downstream body.
    let seen = seen.lock().await.clone().unwrap();
    assert!(seen.get("input").is_some());
    assert!(seen.get("messages").is_none());
    assert_eq!(seen["model"], "claude-test");
    assert_eq!(seen["max_output_tokens"], 256);
}

#[tokio::test]
async fn chat_completions_should_reject_streaming_non_chat_upstream_with_501() {
    // chat downstream + responses upstream + streaming → still 501.
    let app = Router::new().route(
        "/responses",
        post(|| async {
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                "should not have been called",
            )
        }),
    );
    let config = responses_upstream_config(spawn_upstream(app).await);

    let req = Request::builder()
        .method("POST")
        .uri("/v1/chat/completions")
        .header("content-type", "application/json")
        .body(Body::from(
            serde_json::to_vec(&json!({
                "model": "test-model",
                "stream": true,
                "messages": [{"role": "user", "content": "Hi"}]
            }))
            .unwrap(),
        ))
        .unwrap();

    let resp = chat_completions(State(config), req).await;
    assert_eq!(resp.status(), StatusCode::NOT_IMPLEMENTED);
    let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX)
        .await
        .unwrap();
    let text = String::from_utf8_lossy(&bytes);
    assert!(text.contains("/v1/responses"));
}

#[tokio::test]
async fn chat_completions_should_bridge_to_responses_upstream_non_streaming() {
    // chat downstream + responses upstream (non-streaming) ✅
    let seen = Arc::new(Mutex::new(None::<serde_json::Value>));
    let seen_clone = seen.clone();
    let app = Router::new().route(
        "/responses",
        post(move |Json(body): Json<serde_json::Value>| {
            let seen = seen_clone.clone();
            async move {
                *seen.lock().await = Some(body);
                Json(json!({
                    "id": "resp_x",
                    "object": "response",
                    "created_at": 1_700_000_000_u64,
                    "model": "test-model",
                    "status": "completed",
                    "output": [{
                        "type": "message",
                        "id": "msg_1",
                        "status": "completed",
                        "role": "assistant",
                        "content": [{"type": "output_text", "text": "Hello chat", "annotations": []}]
                    }],
                    "usage": {"input_tokens": 7, "output_tokens": 4, "total_tokens": 11}
                }))
            }
        }),
    );
    let config = responses_upstream_config(spawn_upstream(app).await);

    let req = Request::builder()
        .method("POST")
        .uri("/v1/chat/completions")
        .header("content-type", "application/json")
        .body(Body::from(
            serde_json::to_vec(&json!({
                "model": "test-model",
                "messages": [
                    {"role": "system", "content": "Be brief."},
                    {"role": "user", "content": "Hi"}
                ]
            }))
            .unwrap(),
        ))
        .unwrap();

    let resp = chat_completions(State(config), req).await;
    assert_eq!(resp.status(), StatusCode::OK);
    let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX)
        .await
        .unwrap();
    let body: serde_json::Value = serde_json::from_slice(&bytes).unwrap();

    // ChatCompletion shape
    assert_eq!(body["object"], "chat.completion");
    assert_eq!(body["choices"][0]["message"]["role"], "assistant");
    assert_eq!(body["choices"][0]["message"]["content"], "Hello chat");
    assert_eq!(body["choices"][0]["finish_reason"], "stop");
    assert_eq!(body["usage"]["prompt_tokens"], 7);
    assert_eq!(body["usage"]["completion_tokens"], 4);

    let seen = seen.lock().await.clone().unwrap();
    assert!(seen.get("input").is_some());
    assert_eq!(seen["instructions"], "Be brief.");
}

#[tokio::test]
async fn messages_should_reject_streaming_with_chat_completions_upstream() {
    // No upstream call expected; the endpoint should short-circuit with 501.
    let app = Router::new().route(
        "/chat/completions",
        post(|| async {
            // If we ever reach the upstream the test fails.
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                "should not have been called",
            )
        }),
    );
    let config = test_config(spawn_upstream(app).await);

    let resp = messages(State(config), anthropic_messages_request(true)).await;
    assert_eq!(resp.status(), StatusCode::NOT_IMPLEMENTED);
    let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX)
        .await
        .unwrap();
    let text = String::from_utf8_lossy(&bytes);
    assert!(text.contains("Streaming"));
    assert!(text.contains("anthropic_messages"));
}
