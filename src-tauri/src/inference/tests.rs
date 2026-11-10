//! Le client contre le faux `llama-server` (crate `fake-llama`), lancé dans
//! le processus de l'essai.

use std::time::Duration;

use fake_llama::{Config, FakeLlama, Mode};
use futures_util::StreamExt;
use serde_json::{Value, json};
use tokio_util::sync::CancellationToken;

use super::*;

const KEY: &str = "test-key";

async fn start(config: Config) -> (FakeLlama, Endpoint) {
    let fake = FakeLlama::start(config.api_key(KEY)).await.unwrap();
    let endpoint = Endpoint::new(fake.url(), KEY).unwrap();
    (fake, endpoint)
}

fn ask(text: &str) -> ChatRequest {
    ChatRequest::new(vec![Message::user(text)])
}

/// Tous les événements du flux, jusqu'à sa fin.
async fn collect(endpoint: &Endpoint, request: &ChatRequest) -> Vec<Result<Event, InferenceError>> {
    let stream = chat_stream(endpoint, request, CancellationToken::new())
        .await
        .unwrap();
    stream.collect().await
}

fn text_of(events: &[Result<Event, InferenceError>], reasoning: bool) -> String {
    events
        .iter()
        .filter_map(|event| match event {
            Ok(Event::Content(text)) if !reasoning => Some(text.as_str()),
            Ok(Event::Reasoning(text)) if reasoning => Some(text.as_str()),
            _ => None,
        })
        .collect()
}

fn weather_tool() -> Value {
    json!({ "type": "function", "function": { "name": "get_weather", "description": "Weather of a city",
        "parameters": { "type": "object", "properties": { "city": { "type": "string" } }, "required": ["city"] } } })
}

