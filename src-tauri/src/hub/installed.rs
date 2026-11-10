//! Modèles installés (onglet Installed du ModelHub) et suppression, lus dans
//! la base locale des modèles (`local_models`, migration 0039) depuis le
//! passage à llama-server : plus de `/api/tags` ni de `DELETE /api/delete`.
//! Ordre : du plus récent au plus ancien.

use std::path::Path;

use super::types::HubInstalled;
use crate::error::AppResult;
use crate::local_models;
use crate::store::Store;
use crate::store::local_models::{self as table, StoredLocalModel};

/// Modèles de la base locale, du plus récent au plus ancien.
pub async fn list(store: &Store) -> AppResult<Vec<HubInstalled>> {
    store
        .call(|connection| Ok(table::list(connection)?.iter().map(installed).collect()))
        .await
}

/// Ligne de la base pour la page : capacités sous les noms d'Ollama que la
/// page lit (`completion`, `vision`, `tools`, `thinking`, `insert`), date
/// d'installation en RFC 3339.
pub fn installed(model: &StoredLocalModel) -> HubInstalled {
    let caps = model.capabilities;
    let capabilities = std::iter::once("completion")
        .chain(caps.vision.then_some("vision"))
        .chain(caps.tools.then_some("tools"))
        .chain(caps.thinking.then_some("thinking"))
        .chain(caps.fim.then_some("insert"))
        .map(str::to_owned)
        .collect();
    HubInstalled {
        model: model.id.clone(),
        size: model.size_bytes,
        modified_at: Some(rfc3339(model.installed_at)),
        parameter_size: model.params_label.clone().unwrap_or_default(),
        quantization_level: model.quant.clone(),
        family: model.arch.clone().unwrap_or_default(),
        capabilities,
        repo: (model.source == local_models::ModelSource::Hf).then(|| model.repo.clone()),
    }
}

/// Supprime le modèle `model` de la base locale et ses fichiers, sauf ceux
/// que `reserved` garde (voir [`local_models::delete`]).
pub async fn delete(
    store: &Store,
    data_dir: &Path,
    model: &str,
    reserved: &local_models::Reserved,
) -> AppResult<()> {
    if model.trim().is_empty() {
        // i18n-ignore : la page ne demande jamais un nom vide.
        return Err(crate::error::AppError::Invalid(
            "model name is empty".into(),
        ));
    }
    local_models::delete(store, data_dir, model.trim(), reserved).await
}

/// Date RFC 3339 en UTC (« 2026-11-08T10:00:00.123Z ») de `ms` depuis 1970.
pub fn rfc3339(ms: i64) -> String {
    let seconds = ms.div_euclid(1000);
    let millis = ms.rem_euclid(1000);
    let (days, rest) = (seconds.div_euclid(86_400), seconds.rem_euclid(86_400));
    let (year, month, day) = civil_from_days(days);
    format!(
        "{year:04}-{month:02}-{day:02}T{:02}:{:02}:{:02}.{millis:03}Z",
        rest / 3600,
        rest % 3600 / 60,
        rest % 60
    )
}

