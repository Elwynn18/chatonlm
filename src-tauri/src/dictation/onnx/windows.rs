//! ONNX Runtime sous Windows : `onnxruntime.dll` de l'archive officielle,
//! installée à côté de l'exécutable (dossier des ressources de Tauri), avec
//! le runtime MSVC qu'elle exige (`msvcp140*.dll`, `vcruntime140*.dll`).
//! Toujours chargée par son chemin absolu : l'ancienne `onnxruntime.dll` de
//! `C:\Windows\System32` (Windows ML) n'est jamais prise à sa place.

/// Nom de la bibliothèque dans le dossier des ressources.
pub const LIBRARY: &str = "onnxruntime.dll";
