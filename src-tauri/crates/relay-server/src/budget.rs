//! Budget du mois (le vrai garde-fou) : chaque appel à un fournisseur est
//! réservé avant d'être fait, au prix du fournisseur, et rendu s'il n'est pas
//! facturé (refus, erreur avant l'envoi). Compté en micro-euros entiers, gardé
//! en base à chaque changement (un redémarrage ne remet rien à zéro).
//!
//! Deux plafonds :
//! - le **mois** : au-delà, plus aucun appel jusqu'au 1er du mois suivant
//!   (UTC) ;
//! - l'**enveloppe du jour** : plafond du mois × `daily_share` ÷ jours du mois
//!   (3 jours moyens par défaut). Une vague d'abus épuise une journée, pas le
//!   reste du mois : la recherche reprend à minuit UTC.
//!
//! Dans les deux cas : `budget_exhausted`, `retryAt` = moment de la reprise.

use crate::store::MonthRecord;
use relay_proto::api::Provider;
use relay_proto::time::{day_index, days_in_month, month_key, next_day_start, next_month_start};

/// Euros → micro-euros (arrondi vers le haut : on compte large).
pub fn micro_eur(eur: f64) -> u64 {
    (eur * 1_000_000.0).ceil().max(0.0) as u64
}

pub struct Budget {
    limit: u64,
    alert_at: u64,
    daily_share: f64,
    month: String,
    record: MonthRecord,
    alerted: bool,
    exhausted_noted: bool,
    day: u64,
    day_spent: u64,
    day_noted: bool,
}

/// Ce qui mérite une ligne au journal après une réservation.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Notice {
    None,
    /// Seuil d'avertissement franchi ce mois-ci.
    Alert,
}

/// Plafond atteint, et moment de la reprise.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Exhausted {
    /// Plafond du mois : 1er du mois suivant.
    Month(u64),
    /// Enveloppe du jour : minuit UTC.
    Day(u64),
}

impl Exhausted {
    pub fn retry_at(self) -> u64 {
        match self {
            Exhausted::Month(at) | Exhausted::Day(at) => at,
        }
    }
}

/// Somme réservée pour un appel, rattachée à son mois et à son jour.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Reservation {
    month: String,
    day: u64,
    cost: u64,
}

impl Budget {
    /// `day` et `day_spent` : dépense du jour gardée en base (un redémarrage
    /// ne rouvre pas l'enveloppe).
    pub fn new(
        limit_eur: f64,
        alert_ratio: f64,
        daily_share: f64,
        month: String,
        record: MonthRecord,
        day: u64,
        day_spent: u64,
    ) -> Self {
        let limit = micro_eur(limit_eur);
        let alert_at = (limit as f64 * alert_ratio.clamp(0.0, 1.0)) as u64;
        Self {
            limit,
            alert_at,
            daily_share: daily_share.max(1.0),
            alerted: record.spent_micro_eur >= alert_at,
            exhausted_noted: false,
            month,
            record,
            day,
            day_spent,
            day_noted: false,
        }
    }

    pub fn month(&self) -> &str {
        &self.month
    }

    pub fn record(&self) -> &MonthRecord {
        &self.record
    }

    /// Jour courant et sa dépense.
    pub fn day(&self) -> (u64, u64) {
        (self.day, self.day_spent)
    }

    /// Enveloppe du jour de `now` (micro-euros).
    pub fn day_limit(&self, now: u64) -> u64 {
        let share = self.limit as f64 * self.daily_share / days_in_month(now) as f64;
        (share.ceil() as u64).min(self.limit)
    }

    /// Passe au mois et au jour de `now` si besoin ; vrai si le mois a changé.
    pub fn roll(&mut self, now: u64) -> bool {
        let today = day_index(now);
        if today != self.day {
            self.day = today;
            self.day_spent = 0;
            self.day_noted = false;
        }
        let month = month_key(now);
        if month == self.month {
            return false;
        }
        self.month = month;
        self.record = MonthRecord::default();
        self.alerted = false;
        self.exhausted_noted = false;
        true
    }

    /// Ce qui manquerait pour un appel de `cost`.
    fn blocked(&self, now: u64, cost: u64) -> Option<Exhausted> {
        if self.record.spent_micro_eur.saturating_add(cost) > self.limit {
            Some(Exhausted::Month(next_month_start(now)))
        } else if self.day_spent.saturating_add(cost) > self.day_limit(now) {
            Some(Exhausted::Day(next_day_start(now)))
        } else {
            None
        }
    }

    /// Plus rien ne passe, même le moins cher des fournisseurs : jusqu'à quand.
    pub fn exhausted(&mut self, now: u64, cheapest: u64) -> Option<Exhausted> {
        self.roll(now);
        self.blocked(now, cheapest)
    }

    /// Réserve `cost` ; `Err` si un plafond serait dépassé.
    pub fn reserve(&mut self, now: u64, cost: u64) -> Result<(Reservation, Notice), Exhausted> {
        self.roll(now);
        if let Some(exhausted) = self.blocked(now, cost) {
            return Err(exhausted);
        }
        self.record.spent_micro_eur += cost;
        self.day_spent += cost;
        let reservation = Reservation {
            month: self.month.clone(),
            day: self.day,
            cost,
        };
        if !self.alerted && self.record.spent_micro_eur >= self.alert_at {
            self.alerted = true;
            return Ok((reservation, Notice::Alert));
        }
        Ok((reservation, Notice::None))
    }

    /// Vrai au premier refus de la période seulement (une ligne au journal).
    pub fn take_exhausted_notice(&mut self, exhausted: Exhausted) -> bool {
        let noted = match exhausted {
            Exhausted::Month(_) => &mut self.exhausted_noted,
            Exhausted::Day(_) => &mut self.day_noted,
        };
        !std::mem::replace(noted, true)
    }