/// Attend qu'une condition sur le faux devienne vraie (1 s au plus).
async fn eventually(mut condition: impl FnMut() -> bool) -> bool {
    for _ in 0..100 {
        if condition() {
            return true;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    false
}

#[tokio::test]
async fn a_streamed_reply_ends_with_its_usage_and_timings() {
    let (fake, endpoint) = start(Config::new("/models/qwen.gguf")).await;
    let events = collect(&endpoint, &ask("Hello world")).await;
    assert_eq!(text_of(&events, false), "Hello! You said: Hello world");
    let Some(Ok(Event::Done {
        finish_reason,
        usage,
        timings,
    })) = events.last()
    else {
        panic!("{events:?}");
    };
    assert_eq!(*finish_reason, FinishReason::Stop);
    let usage = usage.unwrap();
    assert_eq!(usage.completion, 5);
    assert!(usage.prompt > 0);
    let timings = timings.unwrap();
    assert_eq!(timings.predicted_n, 5);
    assert_eq!(timings.cache_n + timings.prompt_n, usage.prompt);
    // Ce qui est parti : flux, usage demandé, clé reçue (sinon 401).
    let sent = &fake.chat_requests()[0];
    assert_eq!(sent["stream"], true);
    assert_eq!(sent["stream_options"]["include_usage"], true);
    assert_eq!(
        sent["messages"],
        json!([{ "role": "user", "content": "Hello world" }])
    );
    fake.stop().await;
}

#[tokio::test]
async fn reasoning_comes_apart_and_think_off_reaches_the_server() {
    let (fake, endpoint) = start(Config::new("chatonlm-think.gguf")).await;
    let events = collect(&endpoint, &ask("2+2?")).await;
    assert!(text_of(&events, true).starts_with("Let me think"));
    assert_eq!(text_of(&events, false), "Hello! You said: 2+2?");
    // La réflexion vient avant la réponse.
    let first_content = events
        .iter()
        .position(|e| matches!(e, Ok(Event::Content(_))))
        .unwrap();
    assert!(
        events[..first_content]
            .iter()
            .any(|e| matches!(e, Ok(Event::Reasoning(_))))
    );
    let off = ChatRequest {
        think: Think::Off,
        ..ask("2+2?")
    };
    let events = collect(&endpoint, &off).await;
    assert!(text_of(&events, true).is_empty());
    assert_eq!(
        fake.chat_requests()[1]["chat_template_kwargs"],
        json!({ "enable_thinking": false })
    );
    fake.stop().await;

    let (fake, endpoint) = start(Config::new("chatonlm-effort.gguf")).await;
    let low = ChatRequest {
        think: Think::Effort(Effort::Low),
        ..ask("hi")
    };
    let high = ChatRequest {
        think: Think::Effort(Effort::High),
        ..ask("hi")
    };
    let low = text_of(&collect(&endpoint, &low).await, true);
    let high = text_of(&collect(&endpoint, &high).await, true);
    assert!(high.len() > low.len());
    assert_eq!(fake.chat_requests()[0]["reasoning_effort"], "low");
    let caps = props(&endpoint).await.unwrap().caps;
    assert!(caps.supports_reasoning_effort);
    assert_eq!(caps.reasoning_effort_levels, ["low", "medium", "high"]);
    fake.stop().await;
}

#[tokio::test]
async fn tool_fragments_become_one_call_then_the_tool_turn_is_answered() {
    let (fake, endpoint) = start(Config::new("chatonlm-tools.gguf")).await;
    let request = ChatRequest {
        tools: vec![weather_tool()],
        ..ask("Paris")
    };
    let events = collect(&endpoint, &request).await;
    let calls: Vec<&Vec<ToolCall>> = events
        .iter()
        .filter_map(|event| match event {
            Ok(Event::ToolCalls(calls)) => Some(calls),
            _ => None,
        })
        .collect();
    assert_eq!(calls.len(), 1, "un seul événement d'appels : {events:?}");
    let call = &calls[0][0];
    assert_eq!(call.name, "get_weather");
    assert_eq!(call.id, "fake-call-0");
    assert_eq!(call.arguments_json(), Some(json!({ "city": "Paris" })));
    assert!(matches!(
        events.last(),
        Some(Ok(Event::Done {
            finish_reason: FinishReason::ToolCalls,
            ..
        }))
    ));
    assert_eq!(fake.chat_requests()[0]["tools"], json!([weather_tool()]));

    // Tour suivant : l'appel dans l'historique, puis le résultat.
    let mut assistant = Message::assistant("");
    assistant.tool_calls = vec![call.clone()];
    let next = ChatRequest {
        messages: vec![
            Message::user("Paris"),
            assistant,
            Message::tool(&call.id, &call.name, "14C, cloudy"),
        ],
        tools: vec![weather_tool()],
        ..ChatRequest::default()
    };
    let events = collect(&endpoint, &next).await;
    assert_eq!(text_of(&events, false), "Tool result received: 14C, cloudy");
    let sent = &fake.chat_requests()[1]["messages"];
    assert_eq!(
        sent[1]["tool_calls"][0]["function"]["arguments"],
        "{\"city\":\"Paris\"}"
    );
    assert_eq!(sent[2]["tool_call_id"], "fake-call-0");
    fake.stop().await;
}

#[tokio::test]
async fn parallel_calls_keep_their_order_and_a_missing_id_is_given() {
    let (fake, endpoint) = start(Config::new("chatonlm-parallel.gguf")).await;
    let request = ChatRequest {
        tools: vec![weather_tool()],
        ..ask("Paris")
    };
    let events = collect(&endpoint, &request).await;
    let Some(Ok(Event::ToolCalls(calls))) =
        events.iter().find(|e| matches!(e, Ok(Event::ToolCalls(_))))
    else {
        panic!("{events:?}");
    };
    assert_eq!(calls.len(), 2);
    assert_eq!(calls[0].arguments_json(), Some(json!({ "city": "Paris" })));
    assert_eq!(
        calls[1].arguments_json(),
        Some(json!({ "city": "Paris (1)" }))
    );
    fake.stop().await;

    let (fake, endpoint) = start(Config::new("chatonlm-noid.gguf")).await;
    let events = collect(&endpoint, &request).await;
    let Some(Ok(Event::ToolCalls(calls))) =
        events.iter().find(|e| matches!(e, Ok(Event::ToolCalls(_))))
    else {
        panic!("{events:?}");
    };
    assert!(calls[0].id.starts_with("call_") && calls[0].id.len() > 10);
    let once = chat_once(&endpoint, &request, CancellationToken::new())
        .await
        .unwrap();
    assert!(!once.tool_calls[0].id.is_empty());
    fake.stop().await;
}

#[tokio::test]
async fn the_whole_reply_at_once() {
    let (fake, endpoint) = start(Config::new("chatonlm-think.gguf")).await;
    let reply = chat_once(&endpoint, &ask("Hi"), CancellationToken::new())
        .await
        .unwrap();
    assert_eq!(reply.content, "Hello! You said: Hi");
    assert!(reply.reasoning.starts_with("Let me think"));
    assert_eq!(reply.finish_reason, FinishReason::Stop);
    assert!(reply.usage.is_some() && reply.timings.is_some());
    assert_eq!(fake.chat_requests()[0]["stream"], false);
    assert!(fake.chat_requests()[0].get("stream_options").is_none());
    // Coupée par max_tokens.
    let short = ChatRequest {
        max_tokens: Some(2),
        think: Think::Off,
        ..ask("Hi")
    };
    let reply = chat_once(&endpoint, &short, CancellationToken::new())
        .await
        .unwrap();
    assert_eq!(reply.finish_reason, FinishReason::Length);
    assert_eq!(reply.content, "Hello! You");
    fake.stop().await;
}

#[tokio::test]
async fn a_json_schema_constrains_the_reply() {
    let (fake, endpoint) = start(Config::new("m.gguf")).await;
    let schema = json!({ "type": "object", "properties": { "name": { "type": "string" }, "age": { "type": "integer" } },
        "required": ["name", "age"] });
    let request = ChatRequest {
        response_format: Some(ResponseFormat::JsonSchema(schema.clone())),
        ..ask("Invent a person.")
    };
    let reply = chat_once(&endpoint, &request, CancellationToken::new())
        .await
        .unwrap();
    let person: Value = serde_json::from_str(&reply.content).unwrap();
    assert!(person["name"].is_string() && person["age"].is_i64());
    assert_eq!(
        fake.chat_requests()[0]["response_format"]["json_schema"]["schema"],
        schema
    );
    fake.stop().await;
}

#[tokio::test]
async fn refusals_of_the_server_get_their_variant() {
    // Contexte dépassé.
    let (fake, endpoint) = start(Config::new("m.gguf").ctx(32)).await;
    let long = ask(&"word ".repeat(64));
    let error = chat_stream(&endpoint, &long, CancellationToken::new())
        .await
        .err()
        .unwrap();
    let InferenceError::ContextExceeded {
        n_prompt_tokens: Some(prompt),
        n_ctx: Some(32),
    } = error
    else {
        panic!("{error:?}");
    };
    assert!(prompt > 32);
    assert!(matches!(
        chat_once(&endpoint, &long, CancellationToken::new()).await,
        Err(InferenceError::ContextExceeded { .. })
    ));
    // Image sans projecteur, puis avec.
    let image = ChatRequest::new(vec![Message::user_parts(vec![
        Part::Text("What?".into()),
        Part::image("image/png", b"\x89PNG\r\n\x1a\n"),
    ])]);
    assert_eq!(
        chat_stream(&endpoint, &image, CancellationToken::new())
            .await
            .err(),
        Some(InferenceError::ImageUnsupported)
    );
    fake.stop().await;
    let (fake, endpoint) = start(Config::new("m.gguf").mmproj("p.gguf")).await;
    let events = collect(&endpoint, &image).await;
    assert_eq!(text_of(&events, false), "I see 1 image(s). You said: What?");
    assert!(props(&endpoint).await.unwrap().modalities.vision);
    fake.stop().await;
    // Clé refusée (la santé reste publique).
    let (fake, _) = start(Config::new("m.gguf")).await;
    let wrong = Endpoint::new(fake.url(), "wrong").unwrap();
    assert_eq!(
        chat_stream(&wrong, &ask("hi"), CancellationToken::new())
            .await
            .err(),
        Some(InferenceError::Unauthorized)
    );
    assert_eq!(props(&wrong).await, Err(InferenceError::Unauthorized));
    assert_eq!(health(&wrong).await, Health::Ready);
    fake.stop().await;
    // Code quelconque, corps vide.
    let (fake, endpoint) = start(Config::new("m.gguf").mode(Mode::Status(418))).await;
    assert_eq!(
        chat_once(&endpoint, &ask("hi"), CancellationToken::new()).await,
        Err(InferenceError::Http {
            status: 418,
            message: String::new()
        })
    );
    assert!(matches!(
        health(&endpoint).await,
        Health::Unexpected(InferenceError::Http { status: 418, .. })
    ));
    fake.stop().await;
}

#[tokio::test]
async fn health_follows_the_loading_and_the_requests_say_loading() {
    let (fake, endpoint) =
        start(Config::new("m.gguf").mode(Mode::Loading(Duration::from_millis(300)))).await;
    assert_eq!(health(&endpoint).await, Health::Loading);
    assert_eq!(
        chat_stream(&endpoint, &ask("hi"), CancellationToken::new())
            .await
            .err(),
        Some(InferenceError::Loading)
    );
    // L'état avant la clé, comme le vrai : même une mauvaise clé lit
    // « chargement ».
    let wrong = Endpoint::new(fake.url(), "wrong").unwrap();
    assert_eq!(
        chat_stream(&wrong, &ask("hi"), CancellationToken::new())
            .await
            .err(),
        Some(InferenceError::Loading)
    );
    tokio::time::sleep(Duration::from_millis(350)).await;
    assert_eq!(health(&endpoint).await, Health::Ready);
    fake.stop().await;
    // Rien n'écoute plus : injoignable, et refus de connexion pour le chat.
    let closed = Endpoint::new(format!("http://{}", fake_addr_closed().await), KEY).unwrap();
    assert_eq!(health(&closed).await, Health::Unreachable);
    assert!(matches!(
        chat_stream(&closed, &ask("hi"), CancellationToken::new()).await,
        Err(InferenceError::Connect(_))
    ));
}

/// Adresse d'un port qui vient d'être libéré.
async fn fake_addr_closed() -> std::net::SocketAddr {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    listener.local_addr().unwrap()
}

#[tokio::test]
async fn cancelling_closes_the_connection_and_frees_the_server() {
    let (fake, endpoint) = start(Config::new("chatonlm-slow.gguf")).await;
    let cancel = CancellationToken::new();
    let mut stream = chat_stream(&endpoint, &ask("one two three four"), cancel.clone())
        .await
        .unwrap();
    assert!(matches!(stream.next().await, Some(Ok(Event::Content(_)))));
    assert_eq!(fake.processing(), 1);
    cancel.cancel();
    assert_eq!(stream.next().await, Some(Err(InferenceError::Cancelled)));
    // Connexion fermée dès l'annulation, sans attendre la fin du flux.
    assert!(eventually(|| fake.disconnects() == 1 && fake.processing() == 0).await);
    assert_eq!(stream.next().await, None);
    fake.stop().await;

    // Avant les en-têtes : un serveur muet.
    let (fake, endpoint) = start(Config::new("m.gguf").mode(Mode::Silent)).await;
    let cancel = CancellationToken::new();
    let later = cancel.clone();
    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(50)).await;
        later.cancel();
    });
    let started = std::time::Instant::now();
    assert_eq!(
        chat_stream(&endpoint, &ask("hi"), cancel).await.err(),
        Some(InferenceError::Cancelled)
    );
    assert!(started.elapsed() < Duration::from_secs(2));
    // Réponse entière et autocomplétion aussi.
    let cancelled = CancellationToken::new();
    cancelled.cancel();
    assert_eq!(
        chat_once(&endpoint, &ask("hi"), cancelled.clone()).await,
        Err(InferenceError::Cancelled)
    );
    assert_eq!(
        infill(&endpoint, &InfillRequest::default(), cancelled).await,
        Err(InferenceError::Cancelled)
    );
    fake.stop().await;
}

