//! Faux yt-dlp pour les e2e de la Transcription (`fake_services::ytdlp`) :
//! mêmes arguments que le vrai ; donné au faux GitHub par
//! `FAKE_GITHUB_YTDLP_BIN` (archive onedir `yt-dlp_linux.zip`).

fn main() {
    let args = std::env::args().skip(1).collect();
    std::process::exit(fake_services::ytdlp::run_yt_dlp(args));
}
