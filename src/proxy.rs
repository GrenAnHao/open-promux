use axum::{
    body::Body,
    extract::{Request, State},
    http::StatusCode,
    response::{IntoResponse, Response},
};
use futures::StreamExt;
use reqwest::{Client, RequestBuilder};
use std::{
    sync::Arc,
    time::{Duration, Instant},
};

use crate::config::Config;
use crate::convert;
use crate::types::*;

const MAX_RETRY_ATTEMPTS: usize = 3;

fn upstream_client() -> Client {
    Client::builder()
        .no_proxy()
        .build()
        .expect("failed to build upstream HTTP client")
}

fn apply_upstream_auth(builder: RequestBuilder, config: &Config) -> RequestBuilder {
    if config.upstream.api_key.is_empty() {
        builder
    } else if config
        .upstream
        .auth_header
        .eq_ignore_ascii_case("authorization")
        && !config
            .upstream
            .api_key
            .to_ascii_lowercase()
            .starts_with("bearer ")
    {
        builder.header(
            &config.upstream.auth_header,
            format!("Bearer {}", config.upstream.api_key),
        )
    } else {
        builder.header(&config.upstream.auth_header, &config.upstream.api_key)
    }
}

fn should_retry_status(status: StatusCode) -> bool {
    status == StatusCode::FORBIDDEN
        || status == StatusCode::TOO_MANY_REQUESTS
        || status.is_server_error()
}

async fn retry_delay(attempt: usize) {
    tokio::time::sleep(Duration::from_millis((attempt as u64) * 250)).await;
}

async fn send_with_retries<F>(
    label: &str,
    mut build: F,
) -> Result<reqwest::Response, reqwest::Error>
where
    F: FnMut() -> RequestBuilder,
{
    for attempt in 1..=MAX_RETRY_ATTEMPTS {
        tracing::info!("{label} upstream request attempt {attempt}/{MAX_RETRY_ATTEMPTS}");

        match build().send().await {
            Ok(resp) => {
                let status =
                    StatusCode::from_u16(resp.status().as_u16()).unwrap_or(StatusCode::BAD_GATEWAY);

                if should_retry_status(status) && attempt < MAX_RETRY_ATTEMPTS {
                    tracing::warn!(
                        "{label} upstream request attempt {attempt}/{MAX_RETRY_ATTEMPTS} returned retryable status {status}; retrying"
                    );
                    retry_delay(attempt).await;
                    continue;
                }

                return Ok(resp);
            }
            Err(e) if attempt < MAX_RETRY_ATTEMPTS => {
                tracing::warn!(
                    "{label} upstream request attempt {attempt}/{MAX_RETRY_ATTEMPTS} failed: {e}; retrying"
                );
                retry_delay(attempt).await;
            }
            Err(e) => return Err(e),
        }
    }

    unreachable!()
}

