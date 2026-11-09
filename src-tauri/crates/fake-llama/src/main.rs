//! Faux `llama-server`, lancé comme le vrai (par l'app avec
//! `CHATONLM_LLAMA_SERVER_BIN`, ou par un essai) : voir `cli.rs`.

#[tokio::main]
async fn main() {
    let code = fake_llama::cli::run(std::env::args().skip(1).collect()).await;
    std::process::exit(code);
}
