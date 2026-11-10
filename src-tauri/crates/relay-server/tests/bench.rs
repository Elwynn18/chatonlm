//! Banc R0 (`chatonlm-relay bench`) contre les faux fournisseurs : les quatre
//! fournisseurs, un refus d'`extra_snippets`, les erreurs, une clé absente,
//! et le binaire lui-même. Aucune clé, aucun titre ni aucune adresse dans le
//! résumé.

use relay_server::bench::{self, BenchDefaults, BenchProvider, Options, QUERIES, Report};
use relay_server::config::{ProvidersConfig, Secret, Secrets};
use relay_server::fake::{
    BRAVE_TEST_KEY, Behavior, CERAMIC_TEST_KEY, FakeProviders, LINKUP_TEST_KEY, MOJEEK_TEST_KEY,
};
use std::time::Duration;

const KEYS: [&str; 4] = [
    BRAVE_TEST_KEY,
    MOJEEK_TEST_KEY,
    CERAMIC_TEST_KEY,
    LINKUP_TEST_KEY,
];

fn secrets(keys: [bool; 4]) -> Secrets {
    let key = |on: bool, value: &str| {
        if on {
            Secret::new(value)
        } else {
            Secret::default()
        }
    };
    Secrets {
        brave_api_key: key(keys[0], BRAVE_TEST_KEY),
        mojeek_api_key: key(keys[1], MOJEEK_TEST_KEY),
        ceramic_api_key: key(keys[2], CERAMIC_TEST_KEY),
        linkup_api_key: key(keys[3], LINKUP_TEST_KEY),
        ..Secrets::default()
    }
}

fn providers(url: &str) -> ProvidersConfig {
    let mut providers = ProvidersConfig::default();
    providers.brave.base_url = url.to_owned();
    providers.mojeek.base_url = url.to_owned();
    providers.brave.timeout_secs = 2;
    providers.mojeek.timeout_secs = 2;
    providers
}

fn defaults(url: &str) -> BenchDefaults {
    BenchDefaults {
        ceramic_url: url.to_owned(),
        linkup_url: url.to_owned(),
        timeout: Duration::from_secs(2),
    }
}

async fn run(fake: &FakeProviders, keys: [bool; 4], queries: usize) -> Report {
    relay_server::install_crypto_provider();
    let targets = bench::targets(&providers(&fake.url), &secrets(keys), &defaults(&fake.url))
        .expect("targets");
    let options = Options {
        pace: Duration::ZERO,
        show: false,
    };
    bench::run(targets, &QUERIES[..queries], &options).await
}

fn provider(report: &Report, which: BenchProvider) -> &bench::Aggregate {
    &report
        .providers
        .iter()
        .find(|p| p.provider == which)
        .expect("provider in the report")
        .all
}

/// Ni clé, ni titre, ni adresse, ni requête dans le résumé (texte et JSON).
fn assert_clean(report: &Report) {
    let text = report.text();
    let json = serde_json::to_string(report).unwrap();
    for out in [&text, &json] {
        for key in KEYS {
            assert!(!out.contains(key), "clé dans le résumé");
        }
        assert!(!out.contains("example.org"), "adresse dans le résumé");
        assert!(!out.contains("Résultat"), "titre dans le résumé");
        assert!(!QUERIES.iter().any(|q| out.contains(q.text)), "requête");
    }
}

