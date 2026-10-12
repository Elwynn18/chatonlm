//! ONNX Runtime sous Linux : bibliothèque officielle sous son SONAME,
//! installée par les paquets dans le dossier des ressources
//! (`/usr/lib/chatonlm`, `$APPDIR/usr/lib/chatonlm`).

/// Nom de la bibliothèque dans le dossier des ressources.
pub const LIBRARY: &str = "libonnxruntime.so.1";