#[tokio::test]
async fn cancelling_while_a_whole_reply_is_read() {
    // En-têtes envoyés, corps commencé puis silence : faux serveur à la main.
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    tokio::spawn(async move {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        while let Ok((mut socket, _)) = listener.accept().await {
            tokio::spawn(async move {
                let mut buffer = [0u8; 4096];
                let _ = socket.read(&mut buffer).await;
                let head = "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: 100\r\n\r\n{\"choices\":";
                let _ = socket.write_all(head.as_bytes()).await;
                std::future::pending::<()>().await;
            });
        }
    });
    let endpoint = Endpoint::new(format!("http://{address}"), KEY).unwrap();
    for whole in [true, false] {
        let cancel = CancellationToken::new();
        let later = cancel.clone();
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(100)).await;
            later.cancel();
        });
        let result = if whole {
            tokio::time::timeout(
                Duration::from_secs(5),
                chat_once(&endpoint, &ask("hi"), cancel),
            )
            .await
            .map(|r| r.map(drop))
        } else {
            tokio::time::timeout(
                Duration::from_secs(5),
                infill(&endpoint, &InfillRequest::default(), cancel),
            )
            .await
            .map(|r| r.map(drop))
        };
        assert_eq!(result, Ok(Err(InferenceError::Cancelled)));
    }
}

