use super::*;

pub async fn chat_completions(State(state): State<Arc<AppState>>, req: Request<Body>) -> Response {
    let start = Instant::now();
    let (parts, body) = req.into_parts();

    if !is_proxy_authorized(&state.config, &parts.headers) {
        return unauthorized_response();
    }

    let body_bytes = match axum::body::to_bytes(body, usize::MAX).await {
        Ok(b) => b,
        Err(e) => {
            tracing::error!("failed to read request body: {e}");
            return (StatusCode::BAD_REQUEST, "failed to read body").into_response();
        }
    };

    let request_json: serde_json::Value = serde_json::from_slice(&body_bytes).unwrap_or_default();
    let request_tokens = estimate_tokens_from_bytes(&body_bytes);
    if state
        .check_global_rate_limits(request_tokens)
        .await
        .is_err()
    {
        return (StatusCode::TOO_MANY_REQUESTS, "rate limit exceeded").into_response();
    }
    let is_stream = request_json
        .get("stream")
        .and_then(|s| s.as_bool())
        .unwrap_or(false);
    let model = request_json.get("model").and_then(|m| m.as_str());

    tracing::info!("[passthrough] POST /v1/chat/completions stream={is_stream}");

    let selections = select_upstreams_for_model(&state, model).await;
    if selections.is_empty() {
        return (
            StatusCode::NOT_FOUND,
            "model not found in configured upstreams",
        )
            .into_response();
    }
    let last_selection_index = selections.len() - 1;

    for (selection_index, selection) in selections.into_iter().enumerate() {
        let can_failover =
            state.config.routing.automatic_failover && selection_index < last_selection_index;
        let upstream = selection.upstream;
        let upstream_config = &upstream.config;
        log_model_route("[passthrough]", &selection);

        // Cross-format `/v1/chat/completions` against non-chat upstreams is
        // not yet implemented. Surface a clear `501` so callers know to use
        // `/v1/responses` (which already supports both directions).
        if upstream_config.api_format != crate::config::UpstreamApiFormat::ChatCompletions {
            tracing::warn!(
                "[passthrough] /v1/chat/completions against non-ChatCompletions upstream is not yet implemented"
            );
            return (
                StatusCode::NOT_IMPLEMENTED,
                "/v1/chat/completions against an `anthropic_messages` or `responses` upstream is not yet implemented. \
                 Use `/v1/responses` (which bridges all three formats), or set the upstream's `api_format` to `chat_completions`.",
            )
                .into_response();
        }

        if upstream.check_rate_limits(request_tokens).await.is_err() {
            if can_failover {
                tracing::warn!("[passthrough] upstream rate limit exceeded; failing over");
                continue;
            }
            return (StatusCode::TOO_MANY_REQUESTS, "rate limit exceeded").into_response();
        }

        let upstream_url = upstream_config.url.trim_end_matches('/');
        let target = format!("{upstream_url}/chat/completions");
        log_upstream_target("[passthrough]", upstream_config, &target);
        let upstream_body = if let Some(upstream_model) = selection.upstream_model.as_ref() {
            let mut upstream_json = request_json.clone();
            if let Some(obj) = upstream_json.as_object_mut() {
                obj.insert(
                    "model".into(),
                    serde_json::Value::String(upstream_model.clone()),
                );
            }
            serde_json::to_vec(&upstream_json).unwrap_or_else(|_| body_bytes.to_vec())
        } else {
            body_bytes.to_vec()
        };

        let upstream_permit = upstream.acquire_permit().await;
        let upstream_resp = match send_with_retries("[passthrough]", upstream_config, || {
            let mut builder = apply_upstream_auth(
                upstream
                    .client
                    .post(&target)
                    .header("content-type", "application/json"),
                upstream_config,
            );

            for (key, value) in parts.headers.iter() {
                if key == "host" || key == "authorization" {
                    continue;
                }
                if let Ok(v) = value.to_str() {
                    builder = builder.header(key.as_str(), v);
                }
            }

            builder.body(upstream_body.clone())
        })
        .await
        {
            Ok(r) => r,
            Err(e) => {
                if can_failover {
                    tracing::warn!("[passthrough] upstream request failed: {e}; failing over");
                    continue;
                }
                tracing::error!("[passthrough] upstream request failed: {e}");
                return (StatusCode::BAD_GATEWAY, "upstream request failed").into_response();
            }
        };

        let status = StatusCode::from_u16(upstream_resp.status().as_u16())
            .unwrap_or(StatusCode::BAD_GATEWAY);
        tracing::debug!(
            "[passthrough] upstream protocol: {:?}",
            upstream_resp.version()
        );

        log_upstream_status("[passthrough]", &selection, status);

        if can_failover && should_retry_status(status) {
            tracing::warn!("[passthrough] upstream returned {status}; failing over");
            continue;
        }

        if is_stream && status.is_success() {
            tracing::info!(
                "[passthrough] streaming response, elapsed={}ms",
                start.elapsed().as_millis()
            );
            let stream = upstream_resp.bytes_stream();
            let body = Body::from_stream(stream.map(move |r| {
                let _upstream_permit = &upstream_permit;
                r.map_err(std::io::Error::other)
            }));

            return Response::builder()
                .status(status)
                .header("content-type", "text/event-stream")
                .header("cache-control", "no-cache")
                .body(body)
                .unwrap();
        }

        return match upstream_resp.bytes().await {
            Ok(bytes) => {
                if status.is_client_error() || status.is_server_error() {
                    tracing::warn!(
                        "[passthrough] upstream error: {}",
                        String::from_utf8_lossy(&bytes)
                    );
                    dump_upstream_error_debug(
                        "[passthrough]",
                        status,
                        upstream_config,
                        &target,
                        &request_json,
                        &upstream_body,
                        &bytes,
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
        };
    }

    (StatusCode::BAD_GATEWAY, "upstream request failed").into_response()
}
