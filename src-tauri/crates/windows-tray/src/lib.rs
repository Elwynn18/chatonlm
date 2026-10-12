//! Crate sans code : il active la fonctionnalité `tray-icon` de Tauri sous
//! Windows, où l'app s'en sert pour sa zone de notification
//! (`desktop/tray/windows.rs`). Cargo réunit les fonctionnalités demandées par
//! tous les crates d'une même cible : l'app la reçoit par ce crate.
//!
//! Elle n'est pas écrite dans le manifeste de l'app, parce que la CLI de Tauri
//! y lit les fonctionnalités de `tauri` sans regarder la cible : la voyant sous
//! `cfg(windows)`, elle exigeait libappindicator pour empaqueter la version
//! Linux (« Can't detect any appindicator library »), ou l'ajoutait aux
//! dépendances du .deb et à l'AppImage quand elle la trouvait.