#[tokio::test]
async fn a_silent_server_or_stream_times_out() {
    // Avant les en-têtes : pas de délai par défaut (le délai de silence n'y
    // vaut pas) ; seulement l'annulation, ou un délai d'en-têtes explicite.
    let (fake, endpoint) = start(Config::new("m.gguf").mode(Mode::Silent)).await;
    let endpoint = endpoint.with_idle_timeout(Duration::from_millis(200));
    let waiting = tokio::time::timeout(
        Duration::from_millis(600),
        chat_stream(&endpoint, &ask("hi"), CancellationToken::new()),
    )
    .await;
    assert!(waiting.is_err(), "toujours en attente des en-têtes");
    let limited = endpoint
        .clone()
        .with_header_timeout(Duration::from_millis(200));
    let started = std::time::Instant::now();
    assert_eq!(
        chat_stream(&limited, &ask("hi"), CancellationToken::new())
            .await
            .err(),
        Some(InferenceError::Timeout)
    );
    assert!(started.elapsed() < Duration::from_secs(2));
    assert_eq!(health(&endpoint).await, Health::Unreachable);
    fake.stop().await;
    // Flux commencé, puis silence.
    let (fake, endpoint) = start(Config::new("chatonlm-hang.gguf")).await;
    let endpoint = endpoint.with_idle_timeout(Duration::from_millis(200));
    let events = collect(&endpoint, &ask("hi")).await;
    assert_eq!(events, [Err(InferenceError::Timeout)]);
    fake.stop().await;
    // Connexions fermées aussitôt.
    let (fake, endpoint) = start(Config::new("m.gguf").mode(Mode::Close)).await;
    assert!(matches!(
        chat_stream(&endpoint, &ask("hi"), CancellationToken::new()).await,
        Err(InferenceError::Connect(_))
    ));
    assert_eq!(health(&endpoint).await, Health::Unreachable);
    fake.stop().await;
}

#[tokio::test]
async fn a_queued_request_waits_longer_than_the_idle_timeout() {
    // `-np 1` : la deuxième requête attend la fin de la première, sans
    // en-têtes (comme b12110), plus longtemps que le délai de silence. Marge
    // sous la charge : 400 ms de silence permis pour des morceaux espacés de
    // 150 ms ; la première réponse (une douzaine de morceaux, ~1,8 s) dure
    // bien plus que deux délais.
    let (fake, endpoint) = start(Config::new("chatonlm-slow.gguf")).await;
    let idle = Duration::from_millis(400);
    let endpoint = endpoint.with_idle_timeout(idle);
    let first = chat_stream(
        &endpoint,
        &ask("one two three four five six seven"),
        CancellationToken::new(),
    )
    .await
    .unwrap();
    let started = std::time::Instant::now();
    let second = tokio::spawn({
        let endpoint = endpoint.clone();
        async move {
            let stream = chat_stream(&endpoint, &ask("hi"), CancellationToken::new()).await?;
            let headers = started.elapsed();
            let events: Vec<_> = stream.collect().await;
            Ok::<_, InferenceError>((headers, events))
        }
    });
    let events: Vec<_> = first.collect().await;
    assert!(
        matches!(events.last(), Some(Ok(Event::Done { .. }))),
        "{events:?}"
    );
    let (headers, events) = second.await.unwrap().unwrap();
    assert!(headers > idle * 2, "en file {headers:?}");
    assert_eq!(text_of(&events, false), "Hello! You said: hi");
    assert!(
        matches!(events.last(), Some(Ok(Event::Done { .. }))),
        "{events:?}"
    );
    fake.stop().await;
}

