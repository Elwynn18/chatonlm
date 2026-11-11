//! Faux services pour les essais de ChatonLM, sans réseau ni modèle : Hugging
//! Face (catalogue du ModelHub, téléchargements directs, page d'état ; voir
//! [`hf`] et [`hf_models`]), `sd-server` (moteur d'images ; voir [`sdcpp`]
//! et le binaire `fake-sd-server`) et moteurs de recherche web (DuckDuckGo,
//! Brave, Mojeek, SearXNG ; voir [`search`] et le binaire `fake-search`). Le
//! moteur de texte a son propre faux (crate `fake-llama`).

/// Faux Hugging Face (serveur à part) : voir [`hf`].
pub mod hf;
pub mod hf_models;

/// Faux `sd-server` (moteur d'images de l'app) : voir [`sdcpp`].
pub mod sdcpp;

/// Faux moteurs de recherche web (crate `web-search`) : voir [`search`].
pub mod search;

/// Petite image PNG valide (1 × 1 pixel transparent) pour les essais.
pub const SAMPLE_PNG: [u8; 68] = [
    0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a, 0x00, 0x00, 0x00, 0x0d, 0x49, 0x48, 0x44, 0x52,
    0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x01, 0x08, 0x06, 0x00, 0x00, 0x00, 0x1f, 0x15, 0xc4,
    0x89, 0x00, 0x00, 0x00, 0x0b, 0x49, 0x44, 0x41, 0x54, 0x78, 0xda, 0x63, 0x60, 0x00, 0x02, 0x00,
    0x00, 0x05, 0x00, 0x01, 0xe9, 0xfa, 0xdc, 0xd8, 0x00, 0x00, 0x00, 0x00, 0x49, 0x45, 0x4e, 0x44,
    0xae, 0x42, 0x60, 0x82,
];

/// Demande d'arrêt d'un faux lancé en processus : SIGTERM (sous Windows :
/// Ctrl+Pause).
pub struct Terminate {
    #[cfg(unix)]
    signal: tokio::signal::unix::Signal,
    #[cfg(windows)]
    signal: tokio::signal::windows::CtrlBreak,
}

impl Terminate {
    pub fn listen() -> Terminate {
        #[cfg(unix)]
        let signal = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
            .expect("cannot listen for SIGTERM");
        #[cfg(windows)]
        let signal = tokio::signal::windows::ctrl_break().expect("cannot listen for Ctrl+Break");
        Terminate { signal }
    }

    /// Attend la prochaine demande.
    pub async fn recv(&mut self) {
        let _ = self.signal.recv().await;
    }
}

#[cfg(test)]
mod tests {
    /// L'image d'essai se décode (en-tête et données valides).
    #[test]
    fn the_sample_png_decodes() {
        let decoder = png::Decoder::new(std::io::Cursor::new(&super::SAMPLE_PNG[..]));
        let reader = decoder.read_info().unwrap();
        assert_eq!((reader.info().width, reader.info().height), (1, 1));
    }
}
