//! Faux `sd-server` (moteur d'images) pour les e2e : mêmes arguments que le
//! vrai (`--listen-ip`, `--listen-port`, fichiers du modèle...). Voir
//! `fake_services::sdcpp`.

#[tokio::main]
async fn main() {
    let args = std::env::args().skip(1).collect();
    std::process::exit(fake_services::sdcpp::run(args).await);
}