#[tokio::test]
async fn the_idle_timeout_counts_from_each_block_pings_included() {
    let idle = Duration::from_millis(200);
    // 150 ms entre deux morceaux (< délai) : la réponse entière dure bien
    // plus que le délai et finit.
    let (fake, endpoint) = start(Config::new("chatonlm-slow.gguf")).await;
    let endpoint = endpoint.with_idle_timeout(idle);
    let started = std::time::Instant::now();
    let events = collect(&endpoint, &ask("one two three four")).await;
    assert!(started.elapsed() > idle * 3);
    assert_eq!(
        text_of(&events, false),
        "Hello! You said: one two three four"
    );
    assert!(
        matches!(events.last(), Some(Ok(Event::Done { .. }))),
        "{events:?}"
    );
    fake.stop().await;
    // Pings seuls (toutes les 100 ms pendant 1 s), puis silence : les pings
    // gardent le flux en vie, le silence qui suit finit en `Timeout`.
    let (fake, endpoint) = start(Config::new("chatonlm-pings.gguf")).await;
    let endpoint = endpoint.with_idle_timeout(idle);
    let started = std::time::Instant::now();
    let events = collect(&endpoint, &ask("hi")).await;
    let elapsed = started.elapsed();
    assert_eq!(events, [Err(InferenceError::Timeout)]);
    let pinged = fake_llama::scenario::PING_EVERY * fake_llama::scenario::PINGS;
    assert!(elapsed >= pinged + idle, "{elapsed:?}");
    assert!(elapsed < pinged + idle * 4, "{elapsed:?}");
    fake.stop().await;
}

#[tokio::test]
async fn a_crash_or_an_error_in_the_middle_ends_the_stream() {
    let (fake, endpoint) = start(Config::new("chatonlm-crash.gguf")).await;
    let events = collect(&endpoint, &ask("hi")).await;
    assert_eq!(text_of(&events, false), "One two three");
    assert!(
        matches!(events.last(), Some(Err(InferenceError::Interrupted(_)))),
        "{events:?}"
    );
    assert_eq!(fake.exit(), Some(fake_llama::Exit::Abort));
    fake.stop().await;

    let (fake, endpoint) = start(Config::new("chatonlm-error.gguf")).await;
    let events = collect(&endpoint, &ask("hi")).await;
    assert_eq!(text_of(&events, false), "One two three");
    assert_eq!(
        events.last(),
        Some(&Err(InferenceError::Http {
            status: 500,
            message: "fake failure in the middle of the reply".into()
        }))
    );
    fake.stop().await;
}

#[tokio::test]
async fn huge_replies_are_refused_without_reading_them_all() {
    let (fake, endpoint) = start(Config::new("m.gguf").mode(Mode::Huge)).await;
    let events = collect(&endpoint, &ask("hi")).await;
    assert!(
        matches!(events.as_slice(), [Err(InferenceError::Malformed(_))]),
        "{events:?}"
    );
    assert!(matches!(
        chat_once(&endpoint, &ask("hi"), CancellationToken::new()).await,
        Err(InferenceError::Malformed(_))
    ));
    assert!(matches!(
        props(&endpoint).await,
        Err(InferenceError::Malformed(_))
    ));
    assert!(matches!(
        tokenize(&endpoint, "hi").await,
        Err(InferenceError::Malformed(_))
    ));
    fake.stop().await;
}

#[tokio::test]
async fn infill_with_and_without_fim_tokens() {
    let (fake, endpoint) = start(Config::new("chatonlm-fim.gguf")).await;
    let request = InfillRequest {
        input_prefix: "def add(a, b):\n    ".into(),
        input_suffix: "\n".into(),
        input_extra: vec![InfillChunk {
            filename: "util.py".into(),
            text: "x = 1\n".into(),
        }],
        n_predict: Some(48),
        temperature: Some(0.0),
        stop: vec!["\n\n".into()],
    };
    let reply = infill(&endpoint, &request, CancellationToken::new())
        .await
        .unwrap();
    assert_eq!(reply.content, fake_llama::FIM_COMPLETION);
    assert_eq!(reply.stop, InfillStop::Eos);
    assert!(reply.tokens_evaluated > 0 && reply.timings.is_some());
    let sent = &fake.requests()[0];
    assert_eq!(sent["path"], "/infill");
    assert_eq!(
        sent["body"]["input_extra"],
        json!([{ "filename": "util.py", "text": "x = 1\n" }])
    );
    assert_eq!(sent["body"]["n_predict"], 48);
    let limited = InfillRequest {
        n_predict: Some(1),
        ..request.clone()
    };
    let reply = infill(&endpoint, &limited, CancellationToken::new())
        .await
        .unwrap();
    assert_eq!(
        (reply.content.as_str(), reply.stop),
        ("return", InfillStop::Limit)
    );
    fake.stop().await;
    let (fake, endpoint) = start(Config::new("qwen3.gguf")).await;
    let reply = infill(&endpoint, &request, CancellationToken::new())
        .await
        .unwrap();
    // Texte sans valeur, coupé à la première chaîne d'arrêt (`\n\n`).
    assert_eq!(reply.content, "\n</think>");
    assert_eq!(reply.stop, InfillStop::Word);
    assert!(fake_llama::FIM_JUNK.starts_with(&reply.content));
    fake.stop().await;
}

