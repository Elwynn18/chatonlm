//! Version de l'app, lue dans son `User-Agent` (`ChatonLM/0.10.0`) : le relais
//! peut couper les versions trop anciennes (code `outdated`).

/// Version `majeur.mineur.correctif` ; un suffixe (`-beta.1`, `+build`) est
/// ignoré.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct AppVersion(pub u32, pub u32, pub u32);

impl AppVersion {
    pub fn parse(text: &str) -> Option<Self> {
        let core = text.split(['-', '+']).next()?;
        let mut parts = core.split('.');
        let mut next = || -> Option<u32> {
            let part = parts.next()?;
            if part.is_empty() || part.len() > 9 || !part.bytes().all(|b| b.is_ascii_digit()) {
                return None;
            }
            part.parse().ok()
        };
        let version = AppVersion(next()?, next()?, next()?);
        parts.next().is_none().then_some(version)
    }

    /// Version d'un `User-Agent` `ChatonLM/<version>` (premier mot).
    pub fn from_user_agent(agent: &str) -> Option<Self> {
        let product = agent.split_whitespace().next()?;
        let version = product.strip_prefix("ChatonLM/")?;
        Self::parse(version)
    }
}

impl std::fmt::Display for AppVersion {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}.{}.{}", self.0, self.1, self.2)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_and_compares() {
        assert_eq!(AppVersion::parse("0.10.0"), Some(AppVersion(0, 10, 0)));
        assert_eq!(AppVersion::parse("1.2.3-beta.1"), Some(AppVersion(1, 2, 3)));
        assert!(AppVersion(0, 10, 0) > AppVersion(0, 9, 5));
        for bad in [
            "",
            "1",
            "1.2",
            "1.2.3.4",
            "a.b.c",
            "1..3",
            "-1.2.3",
            "1.2.99999999999",
        ] {
            assert_eq!(AppVersion::parse(bad), None, "{bad}");
        }
        assert_eq!(
            AppVersion::from_user_agent("ChatonLM/0.10.1"),
            Some(AppVersion(0, 10, 1))
        );
        assert_eq!(AppVersion::from_user_agent("curl/8.0.0"), None);
        assert_eq!(AppVersion::from_user_agent(""), None);
    }
}
