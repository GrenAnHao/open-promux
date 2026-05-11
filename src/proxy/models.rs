use super::*;

pub async fn models(State(state): State<Arc<AppState>>, headers: HeaderMap) -> Response {
    if !is_proxy_authorized(&state.config, &headers) {
        return unauthorized_response();
    }

    let upstreams = &state.upstreams;

    if upstreams.is_empty() {
        return (StatusCode::BAD_GATEWAY, "no upstreams configured").into_response();
    }

    if upstreams.len() == 1 {
        let upstream = &upstreams[0];
        let upstream_config = &upstream.config;
        let upstream_url = upstream_config.url.trim_end_matches('/');
        let target = format!("{upstream_url}/models");
        log_upstream_target("[models] GET /v1/models", upstream_config, &target);
        let _upstream_permit = upstream.acquire_permit().await;

        let upstream_resp = match send_with_retries("[models]", upstream_config, || {
            apply_upstream_auth(upstream.client.get(&target), upstream_config)
        })
        .await
        {
            Ok(r) => r,
            Err(e) => {
                tracing::error!("[models] upstream request failed: {e}");
                return (StatusCode::BAD_GATEWAY, "upstream request failed").into_response();
            }
        };

        let status = StatusCode::from_u16(upstream_resp.status().as_u16())
            .unwrap_or(StatusCode::BAD_GATEWAY);
        tracing::debug!("[models] upstream protocol: {:?}", upstream_resp.version());

        return match upstream_resp.bytes().await {
            Ok(bytes) => {
                if status.is_client_error() || status.is_server_error() {
                    tracing::warn!(
                        "[models] upstream error: {}",
                        String::from_utf8_lossy(&bytes)
                    );
                }
                let bytes = if upstream_config.name.is_some()
                    || state.config.routing.expose_model_aliases
                {
                    match serde_json::from_slice::<serde_json::Value>(&bytes) {
                        Ok(mut body) => {
                            if let Some(name) = upstream_config.name.as_ref()
                                && let Some(items) =
                                    body.get_mut("data").and_then(|data| data.as_array_mut())
                            {
                                for item in items {
                                    prefix_model_item_id(item, name);
                                }
                            }
                            if state.config.routing.expose_model_aliases {
                                append_model_alias_items(
                                    &mut body,
                                    &state.config.routing.model_aliases,
                                );
                            }
                            serde_json::to_vec(&body).unwrap_or_else(|_| bytes.to_vec())
                        }
                        Err(_) => bytes.to_vec(),
                    }
                } else {
                    bytes.to_vec()
                };
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
        };
    }

    let fetched = futures::future::join_all(
        upstreams
            .iter()
            .map(|upstream| fetch_model_items_cached(upstream, "[models]")),
    )
    .await;
    let mut merged = Vec::new();
    for (upstream, result) in upstreams.iter().zip(fetched) {
        match result {
            Ok(snapshot) => {
                merged.extend(snapshot.items.iter().cloned().map(|mut item| {
                    if let Some(name) = upstream.config.name.as_ref() {
                        prefix_model_item_id(&mut item, name);
                    }
                    item
                }));
            }
            Err(status) => {
                return (status, "failed to fetch upstream models").into_response();
            }
        }
    }

    let body = serde_json::json!({
        "object": "list",
        "data": merged
    });
    let mut body = body;
    if state.config.routing.expose_model_aliases {
        append_model_alias_items(&mut body, &state.config.routing.model_aliases);
    }

    let mut resp = Response::new(Body::from(body.to_string()));
    *resp.status_mut() = StatusCode::OK;
    resp.headers_mut()
        .insert("content-type", "application/json".parse().unwrap());
    resp
}