    /// Appel non facturé : la réservation est rendue (au mois et au jour où
    /// elle a été prise ; une réservation d'une période écoulée ne touche pas
    /// à la nouvelle).
    pub fn refund(&mut self, now: u64, reservation: Reservation) {
        self.roll(now);
        if reservation.day == self.day {
            self.day_spent = self.day_spent.saturating_sub(reservation.cost);
        }
        if reservation.month != self.month {
            return;
        }
        self.record.spent_micro_eur = self.record.spent_micro_eur.saturating_sub(reservation.cost);
        self.record.failures += 1;
    }

    /// Appel facturé et réussi.
    pub fn count(&mut self, now: u64, provider: Provider) {
        self.roll(now);
        match provider {
            Provider::Brave => self.record.brave += 1,
            Provider::Mojeek => self.record.mojeek += 1,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const T: u64 = 1_794_139_200; // 8 nov. 2026.

    /// Sans enveloppe du jour (part ≥ jours du mois).
    fn monthly(limit: f64, alert: f64, record: MonthRecord) -> Budget {
        Budget::new(limit, alert, 31.0, month_key(T), record, day_index(T), 0)
    }

    #[test]
    fn reserve_until_the_cap_then_wait_for_next_month() {
        // 1 € de plafond, 0,25 € l'appel : 4 appels.
        let mut budget = monthly(1.0, 0.5, MonthRecord::default());
        assert_eq!(budget.reserve(T, 250_000).unwrap().1, Notice::None);
        assert_eq!(budget.reserve(T, 250_000).unwrap().1, Notice::Alert);
        let (third, notice) = budget.reserve(T, 250_000).unwrap();
        assert_eq!(notice, Notice::None);
        budget.reserve(T, 250_000).unwrap();
        let month = Exhausted::Month(next_month_start(T));
        assert_eq!(budget.reserve(T, 250_000), Err(month));
        assert_eq!(budget.exhausted(T, 1), Some(month));
        assert!(budget.take_exhausted_notice(month));
        assert!(!budget.take_exhausted_notice(month));
        // Un appel rendu libère sa place.
        budget.refund(T, third);
        assert_eq!(budget.exhausted(T, 250_000), None);
        assert_eq!(budget.record().failures, 1);
        // Le mois suivant repart de zéro.
        let next = next_month_start(T);
        assert_eq!(budget.exhausted(next, 250_000), None);
        assert_eq!(budget.reserve(next, 250_000).unwrap().1, Notice::None);
        assert_eq!(budget.month(), "2026-12");
        assert!(budget.take_exhausted_notice(month));
    }

    #[test]
    fn spending_survives_a_restart() {
        let record = MonthRecord {
            spent_micro_eur: 999_999,
            ..Default::default()
        };
        let mut budget = monthly(1.0, 0.8, record);
        assert_eq!(
            budget.reserve(T, 2),
            Err(Exhausted::Month(next_month_start(T)))
        );
        assert!(budget.reserve(T, 1).is_ok());
    }

    #[test]
    fn a_refund_from_last_month_does_not_touch_this_one() {
        let mut budget = monthly(1.0, 0.8, MonthRecord::default());
        let (old, _) = budget.reserve(T, 500_000).unwrap();
        let next = next_month_start(T);
        budget.reserve(next, 100_000).unwrap();
        budget.refund(next + 5, old);
        assert_eq!(budget.record().spent_micro_eur, 100_000);
        assert_eq!(budget.day(), (day_index(next), 100_000));
        assert_eq!(micro_eur(30.0), 30_000_000);
        assert_eq!(micro_eur(5.0 / 1000.0), 5_000);
    }

    /// I1 : un jour ne dépense pas plus de 3 jours moyens ; le lendemain,
    /// l'enveloppe se rouvre, et le reste du mois demeure.
    #[test]
    fn a_day_cannot_spend_more_than_its_envelope() {
        // 30 € en novembre (30 jours) : 3 € par jour, soit 600 appels Brave.
        let mut budget = Budget::new(
            30.0,
            0.8,
            3.0,
            month_key(T),
            MonthRecord::default(),
            day_index(T),
            0,
        );
        assert_eq!(budget.day_limit(T), 3_000_000);
        for _ in 0..600 {
            budget.reserve(T, 5_000).unwrap();
        }
        let day = Exhausted::Day(next_day_start(T));
        assert_eq!(budget.reserve(T, 5_000), Err(day));
        assert_eq!(budget.exhausted(T, 2_500), Some(day));
        assert!(budget.take_exhausted_notice(day));
        assert!(!budget.take_exhausted_notice(day));
        // Un appel rendu rouvre sa place dans le jour aussi ; un redémarrage
        // garde la dépense du jour (gardée en base, relue ici).
        let mut restarted = Budget::new(
            30.0,
            0.8,
            3.0,
            month_key(T),
            MonthRecord::default(),
            day_index(T),
            2_995_000,
        );
        let (reserved, _) = restarted.reserve(T, 5_000).unwrap();
        assert_eq!(restarted.exhausted(T, 1), Some(day));
        restarted.refund(T, reserved);
        assert_eq!(restarted.exhausted(T, 5_000), None);
        // Le lendemain : de nouveau 3 €, et 27 € restent au mois.
        let tomorrow = next_day_start(T);
        assert_eq!(budget.exhausted(tomorrow, 5_000), None);
        budget.reserve(tomorrow, 5_000).unwrap();
        assert_eq!(budget.record().spent_micro_eur, 3_005_000);
        assert!(budget.take_exhausted_notice(day));
        // Décembre (31 jours) : 30 × 3 ÷ 31.
        assert_eq!(budget.day_limit(next_month_start(T)), 2_903_226);
    }
}
