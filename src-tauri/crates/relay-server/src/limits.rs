//! Limites en mémoire seulement (jamais sur disque) : quotas du jour par clé,
//! limites par réseau, nonces déjà vus. Fonctions pures de l'heure donnée,
//! essayées seules.
//!
//! **Réseaux en cascade** : une adresse compte pour son réseau étroit et pour
//! les réseaux plus larges qui le contiennent (IPv6 : /56, /48, /32 ; IPv4 :
//! /24, /16), chacun sous son plafond. Un /48 (gratuit chez un service de
//! tunnel IPv6) contient 256 /56 : sans le niveau /48, il valait 256 réseaux.
//!
//! **Tables pleines** : chaque table est bornée, et une table pleine ne refuse
//! personne d'autre que celui qui la remplit.
//! - Compteurs par réseau et par clé : deux générations ([`Counters`]). Quand
//!   la récente est pleine, la plus ancienne est oubliée : seuls les compteurs
//!   restés sans usage depuis une génération repartent de zéro (au plus du
//!   quota rendu, jamais un refus). Un attaquant qui frappe sans cesse garde
//!   ses propres compteurs dans la génération récente.
//! - Défis (non signés, donc le moins cher à envoyer) : table à part, avec sa
//!   propre borne ; pleine, elle ne refuse que les défis des réseaux nouveaux,
//!   jamais une recherche ni une inscription.
//! - Nonces : rangés par date d'expiration (ménage sans parcours de toute la
//!   table) ; pleine, la table oublie le nonce qui expire le plus tôt. La
//!   cadence par clé passe avant (une clé y met au plus quelques centaines
//!   d'entrées), et rejouer une requête dont le nonce a été oublié revient à
//!   en envoyer une nouvelle : mêmes quotas, même cadence.
//!
//! **Mémoire** ([`worst_case_bytes`], essayé) : aux bornes par défaut, toutes
//! les tables pleines en même temps tiennent sous 64 Mio. Avec les preuves de
//! travail vérifiées (64 Mio au plus, voir `server.rs`) et le reste du
//! processus (runtime, connexions, clients TLS, SQLite : quelques dizaines de
//! Mio), le relais reste sous le `MemoryHigh=192M` de son unité systemd, loin
//! du `MemoryMax=256M` (une mise à mort redémarrerait le relais et remettrait
//! les quotas du jour à zéro). Chaque table est vidée et sa mémoire rendue à
//! chaque changement de minute ou de jour.

use crate::config::{TablesConfig, Tiers};
use relay_proto::CLOCK_TOLERANCE_SECS;
use relay_proto::sign::PublicKey;
use relay_proto::time::{day_index, next_day_start};
use std::collections::{BTreeMap, HashMap};
use std::hash::Hash;
use std::net::IpAddr;

/// Niveaux de réseau, du plus étroit au plus large.
pub const LEVELS: usize = 3;

/// Préfixes d'un niveau : IPv4 (le troisième niveau n'existe pas en IPv4 : un
/// /8 couvrirait des opérateurs entiers), IPv6.
const V4_PREFIXES: [Option<u8>; LEVELS] = [Some(24), Some(16), None];
const V6_PREFIXES: [Option<u8>; LEVELS] = [Some(56), Some(48), Some(32)];

/// Réseau : adresse masquée et longueur du préfixe.
pub type Net = (IpAddr, u8);

fn masked(octets: &mut [u8], bits: u8) {
    let bits = usize::from(bits);
    for (index, byte) in octets.iter_mut().enumerate() {
        let start = index * 8;
        if start >= bits {
            *byte = 0;
        } else if start + 8 > bits {
            *byte &= 0xffu8 << (start + 8 - bits);
        }
    }
}

