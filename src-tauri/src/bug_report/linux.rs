//! Système et bureau sous Linux, pour le diagnostic d'un signalement.

/// `PRETTY_NAME` de os-release et version du noyau.
pub fn description() -> String {
    let release = ["/etc/os-release", "/usr/lib/os-release"]
        .iter()
        .find_map(|path| std::fs::read_to_string(path).ok())
        .and_then(|text| super::os_release_name(&text))
        .unwrap_or_else(|| "Linux".to_owned());
    match std::fs::read_to_string("/proc/sys/kernel/osrelease") {
        Ok(kernel) => format!("{release}, kernel {}", kernel.trim()),
        Err(_) => release,
    }
}

/// Bureau (`XDG_CURRENT_DESKTOP`) et session (`XDG_SESSION_TYPE`).
pub fn desktop() -> Option<String> {
    let read = |name| {
        std::env::var(name)
            .ok()
            .filter(|value: &String| !value.is_empty())
    };
    match (read("XDG_CURRENT_DESKTOP"), read("XDG_SESSION_TYPE")) {
        (Some(desktop), Some(session)) => Some(format!("{desktop}, {session}")),
        (desktop, session) => desktop.or(session),
    }
}