pub async fn chat_completions(State(config): State<Arc<Config>>, req: Request<Body>) -> Response {
    let start = Instant::now();
    let (parts, body) = req.into_parts();

    let body_bytes = match axum::body::to_bytes(body, usize::MAX).await {
        Ok(b) => b,
        Err(e) => {
            tracing::error!("failed to read request body: {e}");
            return (StatusCode::BAD_REQUEST, "failed to read body").into_response();
        }
    };

    let is_stream = {
        let v: serde_json::Value = serde_json::from_slice(&body_bytes).unwrap_or_default();
        v.get("stream").and_then(|s| s.as_bool()).unwrap_or(false)
    };

    tracing::info!("[passthrough] POST /v1/chat/completions stream={is_stream}");

    let upstream_url = config.upstream.url.trim_end_matches('/');
    let target = format!("{upstream_url}/chat/completions");
    tracing::info!("[passthrough] -> upstream: {target}");

    let client = upstream_client();
    let upstream_resp = match send_with_retries("[passthrough]", || {
        let mut builder = apply_upstream_auth(
            client
                .post(&target)
                .header("content-type", "application/json"),
            &config,
        );

        for (key, value) in parts.headers.iter() {
            if key == "host" || key == "authorization" {
                continue;
            }
            if let Ok(v) = value.to_str() {
                builder = builder.header(key.as_str(), v);
            }
        }

        builder.body(body_bytes.clone())
    })
    .await
    {
        Ok(r) => r,
        Err(e) => {
            tracing::error!("[passthrough] upstream request failed: {e}");
            return (StatusCode::BAD_GATEWAY, "upstream request failed").into_response();
        }
    };

    let status =
        StatusCode::from_u16(upstream_resp.status().as_u16()).unwrap_or(StatusCode::BAD_GATEWAY);

    tracing::info!("[passthrough] upstream responded: {status}");

    if is_stream && status.is_success() {
        tracing::info!(
            "[passthrough] streaming response, elapsed={}ms",
            start.elapsed().as_millis()
        );
        let stream = upstream_resp.bytes_stream();
        let body = Body::from_stream(stream.map(|r| r.map_err(std::io::Error::other)));

        return Response::builder()
            .status(status)
            .header("content-type", "text/event-stream")
            .header("cache-control", "no-cache")
            .body(body)
            .unwrap();
    }

    match upstream_resp.bytes().await {
        Ok(bytes) => {
            if status.is_client_error() || status.is_server_error() {
                tracing::warn!(
                    "[passthrough] upstream error: {}",
                    String::from_utf8_lossy(&bytes)
                );
            }
            tracing::info!(
                "[passthrough] done, {}B, elapsed={}ms",
                bytes.len(),
                start.elapsed().as_millis()
            );
            let mut resp = Response::new(Body::from(bytes));
            *resp.status_mut() = status;
            resp.headers_mut()
                .insert("content-type", "application/json".parse().unwrap());
            resp
        }
        Err(e) => {
            tracing::error!("[passthrough] failed to read upstream response: {e}");
            (StatusCode::BAD_GATEWAY, "failed to read upstream response").into_response()
        }
    }
}

