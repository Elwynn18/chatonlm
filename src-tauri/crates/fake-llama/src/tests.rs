//! Essais du faux serveur : réponses lues par le routeur lui-même, formes
//! comparées à celles relevées sur le vrai `llama-server` (b12110).

use futures_util::StreamExt;

use super::*;

/// État d'un serveur prêt (ou en chargement), sans écoute réseau.
fn server(config: Config) -> Arc<Shared> {
    Shared::new(config).0
}

async fn call(
    shared: &Arc<Shared>,
    method: Method,
    path: &str,
    key: Option<&str>,
    body: Value,
) -> Response {
    let mut headers = HeaderMap::new();
    if let Some(key) = key {
        headers.insert(
            header::AUTHORIZATION,
            format!("Bearer {key}").parse().unwrap(),
        );
    }
    let body = if body.is_null() {
        Bytes::new()
    } else {
        Bytes::from(body.to_string())
    };
    handle(
        State(shared.clone()),
        method,
        path.parse().unwrap(),
        headers,
        body,
    )
    .await
}

async fn read(response: Response) -> (u16, Value) {
    let status = response.status().as_u16();
    let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    (
        status,
        serde_json::from_slice(&bytes).unwrap_or(Value::Null),
    )
}

/// Blocs `data:` d'un flux SSE entier.
async fn events(response: Response) -> Vec<String> {
    assert_eq!(
        response.headers()[header::CONTENT_TYPE],
        "text/event-stream"
    );
    let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let text = String::from_utf8(bytes.to_vec()).unwrap();
    assert!(text.ends_with("\n\n"));
    text.split("\n\n")
        .filter(|event| !event.is_empty())
        .map(|event| event.strip_prefix("data: ").expect(event).to_owned())
        .collect()
}

fn user(text: &str) -> Value {
    json!({ "messages": [{ "role": "user", "content": text }] })
}

fn with(mut base: Value, extra: Value) -> Value {
    if let (Value::Object(base), Value::Object(extra)) = (&mut base, extra) {
        base.extend(extra);
    }
    base
}

#[tokio::test]
async fn health_is_public_and_says_loading_first() {
    let shared = server(
        Config::new("m.gguf")
            .api_key("k")
            .mode(Mode::Loading(Duration::from_millis(150))),
    );
    let (status, body) = read(call(&shared, Method::GET, "/health", None, Value::Null).await).await;
    assert_eq!(status, 503);
    assert_eq!(
        body,
        json!({ "error": { "code": 503, "message": "Loading model", "type": "unavailable_error" } })
    );
    // Comme le vrai (server-http.cpp, relevé sur b12110) : l'état avant la
    // clé, pendant le chargement 503 même sans clé ou avec une mauvaise.
    for key in [None, Some("bad"), Some("k")] {
        let (status, _) = read(call(&shared, Method::GET, "/props", key, Value::Null).await).await;
        assert_eq!(status, 503, "{key:?}");
    }
    tokio::time::sleep(Duration::from_millis(200)).await;
    let (status, body) =
        read(call(&shared, Method::GET, "/v1/health", None, Value::Null).await).await;
    assert_eq!((status, body), (200, json!({ "status": "ok" })));
    // Puis la clé.
    let (status, _) =
        read(call(&shared, Method::GET, "/props", Some("bad"), Value::Null).await).await;
    assert_eq!(status, 401);
}

#[tokio::test]
async fn the_api_key_is_checked_like_the_real_server() {
    let shared = server(Config::new("m.gguf").api_key("secret"));
    let (status, body) = read(
        call(
            &shared,
            Method::GET,
            "/v1/models",
            Some("nope"),
            Value::Null,
        )
        .await,
    )
    .await;
    assert_eq!(status, 401);
    assert_eq!(body["error"]["message"], "Invalid API Key");
    assert_eq!(body["error"]["type"], "authentication_error");
    let mut headers = HeaderMap::new();
    headers.insert("x-api-key", "secret".parse().unwrap());
    let response = handle(
        State(shared.clone()),
        Method::GET,
        "/slots".parse().unwrap(),
        headers,
        Bytes::new(),
    )
    .await;
    assert_eq!(response.status(), 200);
    let (status, body) = read(
        call(
            &shared,
            Method::GET,
            "/api/tags",
            Some("secret"),
            Value::Null,
        )
        .await,
    )
    .await;
    assert_eq!(status, 404);
    assert_eq!(body["error"]["message"], "File Not Found");
    // Pilotage sans clé.
    let (status, _) =
        read(call(&shared, Method::GET, "/fake/requests", None, Value::Null).await).await;
    assert_eq!(status, 200);
}

