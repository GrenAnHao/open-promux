use super::*;
use std::collections::VecDeque;

pub async fn responses(State(state): State<Arc<AppState>>, req: Request<Body>) -> Response {
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

    let responses_req: ResponsesRequest = match serde_json::from_slice(&body_bytes) {
        Ok(r) => r,
        Err(e) => {
            tracing::error!("[responses] failed to parse request: {e}");
            return (StatusCode::BAD_REQUEST, format!("invalid request: {e}")).into_response();
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

    let is_stream = responses_req.stream.unwrap_or(false);
    tracing::info!(
        "[responses] POST /v1/responses model={} stream={is_stream}",
        responses_req.model
    );

    if state.config.debug.enabled && state.config.debug.log_conversations {
        dump_conversation_debug("[responses]", &body_bytes);
    }

    let chat_req = convert::responses_to_chat(&responses_req);

    let chat_body = match serde_json::to_vec(&chat_req) {
        Ok(b) => b,
        Err(e) => {
            tracing::error!("[responses] failed to serialize chat request: {e}");
            return (StatusCode::INTERNAL_SERVER_ERROR, "conversion error").into_response();
        }
    };

    let selections = select_upstreams_for_model(&state, Some(&responses_req.model)).await;
    if selections.is_empty() {
        return (
            StatusCode::NOT_FOUND,
            "model not found in configured upstreams",
        )
            .into_response();
    }
    let last_selection_index = selections.len() - 1;
    let mut selected_response = None;

    for (selection_index, selection) in selections.into_iter().enumerate() {
        let can_failover =
            state.config.routing.automatic_failover && selection_index < last_selection_index;
        let upstream = selection.upstream;
        let upstream_config = &upstream.config;
        log_model_route("[responses]", &selection);

        if upstream.check_rate_limits(request_tokens).await.is_err() {
            if can_failover {
                tracing::warn!("[responses] upstream rate limit exceeded; failing over");
                continue;
            }
            return (StatusCode::TOO_MANY_REQUESTS, "rate limit exceeded").into_response();
        }

        let upstream_model = selection
            .upstream_model
            .clone()
            .unwrap_or_else(|| responses_req.model.clone());
        let upstream_url = upstream_config.url.trim_end_matches('/');
        let (target, upstream_body, anthropic_req) = match upstream_config.api_format {
            UpstreamApiFormat::ChatCompletions => {
                let body = if upstream_model == chat_req.model {
                    chat_body.clone()
                } else {
                    let mut upstream_req = chat_req.clone();
                    upstream_req.model = upstream_model;
                    serde_json::to_vec(&upstream_req).unwrap_or_else(|_| chat_body.clone())
                };
                (format!("{upstream_url}/chat/completions"), body, None)
            }
            UpstreamApiFormat::AnthropicMessages => {
                let mut anthropic_req = convert::responses_to_anthropic(&responses_req);
                anthropic_req.model = upstream_model;
                let body = match serde_json::to_vec(&anthropic_req) {
                    Ok(body) => body,
                    Err(e) => {
                        tracing::error!("[responses] failed to serialize anthropic request: {e}");
                        return (StatusCode::INTERNAL_SERVER_ERROR, "conversion error")
                            .into_response();
                    }
                };
                (
                    format!("{upstream_url}/messages"),
                    body,
                    Some(anthropic_req),
                )
            }
            UpstreamApiFormat::Responses => {
                // Responses upstream → passthrough. We may rewrite `model`
                // when routing strips a `name:` prefix; otherwise the
                // original request body is forwarded byte-for-byte.
                let body = if upstream_model != responses_req.model {
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
                (format!("{upstream_url}/responses"), body, None)
            }
        };
        log_upstream_target("[responses]", upstream_config, &target);

        let upstream_permit = upstream.acquire_permit().await;
        let upstream_resp = match send_with_retries("[responses]", upstream_config, || {
            let builder = apply_upstream_auth(
                upstream
                    .client
                    .post(&target)
                    .header("content-type", "application/json"),
                upstream_config,
            );
            let builder = match upstream_config.api_format {
                UpstreamApiFormat::ChatCompletions | UpstreamApiFormat::Responses => builder,
                UpstreamApiFormat::AnthropicMessages => apply_anthropic_headers(builder),
            };
            builder.body(upstream_body.clone())
        })
        .await
        {
            Ok(r) => r,
            Err(e) => {
                if can_failover {
                    tracing::warn!("[responses] upstream request failed: {e}; failing over");
                    continue;
                }
                tracing::error!("[responses] upstream request failed: {e}");
                return (StatusCode::BAD_GATEWAY, "upstream request failed").into_response();
            }
        };

        let status = StatusCode::from_u16(upstream_resp.status().as_u16())
            .unwrap_or(StatusCode::BAD_GATEWAY);
        tracing::debug!(
            "[responses] upstream protocol: {:?}",
            upstream_resp.version()
        );

        log_upstream_status("[responses]", &selection, status);

        if can_failover && should_retry_status(status) {
            tracing::warn!("[responses] upstream returned {status}; failing over");
            continue;
        }

        selected_response = Some((
            upstream_resp,
            status,
            upstream_permit,
            upstream_config.api_format,
            upstream_config,
            upstream,
            target,
            upstream_body,
            anthropic_req,
        ));
        break;
    }

    let Some((
        upstream_resp,
        status,
        upstream_permit,
        upstream_api_format,
        upstream_config,
        upstream,
        target,
        upstream_body,
        anthropic_req,
    )) = selected_response
    else {
        return (StatusCode::BAD_GATEWAY, "upstream request failed").into_response();
    };

    if status.is_client_error() || status.is_server_error() {
        let err_bytes = upstream_resp.bytes().await.unwrap_or_default();
        if upstream_api_format == UpstreamApiFormat::AnthropicMessages
            && let Some(anthropic_req) = anthropic_req
            && let Some(retry_body) =
                rectify_anthropic_retry_body(&state.config.rectifier, anthropic_req, &err_bytes)
        {
            tracing::info!("[responses] anthropic rectifier triggered; retrying same upstream");
            let retry_resp = match send_with_retries("[responses]", upstream_config, || {
                apply_anthropic_headers(apply_upstream_auth(
                    upstream
                        .client
                        .post(&target)
                        .header("content-type", "application/json"),
                    upstream_config,
                ))
                .body(retry_body.clone())
            })
            .await
            {
                Ok(resp) => resp,
                Err(e) => {
                    tracing::error!("[responses] anthropic rectifier retry failed: {e}");
                    return (StatusCode::BAD_GATEWAY, "upstream request failed").into_response();
                }
            };
            let retry_status = StatusCode::from_u16(retry_resp.status().as_u16())
                .unwrap_or(StatusCode::BAD_GATEWAY);
            if !retry_status.is_client_error() && !retry_status.is_server_error() {
                if is_stream {
                    return handle_streaming_response(
                        retry_resp,
                        upstream_api_format,
                        responses_req.model.clone(),
                        upstream_permit,
                    );
                }
                return handle_non_streaming_response(
                    retry_resp,
                    retry_status,
                    upstream_api_format,
                    start,
                )
                .await;
            }
            let retry_err_bytes = retry_resp.bytes().await.unwrap_or_default();
            dump_upstream_error_debug(
                "[responses]",
                retry_status,
                upstream_config,
                &target,
                &request_json,
                &retry_body,
                &retry_err_bytes,
            );
            let mut resp = Response::new(Body::from(retry_err_bytes));
            *resp.status_mut() = retry_status;
            resp.headers_mut()
                .insert("content-type", "application/json".parse().unwrap());
            return resp;
        }
        tracing::warn!(
            "[responses] upstream error: {}",
            String::from_utf8_lossy(&err_bytes)
        );
        dump_upstream_error_debug(
            "[responses]",
            status,
            upstream_config,
            &target,
            &request_json,
            &upstream_body,
            &err_bytes,
        );
        record_request_metric(
            &state,
            upstream_config,
            Some(&responses_req.model),
            false,
            body_bytes.len() as u64,
            err_bytes.len() as u64,
            start.elapsed().as_millis() as u64,
        )
        .await;
        let mut resp = Response::new(Body::from(err_bytes));
        *resp.status_mut() = status;
        resp.headers_mut()
            .insert("content-type", "application/json".parse().unwrap());
        return resp;
    }

    // ── Non-streaming ──
    if !is_stream {
        let response =
            handle_non_streaming_response(upstream_resp, status, upstream_api_format, start).await;
        let response_status = response.status();
        record_request_metric(
            &state,
            upstream_config,
            Some(&responses_req.model),
            response_status.is_success(),
            body_bytes.len() as u64,
            0,
            start.elapsed().as_millis() as u64,
        )
        .await;
        return response;
    }

    record_request_metric(
        &state,
        upstream_config,
        Some(&responses_req.model),
        true,
        body_bytes.len() as u64,
        0,
        start.elapsed().as_millis() as u64,
    )
    .await;
    handle_streaming_response(
        upstream_resp,
        upstream_api_format,
        responses_req.model.clone(),
        upstream_permit,
    )
}

fn handle_streaming_response(
    upstream_resp: reqwest::Response,
    upstream_api_format: UpstreamApiFormat,
    model: String,
    upstream_permit: Option<tokio::sync::OwnedSemaphorePermit>,
) -> Response {
    if upstream_api_format == UpstreamApiFormat::Responses {
        tracing::info!("[responses] streaming passthrough (responses upstream)");
        let stream = upstream_resp.bytes_stream();
        let body = Body::from_stream(stream.map(move |r| {
            let _upstream_permit = &upstream_permit;
            r.map_err(std::io::Error::other)
        }));
        return Response::builder()
            .status(StatusCode::OK)
            .header("content-type", "text/event-stream")
            .header("cache-control", "no-cache")
            .body(body)
            .unwrap();
    }

    tracing::info!("[responses] starting stream conversion");
    let stream = upstream_resp.bytes_stream();

    let transformed = futures::stream::unfold(
        (
            stream,
            convert::StreamState::new(),
            convert::SseDecoder::new(),
            upstream_api_format,
            model,
            false,
            VecDeque::new(),
            None::<ChatUsage>,
            false,
        ),
        |(
            mut stream,
            mut state,
            mut decoder,
            upstream_api_format,
            model,
            mut started,
            mut pending,
            mut last_usage,
            mut completed,
        )| async move {
            loop {
                // drain pending
                if let Some(event) = pending.pop_front() {
                    return Some((
                        Ok::<_, std::io::Error>(event),
                        (
                            stream,
                            state,
                            decoder,
                            upstream_api_format,
                            model,
                            started,
                            pending,
                            last_usage,
                            completed,
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
                                    pending.push_back(end_event);
                                    completed = true;
                                }
                                continue;
                            }

                            match upstream_api_format {
                                UpstreamApiFormat::Responses => {
                                    // Unreachable: Responses upstream is
                                    // forwarded byte-for-byte before this
                                    // state machine is constructed.
                                    unreachable!(
                                        "responses passthrough should not reach SSE decoder"
                                    );
                                }
                                UpstreamApiFormat::ChatCompletions => {
                                    let chunk: ChatChunk = match serde_json::from_str(&data) {
                                        Ok(c) => c,
                                        Err(e) => {
                                            tracing::warn!(
                                                "[responses] stream: failed to parse chunk: {e}"
                                            );
                                            continue;
                                        }
                                    };

                                    if chunk.usage.is_some() {
                                        last_usage = chunk.usage.clone();
                                    }

                                    if !started {
                                        tracing::info!(
                                            "[responses] stream: first chunk received, emitting start events"
                                        );
                                        let start_events =
                                            convert::convert_stream_start(&mut state, &model);
                                        pending.extend(start_events);
                                        started = true;
                                    }

                                    let chunk_events =
                                        convert::convert_stream_chunk(&mut state, &chunk);
                                    pending.extend(chunk_events);
                                }
                                UpstreamApiFormat::AnthropicMessages => {
                                    let event: AnthropicStreamEvent = match serde_json::from_str(
                                        &data,
                                    ) {
                                        Ok(event) => event,
                                        Err(e) => {
                                            tracing::warn!(
                                                "[responses] stream: failed to parse anthropic event: {e}"
                                            );
                                            continue;
                                        }
                                    };

                                    if let Some(usage) =
                                        convert::anthropic_stream_event_usage(&event)
                                    {
                                        last_usage = Some(usage);
                                    }

                                    if !started {
                                        tracing::info!(
                                            "[responses] stream: first chunk received, emitting start events"
                                        );
                                        let start_model =
                                            convert::anthropic_stream_event_model(&event)
                                                .unwrap_or(&model);
                                        let start_events =
                                            convert::convert_stream_start(&mut state, start_model);
                                        pending.extend(start_events);
                                        started = true;
                                    }

                                    if let Some(chunk) =
                                        convert::anthropic_stream_event_to_chat_chunk(
                                            &event, &model,
                                        )
                                    {
                                        let chunk_events =
                                            convert::convert_stream_chunk(&mut state, &chunk);
                                        pending.extend(chunk_events);
                                    }

                                    if convert::anthropic_stream_event_is_stop(&event) && !completed
                                    {
                                        let finish_events =
                                            convert::convert_stream_finish(&mut state);
                                        pending.extend(finish_events);
                                        let end_event = convert::convert_stream_end(
                                            &state,
                                            last_usage.as_ref(),
                                        );
                                        pending.push_back(end_event);
                                        completed = true;
                                    }
                                }
                            }
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
                            pending.push_back(end_event);
                            completed = true;
                            continue;
                        }
                        return None;
                    }
                }
            }
        },
    );

    let body = Body::from_stream(transformed.map(move |r| {
        let _upstream_permit = &upstream_permit;
        r.map_err(std::io::Error::other)
    }));

    Response::builder()
        .status(StatusCode::OK)
        .header("content-type", "text/event-stream")
        .header("cache-control", "no-cache")
        .body(body)
        .unwrap()
}

async fn handle_non_streaming_response(
    upstream_resp: reqwest::Response,
    status: StatusCode,
    upstream_api_format: UpstreamApiFormat,
    start: Instant,
) -> Response {
    match upstream_resp.bytes().await {
        Ok(bytes) => {
            // Responses upstream → just pass the JSON through; the upstream
            // already speaks Responses API so no shape conversion is needed.
            if upstream_api_format == UpstreamApiFormat::Responses {
                tracing::info!(
                    "[responses] done (passthrough), {}B, status={status}, elapsed={}ms",
                    bytes.len(),
                    start.elapsed().as_millis()
                );
                let mut resp = Response::new(Body::from(bytes));
                *resp.status_mut() = status;
                resp.headers_mut()
                    .insert("content-type", "application/json".parse().unwrap());
                return resp;
            }

            let responses_resp = match upstream_api_format {
                UpstreamApiFormat::ChatCompletions => {
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
                    convert::chat_to_responses(&chat_resp)
                }
                UpstreamApiFormat::AnthropicMessages => {
                    let anthropic_resp: AnthropicResponse = match serde_json::from_slice(&bytes) {
                        Ok(r) => r,
                        Err(e) => {
                            tracing::error!(
                                "[responses] failed to parse anthropic upstream response: {e}"
                            );
                            return (
                                StatusCode::BAD_GATEWAY,
                                format!("invalid upstream response: {e}"),
                            )
                                .into_response();
                        }
                    };
                    convert::anthropic_to_responses(&anthropic_resp)
                }
                UpstreamApiFormat::Responses => {
                    unreachable!("responses passthrough handled before this match")
                }
            };
            let out = serde_json::to_vec(&responses_resp).unwrap();
            tracing::info!(
                "[responses] done, status={status}, output items={}, {}B, elapsed={}ms",
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
    }
}