/// Réseaux d'une adresse, du plus étroit au plus large. Une IPv4 dans une
/// IPv6 est ramenée à l'IPv4.
pub fn networks_of(ip: IpAddr) -> [Option<Net>; LEVELS] {
    let ip = match ip {
        IpAddr::V6(v6) => v6.to_ipv4_mapped().map_or(ip, IpAddr::V4),
        v4 => v4,
    };
    let mut out = [None; LEVELS];
    for (level, slot) in out.iter_mut().enumerate() {
        *slot = match ip {
            IpAddr::V4(v4) => V4_PREFIXES[level].map(|bits| {
                let mut octets = v4.octets();
                masked(&mut octets, bits);
                (IpAddr::from(octets), bits)
            }),
            IpAddr::V6(v6) => V6_PREFIXES[level].map(|bits| {
                let mut octets = v6.octets();
                masked(&mut octets, bits);
                (IpAddr::from(octets), bits)
            }),
        };
    }
    out
}

/// Réseau le plus étroit : /24 en IPv4, /56 en IPv6.
pub fn network_of(ip: IpAddr) -> IpAddr {
    networks_of(ip)[0].map_or(ip, |(net, _)| net)
}

/// Compteurs bornés. `evict` : deux générations, la plus ancienne oubliée
/// quand la récente est pleine ; sinon une seule table, qui refuse les
/// nouvelles entrées une fois pleine.
pub struct Counters<K> {
    current: HashMap<K, u32>,
    previous: HashMap<K, u32>,
    /// Entrées au plus par génération.
    generation: usize,
    evict: bool,
}

impl<K: Hash + Eq + Copy> Counters<K> {
    /// `capacity` : entrées au plus en tout.
    pub fn new(capacity: usize, evict: bool) -> Self {
        Self {
            current: HashMap::new(),
            previous: HashMap::new(),
            generation: if evict {
                (capacity / 2).max(1)
            } else {
                capacity.max(1)
            },
            evict,
        }
    }

    pub fn get(&self, key: &K) -> u32 {
        self.current
            .get(key)
            .or_else(|| self.previous.get(key))
            .copied()
            .unwrap_or(0)
    }

    fn contains(&self, key: &K) -> bool {
        self.current.contains_key(key) || self.previous.contains_key(key)
    }

    /// Place pour `new` entrées de plus (toujours, avec l'oubli).
    fn has_room(&self, new: usize) -> bool {
        self.evict || self.current.len() + new <= self.generation
    }

    pub fn add(&mut self, key: K) {
        if let Some(count) = self.current.get_mut(&key) {
            *count = count.saturating_add(1);
            return;
        }
        let count = self.previous.remove(&key).unwrap_or(0);
        if self.current.len() >= self.generation {
            if !self.evict {
                return;
            }
            // La plus ancienne génération est oubliée (sa mémoire rendue).
            self.previous = std::mem::take(&mut self.current);
        }
        self.current.insert(key, count.saturating_add(1));
    }

    pub fn sub(&mut self, key: &K) {
        if let Some(count) = self
            .current
            .get_mut(key)
            .or_else(|| self.previous.get_mut(key))
        {
            *count = count.saturating_sub(1);
        }
    }

    /// Tout oublier, mémoire rendue (une table remplie un jour d'attaque ne
    /// garde pas sa taille les jours suivants).
    pub fn reset(&mut self) {
        self.current = HashMap::new();
        self.previous = HashMap::new();
    }