#[tokio::test]
async fn props_and_tokenize_describe_the_loaded_model() {
    let (fake, endpoint) = start(
        Config::new("/m/chatonlm-tools.gguf")
            .alias("unsloth/X-GGUF:Q4_K_M")
            .ctx(2048),
    )
    .await;
    let props = props(&endpoint).await.unwrap();
    assert_eq!(props.n_ctx, 2048);
    assert_eq!(props.model_alias, "unsloth/X-GGUF:Q4_K_M");
    assert_eq!(props.model_path, "/m/chatonlm-tools.gguf");
    assert_eq!(props.build_info, fake_llama::BUILD_INFO);
    assert!(props.caps.supports_tools && !props.caps.supports_reasoning_effort);
    assert!(!props.modalities.vision && !props.is_sleeping);
    assert!(props.chat_template.contains("tool_call"));
    let tokens = tokenize(&endpoint, "Hello world").await.unwrap();
    assert_eq!(tokens, fake_llama::tokens::ids("Hello world"));
    fake.stop().await;
}

/// Serveur à la main qui répond une fois ce corps SSE, puis ferme.
async fn sse_once(body: &'static str) -> Endpoint {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    tokio::spawn(async move {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let (mut socket, _) = listener.accept().await.unwrap();
        let mut buffer = [0u8; 4096];
        let _ = socket.read(&mut buffer).await;
        let response = format!(
            "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
            body.len()
        );
        socket.write_all(response.as_bytes()).await.unwrap();
    });
    Endpoint::new(format!("http://{address}"), KEY).unwrap()
}

#[tokio::test]
async fn a_stream_that_ends_too_early_is_interrupted() {
    const HI: &str = "data: {\"choices\":[{\"index\":0,\"delta\":{\"content\":\"Hi\"}}]}\n\n";
    // Corps coupé avant la raison de fin.
    let events = collect(&sse_once(HI).await, &ask("hi")).await;
    assert_eq!(events[0], Ok(Event::Content("Hi".into())));
    assert!(
        matches!(events[1], Err(InferenceError::Interrupted(_))),
        "{events:?}"
    );
    assert_eq!(events.len(), 2);
    // `[DONE]` sans raison de fin : b12110 envoie toujours la raison avant,
    // la réponse est incomplète.
    const DONE_ALONE: &str = concat!(
        "data: {\"choices\":[{\"index\":0,\"delta\":{\"content\":\"Hi\"}}]}\n\n",
        "data: [DONE]\n\n"
    );
    let events = collect(&sse_once(DONE_ALONE).await, &ask("hi")).await;
    assert!(
        matches!(
            events.as_slice(),
            [Ok(Event::Content(_)), Err(InferenceError::Interrupted(_))]
        ),
        "{events:?}"
    );
    // Raison de fin reçue, puis fin du corps sans `[DONE]` : réponse entière.
    const NO_DONE: &str = concat!(
        "data: {\"choices\":[{\"index\":0,\"delta\":{\"content\":\"Hi\"}}]}\n\n",
        "data: {\"choices\":[{\"index\":0,\"finish_reason\":\"stop\",\"delta\":{}}]}\n\n"
    );
    let events = collect(&sse_once(NO_DONE).await, &ask("hi")).await;
    assert_eq!(
        events,
        [
            Ok(Event::Content("Hi".into())),
            Ok(Event::Done {
                finish_reason: FinishReason::Stop,
                usage: None,
                timings: None
            })
        ]
    );
}

#[tokio::test]
async fn idle_connections_are_dropped_before_the_server_closes_them() {
    // cpp-httplib ferme une connexion inactive au bout de 5 s : le client ne
    // la reprend plus après 4 s (sinon une requête partie au moment de la
    // fermeture finirait en erreur de connexion). Serveur à la main qui
    // garde les connexions et compte celles qu'il reçoit.
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let accepted = Arc::new(AtomicUsize::new(0));
    let counter = accepted.clone();
    tokio::spawn(async move {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        while let Ok((mut socket, _)) = listener.accept().await {
            counter.fetch_add(1, Ordering::SeqCst);
            tokio::spawn(async move {
                let mut buffer = [0u8; 4096];
                // Une requête GET sans corps par lecture (keep-alive).
                while matches!(socket.read(&mut buffer).await, Ok(n) if n > 0) {
                    let body = "{\"status\":\"ok\"}";
                    let response = format!(
                        "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{body}",
                        body.len()
                    );
                    if socket.write_all(response.as_bytes()).await.is_err() {
                        break;
                    }
                }
            });
        }
    });
    let endpoint = Endpoint::new(format!("http://{address}"), KEY).unwrap();
    assert_eq!(health(&endpoint).await, Health::Ready);
    // Tout de suite : même connexion.
    assert_eq!(health(&endpoint).await, Health::Ready);
    assert_eq!(accepted.load(Ordering::SeqCst), 1);
    // Après 4,5 s d'inactivité : nouvelle connexion.
    tokio::time::sleep(Duration::from_millis(4500)).await;
    assert_eq!(health(&endpoint).await, Health::Ready);
    assert_eq!(accepted.load(Ordering::SeqCst), 2);
}