#[tokio::test]
async fn a_stream_has_the_shape_of_the_real_one() {
    let shared = server(Config::new("/m/Qwen3.gguf"));
    let request = with(
        user("Hello world"),
        json!({ "stream": true, "stream_options": { "include_usage": true } }),
    );
    let events =
        events(call(&shared, Method::POST, "/v1/chat/completions", None, request).await).await;
    assert_eq!(events.last().unwrap(), "[DONE]");
    let chunks: Vec<Value> = events[..events.len() - 1]
        .iter()
        .map(|e| serde_json::from_str(e).unwrap())
        .collect();
    let first = &chunks[0];
    assert_eq!(
        first["choices"][0]["delta"],
        json!({ "role": "assistant", "content": null })
    );
    assert_eq!(first["object"], "chat.completion.chunk");
    assert_eq!(first["system_fingerprint"], BUILD_INFO);
    assert_eq!(first["model"], "/m/Qwen3.gguf");
    assert!(first["choices"][0]["finish_reason"].is_null());
    let text: String = chunks
        .iter()
        .filter_map(|c| c["choices"][0]["delta"]["content"].as_str())
        .collect();
    assert_eq!(text, "Hello! You said: Hello world");
    let finish = &chunks[chunks.len() - 2];
    assert_eq!(finish["choices"][0]["finish_reason"], "stop");
    assert_eq!(finish["choices"][0]["delta"], json!({}));
    assert!(finish.get("timings").is_none());
    let usage = &chunks[chunks.len() - 1];
    assert_eq!(usage["choices"], json!([]));
    assert_eq!(usage["usage"]["completion_tokens"], 5);
    let prompt = usage["usage"]["prompt_tokens"].as_u64().unwrap();
    assert_eq!(
        prompt,
        (2 + tokens::PER_MESSAGE + tokens::GENERATION_PROMPT) as u64
    );
    assert_eq!(usage["timings"]["predicted_n"], 5);
    assert_eq!(
        usage["timings"]["prompt_n"].as_u64().unwrap()
            + usage["timings"]["cache_n"].as_u64().unwrap(),
        prompt
    );
    // Sans include_usage : temps dans le bloc de fin, pas de bloc d'usage.
    let request = with(user("Hello world"), json!({ "stream": true }));
    let events = events_of(&shared, request).await;
    let finish: Value = serde_json::from_str(&events[events.len() - 2]).unwrap();
    assert_eq!(finish["choices"][0]["finish_reason"], "stop");
    assert_eq!(finish["timings"]["cache_n"], prompt - 1);
    assert!(finish.get("usage").is_none());
}

async fn events_of(shared: &Arc<Shared>, request: Value) -> Vec<String> {
    events(call(shared, Method::POST, "/v1/chat/completions", None, request).await).await
}

