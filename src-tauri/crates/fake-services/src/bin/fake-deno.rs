//! Faux Deno pour les e2e de la Transcription (`fake_services::ytdlp`) :
//! `--version` et `run … -` ; donné au faux GitHub par
//! `FAKE_GITHUB_DENO_BIN`.

fn main() {
    let args = std::env::args().skip(1).collect();
    std::process::exit(fake_services::ytdlp::run_deno(args));
}