/// Vérification à la main contre le vrai `llama-server` (jamais lancée par
/// `cargo test` ni par check.sh) : adresse et clé dans
/// `CHATONLM_LLAMA_REAL_URL` et `CHATONLM_LLAMA_REAL_KEY`, modèle décrit par
/// `CHATONLM_LLAMA_REAL_KIND` (`text` : Qwen3 avec outils et réflexion,
/// `-c 1024` ; `vision` : avec `--mmproj` ; `fim` : modèle à jetons FIM).
#[tokio::test]
#[ignore = "vrai llama-server lancé à la main"]
async fn real_llama_server_by_hand() {
    let (Ok(url), Ok(key)) = (
        std::env::var("CHATONLM_LLAMA_REAL_URL"),
        std::env::var("CHATONLM_LLAMA_REAL_KEY"),
    ) else {
        return;
    };
    let kind = std::env::var("CHATONLM_LLAMA_REAL_KIND").unwrap_or_else(|_| "text".into());
    let endpoint = Endpoint::new(&url, &key).unwrap();
    assert_eq!(health(&endpoint).await, Health::Ready);
    let props = props(&endpoint).await.unwrap();
    assert!(props.build_info.starts_with("b12110"), "{props:?}");
    eprintln!("props : {props:?}");
    let wrong = Endpoint::new(&url, "wrong").unwrap();
    assert_eq!(props_of(&wrong).await, Err(InferenceError::Unauthorized));
    match kind.as_str() {
        "vision" => {
            assert!(props.modalities.vision);
            let png = std::fs::read(std::env::var("CHATONLM_LLAMA_REAL_IMAGE").unwrap()).unwrap();
            let request = ChatRequest {
                max_tokens: Some(40),
                temperature: Some(0.0),
                ..ChatRequest::new(vec![Message::user_parts(vec![
                    Part::Text("Describe this image in one sentence.".into()),
                    Part::image("image/png", &png),
                ])])
            };
            let events = collect(&endpoint, &request).await;
            eprintln!("vision : {}", text_of(&events, false));
            assert!(!text_of(&events, false).is_empty());
            assert!(matches!(events.last(), Some(Ok(Event::Done { .. }))));
        }
        "fim" => {
            let request = InfillRequest {
                input_prefix: "def fibonacci(n):\n    ".into(),
                input_suffix: "\n".into(),
                input_extra: vec![InfillChunk {
                    filename: "util.py".into(),
                    text: "import math\n".into(),
                }],
                n_predict: Some(48),
                temperature: Some(0.0),
                stop: vec!["\n\n\n".into()],
            };
            let reply = infill(&endpoint, &request, CancellationToken::new())
                .await
                .unwrap();
            eprintln!("fim : {reply:?}");
            assert!(reply.content.contains("return") || reply.content.contains("if"));
            assert!(reply.timings.is_some() && reply.tokens_evaluated > 0);
        }
        _ => {
            assert!(props.caps.supports_tools && !props.modalities.vision);
            // Réflexion par défaut, puis coupée.
            let request = ChatRequest {
                max_tokens: Some(60),
                temperature: Some(0.0),
                ..ask("What is 17 times 23?")
            };
            let events = collect(&endpoint, &request).await;
            assert!(!text_of(&events, true).is_empty(), "{events:?}");
            let Some(Ok(Event::Done {
                finish_reason,
                usage: Some(usage),
                timings: Some(timings),
            })) = events.last()
            else {
                panic!("{events:?}");
            };
            eprintln!("think : fin {finish_reason:?}, {usage:?}, {timings:?}");
            assert_eq!(usage.completion, timings.predicted_n);
            let off = ChatRequest {
                think: Think::Off,
                max_tokens: Some(40),
                ..request.clone()
            };
            let events = collect(&endpoint, &off).await;
            assert!(text_of(&events, true).is_empty());
            eprintln!("sans réflexion : {}", text_of(&events, false));
            assert!(text_of(&events, false).contains("391"));
            // Effort : accepté (sans effet sur Qwen3).
            let effort = ChatRequest {
                think: Think::Effort(Effort::Low),
                max_tokens: Some(5),
                ..request.clone()
            };
            assert!(
                chat_once(&endpoint, &effort, CancellationToken::new())
                    .await
                    .is_ok()
            );
            // Outils, en flux puis entier, puis le tour de l'outil.
            let tools = ChatRequest {
                tools: vec![weather_tool()],
                think: Think::Off,
                temperature: Some(0.0),
                ..ask("What is the weather in Paris? Use the tool.")
            };
            let events = collect(&endpoint, &tools).await;
            let Some(Ok(Event::ToolCalls(calls))) =
                events.iter().find(|e| matches!(e, Ok(Event::ToolCalls(_))))
            else {
                panic!("{events:?}");
            };
            assert_eq!(calls.len(), 1);
            assert_eq!(calls[0].name, "get_weather");
            assert_eq!(calls[0].arguments_json(), Some(json!({ "city": "Paris" })));
            assert!(!calls[0].id.is_empty());
            assert!(matches!(
                events.last(),
                Some(Ok(Event::Done {
                    finish_reason: FinishReason::ToolCalls,
                    ..
                }))
            ));
            let once = chat_once(&endpoint, &tools, CancellationToken::new())
                .await
                .unwrap();
            assert_eq!(once.tool_calls[0].name, "get_weather");
            let mut assistant = Message::assistant("");
            assistant.tool_calls = calls.clone();
            let mut turn = tools.clone();
            turn.messages.push(assistant);
            turn.messages
                .push(Message::tool(&calls[0].id, "get_weather", "14°C, cloudy"));
            let answer = chat_once(&endpoint, &turn, CancellationToken::new())
                .await
                .unwrap();
            eprintln!("après l'outil : {}", answer.content);
            assert!(answer.content.contains("14"));
            // Schéma JSON. serde_json (sans `preserve_order`) trie les clés :
            // propriétés choisies dans l'ordre alphabétique, que la grammaire
            // du serveur impose.
            let schema = json!({ "type": "object", "properties": { "name": { "type": "string" }, "years": { "type": "integer" } },
                "required": ["name", "years"] });
            let structured = ChatRequest {
                response_format: Some(ResponseFormat::JsonSchema(schema)),
                think: Think::Off,
                temperature: Some(0.0),
                max_tokens: Some(200),
                ..ask("Invent a person.")
            };
            let reply = chat_once(&endpoint, &structured, CancellationToken::new())
                .await
                .unwrap();
            eprintln!("schéma : {:?} ({:?})", reply.content, reply.finish_reason);
            let person: Value = serde_json::from_str(&reply.content).unwrap();
            assert!(
                person["name"].is_string() && person["years"].is_i64(),
                "{person}"
            );
            // Refus : contexte, image sans projecteur.
            let long = ask(&"word ".repeat(1500));
            let error = chat_stream(&endpoint, &long, CancellationToken::new())
                .await
                .err();
            eprintln!("contexte : {error:?}");
            assert!(matches!(
                error,
                Some(InferenceError::ContextExceeded {
                    n_prompt_tokens: Some(_),
                    n_ctx: Some(_)
                })
            ));
            let image = ChatRequest::new(vec![Message::user_parts(vec![
                Part::Text("What?".into()),
                Part::image("image/png", b"\x89PNG"),
            ])]);
            assert_eq!(
                chat_stream(&endpoint, &image, CancellationToken::new())
                    .await
                    .err(),
                Some(InferenceError::ImageUnsupported)
            );
            // Jetons.
            assert_eq!(tokenize(&endpoint, "Hello world").await.unwrap().len(), 2);
            // Annulation : l'emplacement est libéré.
            let cancel = CancellationToken::new();
            let long_reply = ChatRequest {
                max_tokens: Some(3000),
                ..ask("Write a very long story.")
            };
            let mut stream = chat_stream(&endpoint, &long_reply, cancel.clone())
                .await
                .unwrap();
            for _ in 0..20 {
                assert!(stream.next().await.unwrap().is_ok());
            }
            cancel.cancel();
            assert_eq!(stream.next().await, Some(Err(InferenceError::Cancelled)));
            drop(stream);
            let started = std::time::Instant::now();
            let client = reqwest::Client::builder().no_proxy().build().unwrap();
            loop {
                let slots: Value = client
                    .get(format!("{url}/slots"))
                    .bearer_auth(&key)
                    .send()
                    .await
                    .unwrap()
                    .json()
                    .await
                    .unwrap();
                if slots
                    .as_array()
                    .unwrap()
                    .iter()
                    .all(|slot| slot["is_processing"] == false)
                {
                    break;
                }
                assert!(
                    started.elapsed() < Duration::from_secs(2),
                    "emplacement toujours occupé"
                );
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
            eprintln!(
                "emplacement libre {} ms après l'annulation",
                started.elapsed().as_millis()
            );
            // Autocomplétion sur un modèle sans jetons FIM : texte sans valeur, mais 200.
            let reply = infill(
                &endpoint,
                &InfillRequest {
                    input_prefix: "def f():\n    ".into(),
                    n_predict: Some(8),
                    ..InfillRequest::default()
                },
                CancellationToken::new(),
            )
            .await
            .unwrap();
            eprintln!("infill sans FIM : {:?}", reply.content);
        }
    }
}

async fn props_of(endpoint: &Endpoint) -> Result<Props, InferenceError> {
    props(endpoint).await
}