    pub fn len(&self) -> usize {
        self.current.len() + self.previous.len()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Places allouées (mémoire).
    pub fn allocated(&self) -> usize {
        self.current.capacity() + self.previous.capacity()
    }
}

/// Prend une unité à chaque niveau, ou rien si un niveau est à son plafond
/// (ou si la table refuse de nouvelles entrées). `take` faux : regarde
/// seulement.
fn cascade<K: Hash + Eq + Copy>(
    table: &mut Counters<K>,
    keys: [Option<K>; LEVELS],
    limits: &Tiers,
    take: bool,
) -> bool {
    let mut new = 0;
    for (key, limit) in keys.iter().zip(limits.0) {
        let Some(key) = key else { continue };
        if table.get(key) >= limit {
            return false;
        }
        if !table.contains(key) {
            new += 1;
        }
    }
    if !table.has_room(new) {
        return false;
    }
    if take {
        for key in keys.into_iter().flatten() {
            table.add(key);
        }
    }
    true
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum IpKind {
    Search,
    Register,
}

/// Compteurs par réseau : par minute (toutes requêtes), par jour (recherches
/// et inscriptions), et les défis à part.
pub struct IpLimits {
    minute: u64,
    per_minute: Counters<Net>,
    day: u64,
    daily: Counters<(Net, IpKind)>,
    challenges: Counters<Net>,
}

impl IpLimits {
    pub fn new(tables: &TablesConfig) -> Self {
        Self {
            minute: 0,
            per_minute: Counters::new(tables.network_minute, true),
            day: 0,
            daily: Counters::new(tables.network_daily, true),
            challenges: Counters::new(tables.challenges, false),
        }
    }

    fn roll(&mut self, now: u64) {
        if now / 60 != self.minute {
            self.minute = now / 60;
            self.per_minute.reset();
        }
        if day_index(now) != self.day {
            self.day = day_index(now);
            self.daily.reset();
            self.challenges.reset();
        }
    }

    /// Une requête de plus dans la minute ; `Err(retry_at)` au-delà.
    pub fn hit_minute(&mut self, ip: IpAddr, now: u64, limits: &Tiers) -> Result<(), u64> {
        self.roll(now);
        if cascade(&mut self.per_minute, networks_of(ip), limits, true) {
            Ok(())
        } else {
            Err((now / 60 + 1) * 60)
        }
    }

    fn daily_keys(ip: IpAddr, kind: IpKind) -> [Option<(Net, IpKind)>; LEVELS] {
        networks_of(ip).map(|net| net.map(|net| (net, kind)))
    }

    /// Prend une unité du jour ; `Err(retry_at)` au-delà.
    pub fn take_daily(
        &mut self,
        ip: IpAddr,
        kind: IpKind,
        now: u64,
        limits: &Tiers,
    ) -> Result<(), u64> {
        self.roll(now);
        if cascade(&mut self.daily, Self::daily_keys(ip, kind), limits, true) {
            Ok(())
        } else {
            Err(next_day_start(now))
        }
    }

    /// Reste-t-il une unité du jour (sans la prendre) ?
    pub fn can_take_daily(
        &mut self,
        ip: IpAddr,
        kind: IpKind,
        now: u64,
        limits: &Tiers,
    ) -> Result<(), u64> {
        self.roll(now);
        if cascade(&mut self.daily, Self::daily_keys(ip, kind), limits, false) {
            Ok(())
        } else {
            Err(next_day_start(now))
        }
    }

    /// Rend une unité (la recherche n'a pas abouti, l'inscription n'a pas pu
    /// être examinée).
    pub fn give_back_daily(&mut self, ip: IpAddr, kind: IpKind, now: u64) {
        self.roll(now);
        for key in Self::daily_keys(ip, kind).into_iter().flatten() {
            self.daily.sub(&key);
        }
    }

    /// Un défi de plus ce jour-là ; pleine, la table ne refuse que les défis.
    pub fn take_challenge(&mut self, ip: IpAddr, now: u64, limits: &Tiers) -> Result<(), u64> {
        self.roll(now);
        if cascade(&mut self.challenges, networks_of(ip), limits, true) {
            Ok(())
        } else {
            Err(next_day_start(now))
        }
    }

    /// Entrées de chaque table (minute, jour, défis) : essais.
    pub fn lens(&self) -> (usize, usize, usize) {
        (
            self.per_minute.len(),
            self.daily.len(),
            self.challenges.len(),
        )
    }

    /// Places allouées en tout : essais.
    pub fn allocated(&self) -> usize {
        self.per_minute.allocated() + self.daily.allocated() + self.challenges.allocated()
    }
}

/// Quotas du jour par clé, et cadence par minute.
pub struct KeyQuotas {
    day: u64,
    used: Counters<PublicKey>,
    minute: u64,
    per_minute: Counters<PublicKey>,
}

impl KeyQuotas {
    pub fn new(tables: &TablesConfig) -> Self {
        Self {
            day: 0,
            used: Counters::new(tables.keys, true),
            minute: 0,
            per_minute: Counters::new(tables.keys, true),
        }
    }

    fn roll(&mut self, now: u64) {
        if day_index(now) != self.day {
            self.day = day_index(now);
            self.used.reset();
        }
        if now / 60 != self.minute {
            self.minute = now / 60;
            self.per_minute.reset();
        }
    }

    pub fn used(&mut self, key: &PublicKey, now: u64) -> u32 {
        self.roll(now);
        self.used.get(key)
    }

    /// Réserve une recherche ; `Err(retry_at)` si le quota est atteint.
    pub fn reserve(&mut self, key: &PublicKey, now: u64, limit: u32) -> Result<(), u64> {
        self.roll(now);
        if self.used.get(key) >= limit {
            return Err(next_day_start(now));
        }
        self.used.add(*key);
        Ok(())
    }

    /// Rend la recherche réservée (aucun fournisseur n'a répondu).
    pub fn release(&mut self, key: &PublicKey, now: u64) {
        self.roll(now);
        self.used.sub(key);
    }

    pub fn hit_minute(&mut self, key: &PublicKey, now: u64, limit: u32) -> Result<(), u64> {
        self.roll(now);
        if self.per_minute.get(key) >= limit {
            return Err((now / 60 + 1) * 60);
        }
        self.per_minute.add(*key);
        Ok(())
    }
}

type Seen = (PublicKey, [u8; 16]);

/// Nonces déjà vus, gardés tant qu'une requête qui les porte serait encore
/// acceptée (son heure + la tolérance), rangés par expiration.
pub struct ReplayCache {
    seen: HashMap<Seen, u64>,
    expiry: BTreeMap<u64, Vec<Seen>>,
    capacity: usize,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Replay {
    Fresh,
    Seen,
}

impl ReplayCache {
    pub fn new(capacity: usize) -> Self {
        Self {
            seen: HashMap::new(),
            expiry: BTreeMap::new(),
            capacity: capacity.max(1),
        }
    }

    /// Oublie les nonces expirés : seulement ceux-là, du plus ancien au plus
    /// récent (jamais un parcours de toute la table).
    fn prune(&mut self, now: u64) {
        while let Some(entry) = self.expiry.first_entry()
            && *entry.key() < now
        {
            for seen in entry.remove() {
                self.seen.remove(&seen);
            }
        }
        if self.seen.is_empty() && self.seen.capacity() > 0 {
            self.seen = HashMap::new();
        }
    }

    /// Table pleine : oublie le nonce qui expire le plus tôt.
    fn evict_one(&mut self) {
        if let Some(mut entry) = self.expiry.first_entry() {
            if let Some(seen) = entry.get_mut().pop() {
                self.seen.remove(&seen);
            }
            if entry.get().is_empty() {
                entry.remove();
            }
        }
    }

    pub fn check(&mut self, key: &PublicKey, nonce: [u8; 16], time: u64, now: u64) -> Replay {
        self.prune(now);
        let entry = (*key, nonce);
        if self.seen.contains_key(&entry) {
            return Replay::Seen;
        }
        while self.seen.len() >= self.capacity {
            self.evict_one();
        }
        let until = time + CLOCK_TOLERANCE_SECS;
        self.seen.insert(entry, until);
        self.expiry.entry(until).or_default().push(entry);
        Replay::Fresh
    }

    pub fn len(&self) -> usize {
        self.seen.len()
    }

    pub fn is_empty(&self) -> bool {
        self.seen.is_empty()
    }
}

/// Octets d'une table de hachage de `entries` éléments au plus (hashbrown :
/// puissance de deux de cases, 7/8 remplies au plus, un octet de contrôle par
/// case).
fn hash_table_bytes<K, V>(entries: usize) -> usize {
    let buckets = (entries.saturating_mul(8).div_ceil(7)).next_power_of_two();
    buckets * (std::mem::size_of::<(K, V)>() + 1) + 16
}

/// Mémoire des tables toutes pleines en même temps (pire cas).
pub fn worst_case_bytes(tables: &TablesConfig) -> usize {
    let generations = |capacity: usize| (capacity / 2).max(1);
    let per_minute = 2 * hash_table_bytes::<Net, u32>(generations(tables.network_minute));
    let daily = 2 * hash_table_bytes::<(Net, IpKind), u32>(generations(tables.network_daily));
    let challenges = hash_table_bytes::<Net, u32>(tables.challenges);
    let keys = 2 * 2 * hash_table_bytes::<PublicKey, u32>(generations(tables.keys));
    // Index des expirations : chaque nonce une fois dans un vecteur (deux
    // fois sa place au pire, par la croissance des vecteurs), et un vecteur
    // par seconde d'expiration possible (10 min et demie).
    let replay = hash_table_bytes::<Seen, u64>(tables.replay)
        + 2 * tables.replay * std::mem::size_of::<Seen>()
        + (2 * CLOCK_TOLERANCE_SECS as usize + 61) * 128;
    per_minute + daily + challenges + keys + replay
}

#[cfg(test)]
mod tests {
    use super::*;

    const T: u64 = 1_794_139_200; // 8 nov. 2026, 12 h UTC.

    fn ip(text: &str) -> IpAddr {
        text.parse().unwrap()
    }

    fn tiers(a: u32, b: u32, c: u32) -> Tiers {
        Tiers([a, b, c])
    }

    fn tables() -> TablesConfig {
        TablesConfig::default()
    }

    #[test]
    fn networks() {
        assert_eq!(network_of(ip("203.0.113.77")), ip("203.0.113.0"));
        assert_eq!(network_of(ip("::ffff:203.0.113.77")), ip("203.0.113.0"));
        assert_eq!(
            network_of(ip("2001:db8:aa:bbcc:1:2:3:4")),
            ip("2001:db8:aa:bb00::")
        );
        assert_eq!(
            networks_of(ip("2001:db8:aa:bbcc::1")),
            [
                Some((ip("2001:db8:aa:bb00::"), 56)),
                Some((ip("2001:db8:aa::"), 48)),
                Some((ip("2001:db8::"), 32)),
            ]
        );
        assert_eq!(
            networks_of(ip("::ffff:198.51.100.7")),
            [
                Some((ip("198.51.100.0"), 24)),
                Some((ip("198.51.0.0"), 16)),
                None
            ]
        );
    }

    #[test]
    fn daily_limits_per_network_and_kind() {
        let mut limits = IpLimits::new(&tables());
        let three = tiers(3, 100, 100);
        for _ in 0..3 {
            limits
                .take_daily(ip("198.51.100.1"), IpKind::Register, T, &three)
                .unwrap();
        }
        // Même /24, autre machine : refusé jusqu'à minuit.
        assert_eq!(
            limits.take_daily(ip("198.51.100.200"), IpKind::Register, T, &three),
            Err(next_day_start(T))
        );
        assert_eq!(
            limits.can_take_daily(ip("198.51.100.200"), IpKind::Register, T, &three),
            Err(next_day_start(T))
        );
        // Autre sorte, autre réseau : libres.
        limits
            .take_daily(ip("198.51.100.1"), IpKind::Search, T, &three)
            .unwrap();
        limits
            .take_daily(ip("198.51.101.1"), IpKind::Register, T, &three)
            .unwrap();
        // Rendu, puis le lendemain.
        limits.give_back_daily(ip("198.51.100.1"), IpKind::Register, T);
        limits
            .take_daily(ip("198.51.100.1"), IpKind::Register, T, &three)
            .unwrap();
        limits
            .take_daily(ip("198.51.100.1"), IpKind::Register, T + 86_400, &three)
            .unwrap();
    }

    /// I1 : 256 /56 d'un même /48 ne valent plus 256 réseaux ; au-dessus, le
    /// /32 borne encore une plage de /48.
    #[test]
    fn wider_networks_cap_their_subnetworks() {
        let mut limits = IpLimits::new(&tables());
        let cascade = tiers(3, 10, 50);
        let mut accepted = 0;
        for n in 0..256u32 {
            let addr = ip(&format!("2001:db8:0:{:x}00::1", n));
            if limits
                .take_daily(addr, IpKind::Register, T, &cascade)
                .is_ok()
            {
                accepted += 1;
            }
        }
        assert_eq!(accepted, 10, "un /48");
        // Un /48 voisin a les siens, jusqu'au plafond du /32.
        let mut accepted = 10;
        for block in 1..100u32 {
            for n in 0..20u32 {
                let addr = ip(&format!("2001:db8:{block:x}:{n:x}00::1"));
                if limits
                    .take_daily(addr, IpKind::Register, T, &cascade)
                    .is_ok()
                {
                    accepted += 1;
                }
            }
        }
        assert_eq!(accepted, 50, "un /32");
        // Un autre /32 n'en souffre pas.
        limits
            .take_daily(ip("2001:db9::1"), IpKind::Register, T, &cascade)
            .unwrap();
        // IPv4 : /24, puis /16.
        let mut accepted = 0;
        for n in 0..=255u32 {
            if limits
                .take_daily(ip(&format!("198.51.{n}.1")), IpKind::Register, T, &cascade)
                .is_ok()
            {
                accepted += 1;
            }
        }
        assert_eq!(accepted, 10, "un /16");
        // Un refus d'un niveau large ne prend rien aux niveaux étroits.
        assert_eq!(
            limits
                .daily
                .get(&((ip("198.51.200.0"), 24), IpKind::Register)),
            0
        );
    }

    #[test]
    fn per_minute() {
        let mut limits = IpLimits::new(&tables());
        let two = tiers(2, 100, 100);
        limits.hit_minute(ip("192.0.2.1"), T, &two).unwrap();
        limits.hit_minute(ip("192.0.2.2"), T, &two).unwrap();
        assert_eq!(limits.hit_minute(ip("192.0.2.3"), T + 1, &two), Err(T + 60));
        limits.hit_minute(ip("192.0.2.3"), T + 60, &two).unwrap();
    }

    /// I3 : une table de réseaux pleine oublie les compteurs les moins
    /// récents au lieu de refuser tous les réseaux nouveaux ; les défis, à
    /// part, ne refusent que des défis.
    #[test]
    fn full_tables_do_not_refuse_everyone() {
        let small = TablesConfig {
            network_minute: 8,
            network_daily: 8,
            challenges: 6,
            keys: 4,
            replay: 4,
        };
        let mut limits = IpLimits::new(&small);
        let wide = tiers(1_000, 1_000, 1_000);
        // Un attaquant remplit tout, depuis des réseaux distincts.
        for n in 0..200u32 {
            let addr = ip(&format!("2001:db8:{n:x}::1"));
            let _ = limits.take_challenge(addr, T, &wide);
            let _ = limits.take_daily(addr, IpKind::Search, T, &wide);
            let _ = limits.hit_minute(addr, T, &wide);
        }
        let (minute, daily, challenges) = limits.lens();
        assert!(minute <= 8 && daily <= 8 && challenges <= 6);
        // Un réseau nouveau cherche toujours…
        let user = ip("198.51.100.7");
        limits.hit_minute(user, T, &wide).unwrap();
        limits.take_daily(user, IpKind::Search, T, &wide).unwrap();
        limits.take_daily(user, IpKind::Register, T, &wide).unwrap();
        // … seul son défi attend le lendemain.
        assert_eq!(
            limits.take_challenge(user, T, &wide),
            Err(next_day_start(T))
        );
        // Les compteurs d'un réseau actif survivent au remplissage.
        let busy = ip("192.0.2.1");
        let one = tiers(3, 1_000, 1_000);
        for n in 0..3u32 {
            limits.take_daily(busy, IpKind::Search, T, &one).unwrap();
            let addr = ip(&format!("2001:db8:ffff:{n:x}::1"));
            let _ = limits.take_daily(addr, IpKind::Search, T, &wide);
        }
        assert_eq!(
            limits.take_daily(busy, IpKind::Search, T, &one),
            Err(next_day_start(T))
        );
        // Le lendemain, tout est rendu (mémoire comprise).
        assert!(limits.allocated() > 0);
        limits.hit_minute(user, next_day_start(T), &wide).unwrap();
        limits
            .take_challenge(user, next_day_start(T), &wide)
            .unwrap();
        let _ = limits.take_daily(user, IpKind::Search, next_day_start(T), &wide);
        assert_eq!(limits.lens(), (2, 2, 2));
        assert!(limits.allocated() < 64, "{}", limits.allocated());
    }

    /// I3 : la remise à zéro (minute, jour) rend la mémoire d'une table
    /// remplie (`clear` garderait sa capacité).
    #[test]
    fn a_reset_gives_the_memory_back() {
        let mut counters = Counters::new(10_000, true);
        for n in 0..4_000u32 {
            counters.add(n);
        }
        assert!(counters.allocated() >= 4_000);
        counters.reset();
        assert_eq!(counters.allocated(), 0);
        let mut limits = IpLimits::new(&tables());
        let wide = tiers(1_000, 1_000, 1_000);
        for n in 0..2_000u32 {
            // Un /32 chacune : aucun plafond atteint.
            let addr = ip(&format!("2001:{:x}::1", n + 1));
            limits.take_daily(addr, IpKind::Search, T, &wide).unwrap();
        }
        assert!(limits.allocated() >= 2_000);
        let _ = limits.take_daily(ip("192.0.2.1"), IpKind::Search, next_day_start(T), &wide);
        assert!(limits.allocated() < 64, "{}", limits.allocated());
    }

    #[test]
    fn key_quota_resets_at_midnight_and_can_be_released() {
        let key = PublicKey([1; 32]);
        let mut quotas = KeyQuotas::new(&tables());
        for _ in 0..50 {
            quotas.reserve(&key, T, 50).unwrap();
        }
        assert_eq!(quotas.reserve(&key, T, 50), Err(next_day_start(T)));
        quotas.release(&key, T);
        assert_eq!(quotas.used(&key, T), 49);
        quotas.reserve(&key, T, 50).unwrap();
        assert_eq!(quotas.used(&key, next_day_start(T)), 0);
        quotas.reserve(&key, next_day_start(T), 50).unwrap();
    }

    #[test]
    fn key_tables_full_still_serve_new_keys() {
        let small = TablesConfig {
            keys: 4,
            ..tables()
        };
        let mut quotas = KeyQuotas::new(&small);
        for n in 0..100u8 {
            quotas.reserve(&PublicKey([n; 32]), T, 1).unwrap();
            quotas.hit_minute(&PublicKey([n; 32]), T, 1).unwrap();
        }
        assert!(quotas.used.len() <= 4 && quotas.per_minute.len() <= 4);
        // Une clé active garde son compte.
        let active = PublicKey([200; 32]);
        quotas.reserve(&active, T, 1).unwrap();
        quotas.reserve(&PublicKey([201; 32]), T, 1).unwrap();
        assert_eq!(quotas.reserve(&active, T, 1), Err(next_day_start(T)));
    }

    #[test]
    fn replays_are_seen_until_they_would_expire() {
        let key = PublicKey([1; 32]);
        let mut cache = ReplayCache::new(1_000);
        assert_eq!(cache.check(&key, [1; 16], T, T), Replay::Fresh);
        assert_eq!(cache.check(&key, [1; 16], T, T + 10), Replay::Seen);
        assert_eq!(
            cache.check(&PublicKey([2; 32]), [1; 16], T, T),
            Replay::Fresh
        );
        // Toujours vu à la limite de la tolérance, oublié après.
        assert_eq!(
            cache.check(&key, [1; 16], T, T + CLOCK_TOLERANCE_SECS),
            Replay::Seen
        );
        assert_eq!(
            cache.check(&key, [9; 16], T, T + CLOCK_TOLERANCE_SECS + 1),
            Replay::Fresh
        );
        assert_eq!(cache.len(), 1);
        assert_eq!(cache.expiry.len(), 1);
    }

    /// I2 : pleine, la table oublie le nonce qui expire le plus tôt ; une autre
    /// clé passe toujours, et un nonce récent reste vu.
    #[test]
    fn a_full_replay_table_evicts_the_oldest_nonce() {
        let (a, b) = (PublicKey([1; 32]), PublicKey([2; 32]));
        let mut cache = ReplayCache::new(3);
        for n in 0..3u8 {
            assert_eq!(cache.check(&a, [n; 16], T + u64::from(n), T), Replay::Fresh);
        }
        assert_eq!(cache.check(&b, [9; 16], T + 10, T), Replay::Fresh);
        assert_eq!(cache.len(), 3);
        // Le plus ancien (heure T) a été oublié, les autres restent vus.
        assert_eq!(cache.check(&a, [2; 16], T + 2, T), Replay::Seen);
        assert_eq!(cache.check(&b, [9; 16], T + 10, T), Replay::Seen);
        let sum: usize = cache.expiry.values().map(Vec::len).sum();
        assert_eq!(sum, cache.len());
    }

    /// Aux bornes par défaut, toutes les tables pleines tiennent sous 64 Mio
    /// (calcul détaillé en tête du module).
    #[test]
    fn default_tables_fit_the_memory_budget() {
        let bytes = worst_case_bytes(&tables());
        assert!(bytes <= 64 << 20, "{} Mio", bytes >> 20);
        // Les bornes par défaut remplissent exactement leurs tables (7/8
        // d'une puissance de deux par génération) : aucune case perdue.
        for (capacity, evict) in [
            (tables().network_minute, true),
            (tables().network_daily, true),
            (tables().challenges, false),
            (tables().keys, true),
            (tables().replay, false),
        ] {
            let generation = if evict { capacity / 2 } else { capacity };
            assert_eq!(generation * 8 % 7, 0, "{capacity}");
            assert!((generation * 8 / 7).is_power_of_two(), "{capacity}");
        }
    }

    /// Mesure (hors des essais ordinaires) : toutes les tables remplies à leur
    /// borne par défaut, mémoire résidente du processus avant et après.
    /// `cargo test -p relay-server --lib measure_tables -- --ignored --nocapture`.
    #[test]
    #[ignore]
    #[cfg(target_os = "linux")]
    fn measure_tables() {
        fn rss_kib() -> usize {
            let status = std::fs::read_to_string("/proc/self/status").unwrap();
            status
                .lines()
                .find_map(|line| line.strip_prefix("VmRSS:"))
                .and_then(|value| value.trim().trim_end_matches("kB").trim().parse().ok())
                .unwrap()
        }
        let defaults = tables();
        let before = rss_kib();
        let mut limits = IpLimits::new(&defaults);
        let mut quotas = KeyQuotas::new(&defaults);
        let mut replay = ReplayCache::new(defaults.replay);
        let huge = tiers(u32::MAX, u32::MAX, u32::MAX);
        let address =
            |n: u32| IpAddr::from([0x2001, 0xdb8, (n >> 16) as u16, n as u16, 0, 0, 0, 1]);
        for n in 0..(defaults.network_daily as u32) {
            let _ = limits.take_daily(address(n), IpKind::Search, T, &huge);
            let _ = limits.hit_minute(address(n), T, &huge);
            let _ = limits.take_challenge(address(n), T, &huge);
        }
        for n in 0..(defaults.keys as u32) {
            let mut bytes = [0u8; 32];
            bytes[..4].copy_from_slice(&n.to_le_bytes());
            let key = PublicKey(bytes);
            let _ = quotas.reserve(&key, T, u32::MAX);
            let _ = quotas.hit_minute(&key, T, u32::MAX);
        }
        for n in 0..(defaults.replay as u64) {
            let mut nonce = [0u8; 16];
            nonce[..8].copy_from_slice(&n.to_le_bytes());
            replay.check(&PublicKey([1; 32]), nonce, T + n % 600, T);
        }
        let after = rss_kib();
        println!(
            "tables pleines : {} Mio mesurés, {} Mio calculés",
            (after - before) / 1024,
            worst_case_bytes(&defaults) >> 20
        );
        assert!((after - before) * 1024 <= worst_case_bytes(&defaults) + (8 << 20));
    }
}
