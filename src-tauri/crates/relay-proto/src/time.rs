//! Jours et mois en temps universel, sans dépendance : les quotas se
//! remettent à zéro à minuit UTC, le budget le 1er du mois UTC.

const DAY: u64 = 86_400;

/// Numéro du jour (jours depuis le 1er janvier 1970).
pub fn day_index(unix: u64) -> u64 {
    unix / DAY
}

/// Début du jour suivant.
pub fn next_day_start(unix: u64) -> u64 {
    (day_index(unix) + 1) * DAY
}

/// Année et mois (1 à 12).
pub fn year_month(unix: u64) -> (i64, u32) {
    let (year, month, _) = civil_from_days(day_index(unix) as i64);
    (year, month)
}

/// Clé du mois : « 2026-11 ».
pub fn month_key(unix: u64) -> String {
    let (year, month) = year_month(unix);
    format!("{year:04}-{month:02}")
}

/// Début du mois suivant (1er à 0 h UTC).
pub fn next_month_start(unix: u64) -> u64 {
    let (year, month) = year_month(unix);
    let (year, month) = if month == 12 {
        (year + 1, 1)
    } else {
        (year, month + 1)
    };
    (days_from_civil(year, month, 1).max(0) as u64) * DAY
}

/// Nombre de jours du mois de `unix` (28 à 31).
pub fn days_in_month(unix: u64) -> u64 {
    let (year, month) = year_month(unix);
    let start = days_from_civil(year, month, 1).max(0) as u64;
    next_month_start(unix) / DAY - start
}

/// Date civile d'un numéro de jour (algorithme de Howard Hinnant).
pub fn civil_from_days(days: i64) -> (i64, u32, u32) {
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    (if m <= 2 { y + 1 } else { y }, m, d)
}

/// Numéro de jour d'une date civile.
pub fn days_from_civil(year: i64, month: u32, day: u32) -> i64 {
    let y = if month <= 2 { year - 1 } else { year };
    let era = y.div_euclid(400);
    let yoe = y.rem_euclid(400);
    let m = i64::from(month);
    let doy = (153 * (if m > 2 { m - 3 } else { m + 9 }) + 2) / 5 + i64::from(day) - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn known_dates() {
        assert_eq!(civil_from_days(0), (1970, 1, 1));
        // 8 nov. 2026, 12 h UTC.
        let t = 1_794_139_200;
        assert_eq!(month_key(t), "2026-11");
        assert_eq!(civil_from_days(day_index(t) as i64), (2026, 11, 8));
        // 1er déc. 2026, 0 h UTC.
        assert_eq!(next_month_start(t), 1_796_083_200);
        assert_eq!(month_key(next_month_start(t)), "2026-12");
        assert_eq!(month_key(next_month_start(t) - 1), "2026-11");
        // Décembre → janvier de l'année suivante.
        assert_eq!(month_key(next_month_start(1_797_292_800)), "2027-01");
        // 29 février.
        assert_eq!(civil_from_days(days_from_civil(2028, 2, 29)), (2028, 2, 29));
        assert_eq!(next_day_start(t), 1_794_182_400);
        assert_eq!(days_in_month(t), 30);
        assert_eq!(days_in_month(next_month_start(t)), 31);
        assert_eq!(days_in_month(days_from_civil(2028, 2, 10) as u64 * DAY), 29);
        assert_eq!(days_in_month(days_from_civil(2027, 2, 28) as u64 * DAY), 28);
    }

    #[test]
    fn round_trip_over_years() {
        for days in (0..80_000).step_by(17) {
            let (y, m, d) = civil_from_days(days);
            assert_eq!(days_from_civil(y, m, d), days);
        }
    }
}