/// Date d'un nombre de jours depuis 1970 (inverse de `days_from_civil` ;
/// algorithme de Howard Hinnant, `civil_from_days`).
fn civil_from_days(days: i64) -> (i64, i64, i64) {
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let day_of_era = z.rem_euclid(146_097);
    let year_of_era =
        (day_of_era - day_of_era / 1460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let month_index = (5 * day_of_year + 2) / 153;
    let day = day_of_year - (153 * month_index + 2) / 5 + 1;
    let month = if month_index < 10 {
        month_index + 3
    } else {
        month_index - 9
    };
    let year = year_of_era + era * 400 + i64::from(month <= 2);
    (year, month, day)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::local_models::tests::model;

    /// Date RFC 3339 (« 2026-10-04T12:00:00.123456789+02:00 », « …Z ») en
    /// nanosecondes depuis 1970 ; `None` si illisible (relit ce qu'écrit
    /// [`rfc3339`]).
    fn timestamp(text: &str) -> Option<i128> {
        let number = |start: usize, len: usize| -> Option<i64> {
            let part = text.get(start..start + len)?;
            part.bytes()
                .all(|byte| byte.is_ascii_digit())
                .then(|| part.parse().ok())?
        };
        let separators = [(4, b'-'), (7, b'-'), (13, b':'), (16, b':')];
        let bytes = text.as_bytes();
        if bytes.len() < 20
            || separators.iter().any(|&(at, byte)| bytes[at] != byte)
            || !matches!(bytes[10], b'T' | b't' | b' ')
        {
            return None;
        }
        let (year, month, day) = (number(0, 4)?, number(5, 2)?, number(8, 2)?);
        let (hour, minute, second) = (number(11, 2)?, number(14, 2)?, number(17, 2)?);
        if !(1..=12).contains(&month) || !(1..=31).contains(&day) || hour > 23 || minute > 59 {
            return None;
        }
        if second > 60 {
            return None;
        }
        let mut rest = &text[19..];
        let mut nanos: i128 = 0;
        if let Some(fraction) = rest.strip_prefix('.') {
            let digits = fraction.bytes().take_while(u8::is_ascii_digit).count();
            if digits == 0 {
                return None;
            }
            for (index, byte) in fraction.bytes().take(digits).enumerate() {
                if index < 9 {
                    nanos = nanos * 10 + i128::from(byte - b'0');
                }
            }
            for _ in digits..9 {
                nanos *= 10;
            }
            rest = &fraction[digits..];
        }
        let offset: i64 = match rest.as_bytes() {
            [b'Z' | b'z'] => 0,
            [sign @ (b'+' | b'-'), h1, h2, b':', m1, m2] => {
                let digits = [*h1, *h2, *m1, *m2];
                if !digits.iter().all(u8::is_ascii_digit) {
                    return None;
                }
                let value = |a: u8, b: u8| i64::from(a - b'0') * 10 + i64::from(b - b'0');
                let minutes = value(*h1, *h2) * 60 + value(*m1, *m2);
                if *sign == b'-' { -minutes } else { minutes }
            }
            _ => return None,
        };
        let seconds =
            days_from_civil(year, month, day) * 86_400 + hour * 3_600 + minute * 60 + second
                - offset * 60;
        Some(i128::from(seconds) * 1_000_000_000 + nanos)
    }

    /// Jours depuis le 1er janvier 1970 (calendrier grégorien proleptique ;
    /// algorithme de Howard Hinnant, `days_from_civil`).
    fn days_from_civil(year: i64, month: i64, day: i64) -> i64 {
        let year = if month <= 2 { year - 1 } else { year };
        let era = year.div_euclid(400);
        let year_of_era = year - era * 400;
        let month_index = (month + 9) % 12;
        let day_of_year = (153 * month_index + 2) / 5 + day - 1;
        let day_of_era = year_of_era * 365 + year_of_era / 4 - year_of_era / 100 + day_of_year;
        era * 146_097 + day_of_era - 719_468
    }

    #[test]
    fn dates_are_read_like_rfc_3339() {
        assert_eq!(timestamp("1970-01-01T00:00:00Z"), Some(0));
        assert_eq!(
            timestamp("1970-01-02T00:00:00Z"),
            Some(86_400 * 1_000_000_000)
        );
        // 2000-03-01 : après un 29 février.
        assert_eq!(
            timestamp("2000-03-01T00:00:00Z"),
            Some(951_868_800 * 1_000_000_000)
        );
        // Fuseaux : la même date.
        assert_eq!(
            timestamp("2026-10-04T12:00:00+02:00"),
            timestamp("2026-10-04T10:00:00Z")
        );
        assert_eq!(
            timestamp("2026-10-04T05:30:00-04:30"),
            timestamp("2026-10-04T10:00:00Z")
        );
        // Fractions : nanosecondes, au-delà de 9 chiffres ignorées.
        assert_eq!(timestamp("1970-01-01T00:00:00.5Z"), Some(500_000_000));
        assert_eq!(
            timestamp("1970-01-01T00:00:00.123456789123+00:00"),
            Some(123_456_789)
        );
        for bad in [
            "",
            "yesterday",
            "2026-10-04",
            "2026-10-04T12:00:00",
            "2026-13-04T12:00:00Z",
            "2026-10-04T12:00:00.Z",
            "2026-10-04T12:00:00+2:00",
            "2026/10/04T12:00:00Z",
            "2026-10-04T12:00:00Zx",
            "+026-10-04T12:00:00Z",
        ] {
            assert_eq!(timestamp(bad), None, "{bad}");
        }
    }

    #[test]
    fn dates_are_written_in_rfc_3339() {
        for ms in [0, 1, 951_868_800_000, 1_794_000_000_123, -86_400_000] {
            let text = rfc3339(ms);
            assert_eq!(
                timestamp(&text),
                Some(i128::from(ms) * 1_000_000),
                "{ms} {text}"
            );
        }
        assert_eq!(rfc3339(0), "1970-01-01T00:00:00.000Z");
        assert_eq!(rfc3339(951_868_800_000), "2000-03-01T00:00:00.000Z");
    }

    #[test]
    fn installed_models_newest_first_with_their_capabilities() {
        let store = Store::open_in_memory().unwrap();
        let mut old = model("u/A-GGUF:Q4_K_M", &["u/A-GGUF/A-Q4_K_M.gguf"], None);
        old.capabilities.thinking = false;
        old.capabilities.fim = true;
        let mut new = model(
            "u/B-GGUF:Q8_0",
            &["u/B-GGUF/B.gguf"],
            Some("u/B-GGUF/mmproj.gguf"),
        );
        new.installed_at = 1_794_000_000_000;
        new.params_label = None;
        store
            .blocking(|c| {
                table::save(c, &old)?;
                table::save(c, &new)
            })
            .unwrap();
        let listed = tauri::async_runtime::block_on(list(&store)).unwrap();
        let names: Vec<_> = listed.iter().map(|m| m.model.as_str()).collect();
        assert_eq!(names, ["u/B-GGUF:Q8_0", "u/A-GGUF:Q4_K_M"]);
        assert_eq!(
            listed[0].capabilities,
            ["completion", "vision", "tools", "thinking"]
        );
        assert_eq!(listed[1].capabilities, ["completion", "tools", "insert"]);
        assert_eq!(listed[0].repo.as_deref(), Some("u/B-GGUF"));
        assert_eq!(listed[0].size, 1234);
        assert_eq!(listed[0].parameter_size, "");
        assert_eq!(listed[1].parameter_size, "0.6B");
        assert_eq!(listed[1].quantization_level, "Q4_K_M");
        assert_eq!(listed[1].family, "qwen3");
        assert_eq!(
            listed[1].modified_at.as_deref(),
            Some("1970-01-01T00:00:00.010Z")
        );
        // Importé d'Ollama : pas de dépôt Hugging Face.
        let mut imported = old.clone();
        imported.source = local_models::ModelSource::Ollama;
        assert_eq!(installed(&imported).repo, None);
    }
}