pub async fn responses(State(config): State<Arc<Config>>, req: Request<Body>) -> Response {
    let start = Instant::now();
    let (_parts, body) = req.into_parts();

    let body_bytes = match axum::body::to_bytes(body, usize::MAX).await {
        Ok(b) => b,
        Err(e) => {
            tracing::error!("failed to read request body: {e}");
            return (StatusCode::BAD_REQUEST, "failed to read body").into_response();
        }
    };

    let responses_req: ResponsesRequest = match serde_json::from_slice(&body_bytes) {
        Ok(r) => r,
        Err(e) => {
            tracing::error!("[responses] failed to parse request: {e}");
            return (StatusCode::BAD_REQUEST, format!("invalid request: {e}")).into_response();
        }
    };

    let is_stream = responses_req.stream.unwrap_or(false);
    tracing::info!(
        "[responses] POST /v1/responses model={} stream={is_stream}",
        responses_req.model
    );

    let chat_req = convert::responses_to_chat(&responses_req);

    let chat_body = match serde_json::to_vec(&chat_req) {
        Ok(b) => b,
        Err(e) => {
            tracing::error!("[responses] failed to serialize chat request: {e}");
            return (StatusCode::INTERNAL_SERVER_ERROR, "conversion error").into_response();
        }
    };

    tracing::info!(
        "[responses] converted to ChatCompletions: {}",
        String::from_utf8_lossy(&chat_body)
    );

    let upstream_url = config.upstream.url.trim_end_matches('/');
    let target = format!("{upstream_url}/chat/completions");
    tracing::info!("[responses] -> upstream: {target}");

    let client = upstream_client();
    let upstream_resp = match send_with_retries("[responses]", || {
        apply_upstream_auth(
            client
                .post(&target)
                .header("content-type", "application/json"),
            &config,
        )
        .body(chat_body.clone())
    })
    .await
    {
        Ok(r) => r,
        Err(e) => {
            tracing::error!("[responses] upstream request failed: {e}");
            return (StatusCode::BAD_GATEWAY, "upstream request failed").into_response();
        }
    };

    let status =
        StatusCode::from_u16(upstream_resp.status().as_u16()).unwrap_or(StatusCode::BAD_GATEWAY);

    tracing::info!("[responses] upstream responded: {status}");

    if status.is_client_error() || status.is_server_error() {
        let err_bytes = upstream_resp.bytes().await.unwrap_or_default();
        tracing::warn!(
            "[responses] upstream error: {}",
            String::from_utf8_lossy(&err_bytes)
        );
        let mut resp = Response::new(Body::from(err_bytes));
        *resp.status_mut() = status;
        resp.headers_mut()
            .insert("content-type", "application/json".parse().unwrap());
        return resp;
    }

    // ── Non-streaming ──
    if !is_stream {
        return match upstream_resp.bytes().await {
            Ok(bytes) => {
                let chat_resp: ChatResponse = match serde_json::from_slice(&bytes) {
                    Ok(r) => r,
                    Err(e) => {
                        tracing::error!("[responses] failed to parse upstream response: {e}");
                        return (
                            StatusCode::BAD_GATEWAY,
                            format!("invalid upstream response: {e}"),
                        )
                            .into_response();
                    }
                };

                let responses_resp = convert::chat_to_responses(&chat_resp);
                let out = serde_json::to_vec(&responses_resp).unwrap();
                tracing::info!(
                    "[responses] done, output items={}, {}B, elapsed={}ms",
                    responses_resp.output.len(),
                    out.len(),
                    start.elapsed().as_millis()
                );
                let mut resp = Response::new(Body::from(out));
                *resp.status_mut() = StatusCode::OK;
                resp.headers_mut()
                    .insert("content-type", "application/json".parse().unwrap());
                resp
            }
            Err(e) => {
                tracing::error!("[responses] failed to read upstream response: {e}");
                (StatusCode::BAD_GATEWAY, "failed to read upstream response").into_response()
            }
        };
    }

    // ── Streaming ──
    tracing::info!("[responses] starting stream conversion");
    let stream = upstream_resp.bytes_stream();
    let model = responses_req.model.clone();

    let transformed = futures::stream::unfold(
        (
            stream,
            convert::StreamState::new(),
            convert::SseDecoder::new(),
            model,
            false,
            Vec::new(),
            None::<ChatUsage>,
            false,
        ),
        |(
            mut stream,
            mut state,
            mut decoder,
            model,
            mut started,
            mut pending,
            mut last_usage,
            mut completed,
        )| async move {
            loop {
                // drain pending
                if !pending.is_empty() {
                    let event = pending.remove(0);
                    return Some((
                        Ok::<_, std::io::Error>(event),
                        (
                            stream, state, decoder, model, started, pending, last_usage, completed,
                        ),
                    ));
                }

                // read next chunk from upstream
                match stream.next().await {
                    Some(Ok(chunk_bytes)) => {
                        for data in decoder.push(&chunk_bytes) {
                            if data.trim() == "[DONE]" {
                                tracing::info!("[responses] stream: upstream sent [DONE]");
                                if !completed {
                                    let finish_events = convert::convert_stream_finish(&mut state);
                                    pending.extend(finish_events);
                                    let end_event =
                                        convert::convert_stream_end(&state, last_usage.as_ref());
                                    pending.push(end_event);
                                    completed = true;
                                }
                                continue;
                            }

                            let chunk: ChatChunk = match serde_json::from_str(&data) {
                                Ok(c) => c,
                                Err(e) => {
                                    tracing::warn!(
                                        "[responses] stream: failed to parse chunk: {e}"
                                    );
                                    continue;
                                }
                            };

                            // save usage if present
                            if chunk.usage.is_some() {
                                last_usage = chunk.usage.clone();
                            }

                            // emit stream start on first chunk
                            if !started {
                                tracing::info!(
                                    "[responses] stream: first chunk received, emitting start events"
                                );
                                let start_events =
                                    convert::convert_stream_start(&mut state, &model);
                                pending.extend(start_events);
                                started = true;
                            }

                            let chunk_events = convert::convert_stream_chunk(&mut state, &chunk);
                            pending.extend(chunk_events);
                        }
                    }
                    Some(Err(e)) => {
                        tracing::error!("[responses] upstream stream error: {e}");
                        return None;
                    }
                    None => {
                        tracing::info!("[responses] stream: upstream connection closed");
                        if started && !completed {
                            let finish_events = convert::convert_stream_finish(&mut state);
                            pending.extend(finish_events);
                            let end_event =
                                convert::convert_stream_end(&state, last_usage.as_ref());
                            pending.push(end_event);
                            completed = true;
                            continue;
                        }
                        return None;
                    }
                }
            }
        },
    );

    let body = Body::from_stream(transformed.map(|r| r.map_err(std::io::Error::other)));

    Response::builder()
        .status(StatusCode::OK)
        .header("content-type", "text/event-stream")
        .header("cache-control", "no-cache")
        .body(body)
        .unwrap()
}

