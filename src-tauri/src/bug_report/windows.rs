//! Système sous Windows, pour le diagnostic d'un signalement.

const CURRENT_VERSION: &str = r"SOFTWARE\Microsoft\Windows NT\CurrentVersion";

/// Édition, version annuelle et build, lus dans le registre.
pub fn description() -> String {
    let Ok(key) = windows_registry::LOCAL_MACHINE.open(CURRENT_VERSION) else {
        return "Windows".to_owned();
    };
    super::windows_name(
        key.get_string("ProductName").ok().as_deref(),
        key.get_string("DisplayVersion").ok().as_deref(),
        key.get_string("CurrentBuildNumber").ok().as_deref(),
        key.get_u32("UBR").ok(),
    )
}

/// Pas de bureau à nommer sous Windows.
pub fn desktop() -> Option<String> {
    None
}