#[tokio::test]
async fn the_four_providers_are_measured_with_their_own_protocol() {
    let fake = FakeProviders::start().await;
    let report = run(&fake, [true; 4], 3).await;
    assert_eq!(report.queries, 3);
    assert_eq!(report.providers.len(), 4);
    for which in BenchProvider::ALL {
        let all = provider(&report, which);
        assert_eq!((all.sent, all.ok, all.billed), (3, 3, 3), "{which:?}");
        assert!(all.errors.is_empty(), "{which:?}");
        assert!(all.results_mean >= 10.0, "{which:?}: {}", all.results_mean);
        assert!(all.snippet_ratio > 0.5, "{which:?}");
    }
    // Brave : extraits supplémentaires demandés et comptés ; les autres n'en ont pas.
    assert!(provider(&report, BenchProvider::Brave).extra_snippet_ratio > 0.5);
    assert_eq!(
        provider(&report, BenchProvider::Mojeek).extra_snippet_ratio,
        0.0
    );
    assert!(!report.brave_extra_snippets_refused);
    let brave = fake.requests();
    assert!(
        brave
            .iter()
            .filter(|r| r.provider == relay_proto::api::Provider::Brave)
            .all(|r| r.params.get("extra_snippets").map(String::as_str) == Some("true"))
    );
    // Linkup : l'image n'est pas comptée (10 textes au plus).
    assert_eq!(provider(&report, BenchProvider::Linkup).results_mean, 10.0);
    // Ceramic et Linkup : jeton Bearer, corps de leur API.
    let requests = fake.bench_requests();
    assert_eq!(requests.len(), 6);
    for request in &requests {
        match request.provider {
            BenchProvider::Ceramic => {
                assert_eq!(request.token.as_deref(), Some(CERAMIC_TEST_KEY));
                assert_eq!(request.body["maxResults"], 10);
                assert!(request.body["query"].is_string());
            }
            BenchProvider::Linkup => {
                assert_eq!(request.token.as_deref(), Some(LINKUP_TEST_KEY));
                assert_eq!(request.body["depth"], "standard");
                assert_eq!(request.body["outputType"], "searchResults");
            }
            other => panic!("{other:?}"),
        }
    }
    // Accord : six paires, chacune sur les trois requêtes.
    assert_eq!(report.agreement.len(), 6);
    assert!(report.agreement.iter().all(|(.., n)| *n == 3));
    // Coût : Brave 3 × 5 € / 1 000.
    assert!((provider(&report, BenchProvider::Brave).cost_eur - 0.015).abs() < 1e-9);
    assert_clean(&report);
}

#[tokio::test]
async fn brave_without_extra_snippets_in_its_plan_is_measured_without() {
    let fake = FakeProviders::start().await;
    fake.set_bench(BenchProvider::Brave, Behavior::NoExtraSnippets);
    let report = run(&fake, [true, false, false, false], 3).await;
    let brave = provider(&report, BenchProvider::Brave);
    assert_eq!((brave.sent, brave.ok), (3, 3));
    assert!(report.brave_extra_snippets_refused);
    assert!(report.text().contains("extra_snippets refused"));
    // Un seul essai refusé, puis plus jamais demandé.
    let asked = fake
        .requests()
        .iter()
        .filter(|r| r.params.contains_key("extra_snippets"))
        .count();
    assert_eq!(asked, 1);
}

#[tokio::test]
async fn errors_are_counted_by_kind_without_leaking_keys() {
    let fake = FakeProviders::start().await;
    // Les faux recopient la clé reçue dans le corps de leurs erreurs.
    fake.set_bench(BenchProvider::Brave, Behavior::Garbage);
    fake.set_bench(BenchProvider::Mojeek, Behavior::DailyLimit);
    fake.set_bench(BenchProvider::Ceramic, Behavior::Status(401));
    fake.set_bench(BenchProvider::Linkup, Behavior::Status(429));
    let report = run(&fake, [true; 4], 2).await;
    let errors = |which| provider(&report, which).errors.clone();
    assert_eq!(
        errors(BenchProvider::Brave).get("malformed reply"),
        Some(&2)
    );
    assert_eq!(
        errors(BenchProvider::Mojeek).get("credit exhausted"),
        Some(&2)
    );
    assert_eq!(errors(BenchProvider::Ceramic).get("key refused"), Some(&2));
    assert_eq!(errors(BenchProvider::Linkup).get("rate limited"), Some(&2));
    // Une réponse illisible a pu être facturée, un refus non.
    assert_eq!(provider(&report, BenchProvider::Brave).billed, 2);
    assert_eq!(provider(&report, BenchProvider::Ceramic).billed, 0);
    assert!(report.text().contains("errors: "));
    assert_clean(&report);
}