pub async fn models(State(config): State<Arc<Config>>) -> Response {
    let upstream_url = config.upstream.url.trim_end_matches('/');
    let target = format!("{upstream_url}/models");
    tracing::info!("[models] GET /v1/models -> upstream: {target}");

    let client = upstream_client();
    let upstream_resp = match send_with_retries("[models]", || {
        apply_upstream_auth(client.get(&target), &config)
    })
    .await
    {
        Ok(r) => r,
        Err(e) => {
            tracing::error!("[models] upstream request failed: {e}");
            return (StatusCode::BAD_GATEWAY, "upstream request failed").into_response();
        }
    };

    let status =
        StatusCode::from_u16(upstream_resp.status().as_u16()).unwrap_or(StatusCode::BAD_GATEWAY);

    match upstream_resp.bytes().await {
        Ok(bytes) => {
            if status.is_client_error() || status.is_server_error() {
                tracing::warn!(
                    "[models] upstream error: {}",
                    String::from_utf8_lossy(&bytes)
                );
            }
            let mut resp = Response::new(Body::from(bytes));
            *resp.status_mut() = status;
            resp.headers_mut()
                .insert("content-type", "application/json".parse().unwrap());
            resp
        }
        Err(e) => {
            tracing::error!("[models] failed to read upstream response: {e}");
            (StatusCode::BAD_GATEWAY, "failed to read upstream response").into_response()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::{
        Json, Router,
        body::Bytes,
        routing::{get, post},
    };
    use serde_json::json;
    use std::{
        convert::Infallible,
        sync::atomic::{AtomicUsize, Ordering},
    };

    async fn spawn_upstream(app: Router) -> String {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });
        format!("http://{addr}")
    }

    fn test_config(upstream_url: String) -> Arc<Config> {
        Arc::new(Config {
            port: 0,
            upstream: crate::config::UpstreamConfig {
                url: upstream_url,
                api_key: String::new(),
                auth_header: "Authorization".into(),
            },
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

        let resp = models(State(config)).await;
        let status = resp.status();
        let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        let body: serde_json::Value = serde_json::from_slice(&bytes).unwrap();

        assert_eq!(status, StatusCode::OK);
        assert_eq!(body["data"][0]["id"], "test-model");
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
        let config = Arc::new(Config {
            port: 0,
            upstream: crate::config::UpstreamConfig {
                url: spawn_upstream(app).await,
                api_key: "test-key".into(),
                auth_header: "Authorization".into(),
            },
        });

        let resp = models(State(config)).await;
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
        let config = Arc::new(Config {
            port: 0,
            upstream: crate::config::UpstreamConfig {
                url: spawn_upstream(app).await,
                api_key: "Bearer test-key".into(),
                auth_header: "Authorization".into(),
            },
        });

        let resp = models(State(config)).await;
        let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        let body: serde_json::Value = serde_json::from_slice(&bytes).unwrap();

        assert_eq!(body["authorization"], "Bearer test-key");
    }

    #[tokio::test]
    async fn models_should_retry_retryable_status_three_times_and_return_final_upstream_error_body()
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

        let resp = models(State(config)).await;
        let status = resp.status();
        let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();

        assert_eq!(status, StatusCode::BAD_GATEWAY);
        assert_eq!(attempts.load(Ordering::SeqCst), 3);
        assert_eq!(
            serde_json::from_slice::<serde_json::Value>(&bytes).unwrap(),
            json!({"error": "upstream failed"})
        );
    }
}