#[tokio::test]
async fn reasoning_and_tool_fragments_come_like_the_real_ones() {
    let shared = server(Config::new("chatonlm-think.gguf"));
    let events = events_of(&shared, with(user("2+2?"), json!({ "stream": true }))).await;
    let reasoning = events
        .iter()
        .filter(|e| e.contains("reasoning_content"))
        .count();
    assert_eq!(reasoning, 8);
    let off = with(
        user("2+2?"),
        json!({ "stream": true, "chat_template_kwargs": { "enable_thinking": false } }),
    );
    assert!(
        !events_of(&shared, off)
            .await
            .iter()
            .any(|e| e.contains("reasoning_content"))
    );

    let shared = server(Config::new("chatonlm-tools.gguf"));
    let tools = json!([{ "type": "function", "function": { "name": "get_weather",
        "parameters": { "type": "object", "properties": { "city": { "type": "string" } } } } }]);
    let events = events_of(
        &shared,
        with(user("Paris"), json!({ "stream": true, "tools": tools })),
    )
    .await;
    let calls: Vec<Value> = events
        .iter()
        .filter_map(|e| serde_json::from_str::<Value>(e).ok())
        .filter_map(|c| {
            c["choices"][0]["delta"]["tool_calls"][0]
                .as_object()
                .cloned()
                .map(Value::Object)
        })
        .collect();
    assert_eq!(calls[0]["index"], 0);
    assert_eq!(calls[0]["type"], "function");
    assert_eq!(calls[0]["function"]["name"], "get_weather");
    assert!(calls[0]["id"].is_string());
    assert!(calls[1].get("id").is_none() && calls[1]["function"].get("name").is_none());
    let arguments: String = calls
        .iter()
        .filter_map(|c| c["function"]["arguments"].as_str())
        .collect();
    assert_eq!(arguments, r#"{"city":"Paris"}"#);
    assert!(
        events
            .iter()
            .any(|e| e.contains(r#""finish_reason":"tool_calls""#))
    );
}

#[tokio::test]
async fn the_non_streamed_reply_has_the_shape_of_the_real_one() {
    let shared = server(Config::new("chatonlm-think.gguf"));
    let (status, body) = read(
        call(
            &shared,
            Method::POST,
            "/v1/chat/completions",
            None,
            user("Hi"),
        )
        .await,
    )
    .await;
    assert_eq!(status, 200);
    assert_eq!(body["object"], "chat.completion");
    let message = &body["choices"][0]["message"];
    assert_eq!(message["role"], "assistant");
    assert_eq!(message["content"], "Hello! You said: Hi");
    assert!(
        message["reasoning_content"]
            .as_str()
            .unwrap()
            .starts_with("Let me think")
    );
    assert_eq!(body["choices"][0]["finish_reason"], "stop");
    assert!(body["usage"]["prompt_tokens_details"]["cached_tokens"].is_u64());
    assert!(body["timings"]["predicted_per_second"].is_f64());
    assert!(body["id"].as_str().unwrap().starts_with("chatcmpl-"));
}

#[tokio::test]
async fn the_errors_are_those_of_the_real_server() {
    let shared = server(Config::new("m.gguf").ctx(64));
    let text = "word ".repeat(100);
    let expected = tokens::pieces(&text).len() + tokens::PER_MESSAGE + tokens::GENERATION_PROMPT;
    let long = user(&text);
    let (status, body) =
        read(call(&shared, Method::POST, "/v1/chat/completions", None, long).await).await;
    assert_eq!(status, 400);
    assert_eq!(body["error"]["type"], "exceed_context_size_error");
    assert_eq!(body["error"]["n_ctx"], 64);
    assert_eq!(body["error"]["n_prompt_tokens"], expected);
    let message = format!(
        "request ({expected} tokens) exceeds the available context size (64 tokens), try increasing it"
    );
    assert_eq!(body["error"]["message"], message);

    let image = json!({ "messages": [{ "role": "user", "content": [
        { "type": "text", "text": "What?" },
        { "type": "image_url", "image_url": { "url": "data:image/png;base64,AAAA" } } ] }] });
    let (status, body) = read(
        call(
            &shared,
            Method::POST,
            "/v1/chat/completions",
            None,
            image.clone(),
        )
        .await,
    )
    .await;
    assert_eq!(status, 500);
    assert!(
        body["error"]["message"]
            .as_str()
            .unwrap()
            .contains("provide the mmproj")
    );
    let vision = server(Config::new("m.gguf").mmproj("p.gguf"));
    let (status, body) =
        read(call(&vision, Method::POST, "/v1/chat/completions", None, image).await).await;
    assert_eq!(status, 200);
    assert_eq!(
        body["choices"][0]["message"]["content"],
        "I see 1 image(s). You said: What?"
    );
    // WebP (même sous un autre type annoncé) : refusé comme b12110 sans
    // `ffmpeg`.
    let webp = json!({ "messages": [{ "role": "user", "content": [
        { "type": "image_url", "image_url": { "url": "data:image/png;base64,UklGRkwAAABXRUJQ" } } ] }] });
    let (status, body) =
        read(call(&vision, Method::POST, "/v1/chat/completions", None, webp).await).await;
    assert_eq!(status, 500);
    assert_eq!(
        body["error"]["message"],
        "Failed to load image or audio file"
    );

    let (status, body) = read(
        call(
            &shared,
            Method::POST,
            "/v1/chat/completions",
            None,
            json!({ "stream": true }),
        )
        .await,
    )
    .await;
    assert_eq!(
        (status, body["error"]["message"].as_str()),
        (400, Some("'messages' is required"))
    );
    let bad = with(
        user("hi"),
        json!({ "chat_template_kwargs": { "enable_thinking": "false" } }),
    );
    let (status, body) =
        read(call(&shared, Method::POST, "/v1/chat/completions", None, bad).await).await;
    assert_eq!(status, 400);
    assert_eq!(
        body["error"]["message"],
        "invalid type for \"enable_thinking\" (expected boolean, got string)"
    );
    let response = handle(
        State(shared.clone()),
        Method::POST,
        "/v1/chat/completions".parse().unwrap(),
        HeaderMap::new(),
        Bytes::from("{\"messages\":"),
    )
    .await;
    let (status, body) = read(response).await;
    assert_eq!(status, 400);
    assert!(
        body["error"]["message"]
            .as_str()
            .unwrap()
            .starts_with("[json.exception.parse_error.101]")
    );
}

#[tokio::test]
async fn props_models_slots_and_metrics_describe_the_server() {
    let shared = server(
        Config::new("/m/chatonlm-effort.gguf")
            .alias("a/b:Q8_0")
            .ctx(2048)
            .mmproj("/m/p.gguf"),
    );
    let (_, props) = read(call(&shared, Method::GET, "/props", None, Value::Null).await).await;
    assert_eq!(props["default_generation_settings"]["n_ctx"], 2048);
    assert_eq!(props["model_alias"], "a/b:Q8_0");
    assert_eq!(props["model_path"], "/m/chatonlm-effort.gguf");
    assert_eq!(props["modalities"]["vision"], true);
    assert_eq!(
        props["chat_template_caps"]["supports_reasoning_effort"],
        true
    );
    assert_eq!(
        props["chat_template_caps"]["reasoning_effort_levels"],
        json!(["low", "medium", "high"])
    );
    assert_eq!(props["build_info"], BUILD_INFO);
    assert_eq!(props["is_sleeping"], false);
    let (_, models) = read(call(&shared, Method::GET, "/v1/models", None, Value::Null).await).await;
    assert_eq!(models["data"][0]["id"], "a/b:Q8_0");
    assert_eq!(models["data"][0]["meta"]["n_ctx"], 2048);
    assert_eq!(
        models["data"][0]["architecture"]["input_modalities"],
        json!(["text", "image"])
    );
    let (_, slots) = read(call(&shared, Method::GET, "/slots", None, Value::Null).await).await;
    assert_eq!(
        slots,
        json!([{ "id": 0, "n_ctx": 2048, "speculative": false, "is_processing": false }])
    );
    let response = call(&shared, Method::GET, "/metrics", None, Value::Null).await;
    let text = String::from_utf8(
        axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap()
            .to_vec(),
    )
    .unwrap();
    assert!(text.contains("llamacpp:tokens_predicted_total 0"));
    let quiet = server(Config::new("m.gguf").metrics(false));
    let (status, _) = read(call(&quiet, Method::GET, "/metrics", None, Value::Null).await).await;
    assert_eq!(status, 501);
}

#[tokio::test]
async fn tokenize_cuts_like_the_real_server_on_simple_text() {
    let shared = server(Config::new("m.gguf"));
    let (_, body) = read(
        call(
            &shared,
            Method::POST,
            "/tokenize",
            None,
            json!({ "content": "Hello world", "with_pieces": true }),
        )
        .await,
    )
    .await;
    assert_eq!(body["tokens"][0]["piece"], "Hello");
    assert_eq!(body["tokens"][1]["piece"], " world");
    let (_, body) = read(
        call(
            &shared,
            Method::POST,
            "/tokenize",
            None,
            json!({ "content": "Hello world" }),
        )
        .await,
    )
    .await;
    assert_eq!(body["tokens"].as_array().unwrap().len(), 2);
}

#[tokio::test]
async fn infill_completes_only_with_a_fim_model() {
    let fim = server(Config::new("chatonlm-fim.gguf"));
    let request = json!({ "input_prefix": "def add(a, b):\n    ", "input_suffix": "\n",
        "input_extra": [{ "filename": "x.py", "text": "x = 1\n" }], "n_predict": 48 });
    let (status, body) =
        read(call(&fim, Method::POST, "/infill", None, request.clone()).await).await;
    assert_eq!(status, 200);
    assert_eq!(body["content"], FIM_COMPLETION);
    assert_eq!(body["stop_type"], "eos");
    assert!(
        body["prompt"]
            .as_str()
            .unwrap()
            .starts_with("<|file_sep|>x.py\nx = 1\n<|fim_prefix|>")
    );
    let limited = with(request.clone(), json!({ "n_predict": 1 }));
    let (_, body) = read(call(&fim, Method::POST, "/infill", None, limited).await).await;
    assert_eq!(
        (body["content"].as_str(), body["stop_type"].as_str()),
        (Some("return"), Some("limit"))
    );
    let stopped = with(request.clone(), json!({ "stop": [" +"] }));
    let (_, body) = read(call(&fim, Method::POST, "/infill", None, stopped).await).await;
    assert_eq!(
        (body["content"].as_str(), body["stopping_word"].as_str()),
        (Some("return a"), Some(" +"))
    );
    let plain = server(Config::new("qwen3.gguf"));
    let (_, body) = read(call(&plain, Method::POST, "/infill", None, request).await).await;
    assert_eq!(body["content"], FIM_JUNK);
}

#[tokio::test]
async fn the_prompt_cache_counts_the_common_prefix() {
    let shared = server(Config::new("m.gguf"));
    let first = read(
        call(
            &shared,
            Method::POST,
            "/v1/chat/completions",
            None,
            user("one two three"),
        )
        .await,
    )
    .await
    .1;
    assert_eq!(first["timings"]["cache_n"], 0);
    let again = read(
        call(
            &shared,
            Method::POST,
            "/v1/chat/completions",
            None,
            user("one two three"),
        )
        .await,
    )
    .await
    .1;
    assert_eq!(again["timings"]["prompt_n"], 1);
    let off = with(user("one two three"), json!({ "cache_prompt": false }));
    let off = read(call(&shared, Method::POST, "/v1/chat/completions", None, off).await)
        .await
        .1;
    assert_eq!(off["timings"]["cache_n"], 0);
}

#[tokio::test]
async fn a_dropped_stream_counts_as_a_disconnect() {
    let shared = server(Config::new("chatonlm-slow.gguf"));
    let response = call(
        &shared,
        Method::POST,
        "/v1/chat/completions",
        None,
        with(user("Hi"), json!({ "stream": true })),
    )
    .await;
    let mut body = response.into_body().into_data_stream();
    body.next().await.unwrap().unwrap();
    assert_eq!(shared.processing.load(Ordering::SeqCst), 1);
    drop(body);
    assert_eq!(shared.disconnects.load(Ordering::SeqCst), 1);
    assert_eq!(shared.processing.load(Ordering::SeqCst), 0);
    // Flux lu jusqu'au bout : pas d'abandon.
    let fast = server(Config::new("m.gguf"));
    events_of(&fast, with(user("Hi"), json!({ "stream": true }))).await;
    assert_eq!(fast.disconnects.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn requests_wait_for_a_free_slot_without_headers() {
    // `-np 1` : la deuxième réponse n'a pas d'en-têtes tant que la première
    // garde l'emplacement (relevé sur b12110 : en-têtes à la fin de la
    // première).
    let shared = server(Config::new("chatonlm-slow.gguf"));
    let stream = || with(user("Hi"), json!({ "stream": true }));
    let first = call(
        &shared,
        Method::POST,
        "/v1/chat/completions",
        None,
        stream(),
    )
    .await;
    let mut body = first.into_body().into_data_stream();
    body.next().await.unwrap().unwrap();
    let queued = tokio::spawn({
        let shared = shared.clone();
        async move {
            call(
                &shared,
                Method::POST,
                "/v1/chat/completions",
                None,
                stream(),
            )
            .await
        }
    });
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert!(!queued.is_finished(), "en file, sans en-têtes");
    assert_eq!(shared.processing.load(Ordering::SeqCst), 1);
    // Non-flux et /infill attendent aussi.
    let whole = tokio::spawn({
        let shared = shared.clone();
        async move {
            call(
                &shared,
                Method::POST,
                "/v1/chat/completions",
                None,
                user("Hi"),
            )
            .await
        }
    });
    let fill = tokio::spawn({
        let shared = shared.clone();
        async move {
            call(
                &shared,
                Method::POST,
                "/infill",
                None,
                json!({ "input_prefix": "a" }),
            )
            .await
        }
    });
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert!(!whole.is_finished() && !fill.is_finished());
    // Emplacement rendu : la suivante est prise.
    drop(body);
    let queued = tokio::time::timeout(Duration::from_secs(5), queued)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(queued.status(), StatusCode::OK);
    drop(queued);
    for task in [whole, fill] {
        let response = tokio::time::timeout(Duration::from_secs(10), task)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
    }
    // Deux emplacements : servies ensemble.
    let two = server(Config::new("chatonlm-slow.gguf").slots(2));
    let a = call(&two, Method::POST, "/v1/chat/completions", None, stream()).await;
    let b = tokio::time::timeout(
        Duration::from_millis(300),
        call(&two, Method::POST, "/v1/chat/completions", None, stream()),
    )
    .await;
    assert!(b.is_ok());
    assert_eq!(two.processing.load(Ordering::SeqCst), 2);
    drop(a);
}

#[tokio::test]
async fn the_pings_scenario_sends_only_pings_then_nothing() {
    let shared = server(Config::new("chatonlm-pings.gguf"));
    let response = call(
        &shared,
        Method::POST,
        "/v1/chat/completions",
        None,
        with(user("Hi"), json!({ "stream": true })),
    )
    .await;
    let mut body = response.into_body().into_data_stream();
    let first = body.next().await.unwrap().unwrap();
    assert!(String::from_utf8_lossy(&first).contains("\"role\":\"assistant\""));
    let started = Instant::now();
    for _ in 0..scenario::PINGS {
        assert_eq!(&body.next().await.unwrap().unwrap()[..], b":\n\n");
    }
    assert!(started.elapsed() >= scenario::PING_EVERY * (scenario::PINGS - 1));
    // Puis plus rien, connexion gardée.
    assert!(
        tokio::time::timeout(Duration::from_millis(300), body.next())
            .await
            .is_err()
    );
}

#[tokio::test]
async fn a_crash_cuts_the_stream_and_asks_to_abort() {
    let shared = server(Config::new("chatonlm-crash.gguf"));
    let response = call(
        &shared,
        Method::POST,
        "/v1/chat/completions",
        None,
        with(user("Hi"), json!({ "stream": true })),
    )
    .await;
    let mut body = response.into_body().into_data_stream();
    let mut texts = 0;
    let mut failed = false;
    while let Some(item) = body.next().await {
        match item {
            Ok(bytes) => {
                texts += usize::from(String::from_utf8_lossy(&bytes).contains("\"content\":\""))
            }
            Err(_) => failed = true,
        }
    }
    assert_eq!((texts, failed), (3, true));
    assert_eq!(*shared.exit.borrow(), Some(Exit::Abort));
}

#[tokio::test]
async fn a_failed_load_ends_the_server_with_code_1() {
    let server = FakeLlama::start(Config::new("chatonlm-loadfail.gguf"))
        .await
        .unwrap();
    assert_eq!(
        tokio::time::timeout(Duration::from_secs(2), server.exit_requested())
            .await
            .unwrap(),
        Exit::Code(1)
    );
    server.stop().await;
}

#[tokio::test]
async fn sleep_idle_is_reported_by_props() {
    let shared = server(Config::new("m.gguf").sleep_idle(Duration::from_millis(50)));
    tokio::time::sleep(Duration::from_millis(80)).await;
    let (_, props) = read(call(&shared, Method::GET, "/props", None, Value::Null).await).await;
    assert_eq!(props["is_sleeping"], true);
    read(
        call(
            &shared,
            Method::POST,
            "/v1/chat/completions",
            None,
            user("wake"),
        )
        .await,
    )
    .await;
    let (_, props) = read(call(&shared, Method::GET, "/props", None, Value::Null).await).await;
    assert_eq!(props["is_sleeping"], false);
}

#[tokio::test]
async fn status_and_huge_modes() {
    let shared = server(Config::new("m.gguf").mode(Mode::Status(418)));
    let response = call(&shared, Method::GET, "/health", None, Value::Null).await;
    assert_eq!(response.status(), 418);
    let shared = server(Config::new("m.gguf").mode(Mode::Huge));
    let response = call(&shared, Method::GET, "/props", None, Value::Null).await;
    let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    assert!(bytes.len() > HUGE_BODY);
}

#[test]
fn modes_are_parsed() {
    assert_eq!(Mode::parse("ok"), Some(Mode::Ok));
    assert_eq!(Mode::parse("status:500"), Some(Mode::Status(500)));
    assert_eq!(
        Mode::parse("loading:250"),
        Some(Mode::Loading(Duration::from_millis(250)))
    );
    assert_eq!(Mode::parse("silent"), Some(Mode::Silent));
    assert_eq!(Mode::parse("bogus"), None);
    assert_eq!(Mode::parse("status:x"), None);
}