/// Chaque statut HTTP d'erreur a sa sorte (`providers::send`, commun au relais).
#[tokio::test]
async fn http_statuses_map_to_their_kind() {
    let fake = FakeProviders::start().await;
    for (status, kind) in [
        (400, "request rejected"),
        (401, "key refused"),
        (402, "credit exhausted"),
        (403, "key refused"),
        (422, "request rejected"),
        (429, "rate limited"),
        (500, "unavailable"),
        (503, "unavailable"),
    ] {
        fake.set_bench(BenchProvider::Ceramic, Behavior::Status(status));
        let report = run(&fake, [false, false, true, false], 1).await;
        let errors = &provider(&report, BenchProvider::Ceramic).errors;
        assert_eq!(errors.get(kind), Some(&1), "{status}: {errors:?}");
    }
}

#[tokio::test]
async fn a_missing_key_skips_its_provider() {
    let fake = FakeProviders::start().await;
    let report = run(&fake, [false, true, false, true], 1).await;
    let names: Vec<_> = report.providers.iter().map(|p| p.provider).collect();
    assert_eq!(names, [BenchProvider::Mojeek, BenchProvider::Linkup]);
    assert!(fake.count(relay_proto::api::Provider::Brave) == 0);
    assert!(
        fake.bench_requests()
            .iter()
            .all(|r| r.provider == BenchProvider::Linkup)
    );
}

/// Le binaire : `bench` avec la configuration du relais (Brave et Mojeek vers
/// les faux), `--queries`, `--out` ; puis sans aucune clé.
#[tokio::test(flavor = "multi_thread")]
async fn the_binary_runs_the_bench() {
    let fake = FakeProviders::start().await;
    let dir = tempfile::tempdir().unwrap();
    let config = dir.path().join("config.toml");
    std::fs::write(
        &config,
        format!(
            "database = {:?}\n[providers.brave]\nbase_url = {:?}\ntimeout_secs = 2\n\
             [providers.mojeek]\nbase_url = {:?}\ntimeout_secs = 2\n",
            dir.path().join("relay.sqlite").to_string_lossy(),
            fake.url,
            fake.url
        ),
    )
    .unwrap();
    let secrets = dir.path().join("secrets.toml");
    std::fs::write(
        &secrets,
        format!("brave_api_key = {BRAVE_TEST_KEY:?}\nmojeek_api_key = {MOJEEK_TEST_KEY:?}\n"),
    )
    .unwrap();
    let out = dir.path().join("summary.json");
    let binary = env!("CARGO_BIN_EXE_chatonlm-relay");
    let args: Vec<std::ffi::OsString> = vec![
        "bench".into(),
        "--config".into(),
        config.clone().into(),
        "--secrets".into(),
        secrets.clone().into(),
        "--queries".into(),
        "2".into(),
        "--out".into(),
        out.clone().into(),
    ];
    let output = tokio::task::spawn_blocking(move || {
        std::process::Command::new(binary)
            .args(args)
            .env_remove("CREDENTIALS_DIRECTORY")
            .output()
            .unwrap()
    })
    .await
    .unwrap();
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(output.status.success(), "{stdout}\n{stderr}");
    assert!(stdout.contains("2 queries"), "{stdout}");
    assert!(stdout.lines().any(|l| l.starts_with("brave ")), "{stdout}");
    assert!(stdout.lines().any(|l| l.starts_with("mojeek ")), "{stdout}");
    let json: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&out).unwrap()).unwrap();
    assert_eq!(json["queries"], 2);
    assert_eq!(json["providers"][0]["all"]["ok"], 2);
    let file = std::fs::read_to_string(&out).unwrap();
    for text in [&*stdout, &*stderr, &file] {
        assert!(!text.contains(BRAVE_TEST_KEY) && !text.contains(MOJEEK_TEST_KEY));
        assert!(!text.contains("example.org"));
    }
    // Le relais ne crée aucune base pour un banc.
    assert!(!dir.path().join("relay.sqlite").exists());

    // Sans clé : rien à mesurer, échec net.
    std::fs::write(&secrets, "").unwrap();
    let output = std::process::Command::new(binary)
        .args(["bench", "--secrets"])
        .arg(&secrets)
        .env_remove("CREDENTIALS_DIRECTORY")
        .output()
        .unwrap();
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("no provider key"));
    // Options du banc refusées ailleurs.
    let output = std::process::Command::new(binary)
        .args(["check", "--config"])
        .arg(&config)
        .arg("--show")
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(2));
}
