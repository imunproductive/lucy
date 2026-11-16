//! HeroSelect (`STU_0E913A0B`, `0C90` st1): while it runs, it keeps the
//! controller's hero limits, which "may this controller play this hero"
//! (`STU_9AE5FC4C`, [`Context::hero_available`]) tests. Client: update
//! 0x7FF789C204E0 evaluates (list, limit) pairs into state arrays, and
//! `sub_7FF789C21BC0` keeps only the members listed at least `limit` times
//! when `limit > 1`; the hero test `sub_7FF789CB0C70` then refuses a hero
//! whose role is in `m_3E447223` (#8916, limit `m_BB844B42` =
//! `max(1, #8917e)`: the role limit) or who is in `m_A519B7A6` (#10476,
//! limit `m_861FA5EE` #8432: the hero limit), and with a non-empty
//! `m_8B103ADE` (#10489, limit `m_52C54CB2`: the allow-list) allows only
//! its heroes (ControllerWorld.md, "Client tools pass 2 (2026-11-16)",
//! CW.3). `m_FD29A448` (allowed at once: it skips the client's per-team
//! rules, which the server doesn't have) and the UI flag `m_0FC04765`
//! (#8419) are not kept. Like FilterArray, it evaluates its lists again
//! when a variable they read changes.

use super::*;

/// One running HeroSelect's lists, each member at its limit.
#[derive(Clone, Debug, Default)]
pub(super) struct HeroLimits {
    /// Role identifiers at the role limit.
    roles: Vec<u64>,
    /// Heroes at the hero limit.
    heroes: Vec<u64>,
    /// The allow-list; empty allows every hero.
    allowed: Vec<u64>,
}

impl HeroLimits {
    /// Whether these limits refuse `hero` of role `role`; None: the role
    /// limit is in force and the hero's role isn't known.
    pub(super) fn refuses(&self, hero: u64, role: impl FnOnce() -> Option<u64>) -> Option<bool> {
        if !self.roles.is_empty() && self.roles.contains(&role()?) {
            return Some(true);
        }
        Some(
            self.heroes.contains(&hero)
                || (!self.allowed.is_empty() && !self.allowed.contains(&hero)),
        )
    }
}

/// The assets of `list` listed at least `limit` times, each once, in
/// first-seen order (`sub_7FF789C21BC0`: a limit of 1 or less keeps them
/// all).
fn at_limit(list: Value, limit: &Value) -> Vec<u64> {
    let mut rows = Vec::new();
    // A list nested too deep keeps nothing (as an unreadable list).
    if expr::value_rows(list, &mut rows).is_err() {
        return Vec::new();
    }
    let assets: Vec<u64> = rows
        .iter()
        .filter_map(|row| match row {
            Value::Asset(guid) if *guid != 0 => Some(*guid),
            _ => None,
        })
        .collect();
    let limit = usize::try_from(expr::to_int(limit, 0).max(1)).unwrap_or(1);
    let mut kept = Vec::new();
    for &asset in &assets {
        if !kept.contains(&asset) && assets.iter().filter(|&&a| a == asset).count() >= limit {
            kept.push(asset);
        }
    }
    kept
}

impl<W: World> Vm<'_, '_, '_, W> {
    /// Evaluate the running HeroSelect's lists again.
    pub(super) fn refresh_hero_select(&mut self, key: StateKey) {
        let id = key.instance;
        let node = self
            .graph(id)
            .state(usize::from(key.raw))
            .expect("HeroSelect state");
        let mut pair = |list: &str, limit: &str| {
            let list = self
                .evaluate_value(id, node, field(&node.fields, list), Some(key))
                .unwrap_or(Value::Null);
            let limit = self
                .evaluate(id, node, field(&node.fields, limit), Some(key))
                .unwrap_or(Value::Null);
            at_limit(list, &limit)
        };
        let limits = HeroLimits {
            roles: pair("m_3E447223", "m_BB844B42"),
            heroes: pair("m_A519B7A6", "m_861FA5EE"),
            allowed: pair("m_8B103ADE", "m_52C54CB2"),
        };
        self.machine.hero_selects.insert(key, limits);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const ANA: u64 = 0x02e0_0000_0000_013b;
    const SOLDIER: u64 = 0x02e0_0000_0000_006e;
    const DAMAGE: u64 = 0x0d80_0000_0000_084e;
    const SUPPORT: u64 = 0x0d80_0000_0000_0851;

    fn list(assets: &[u64]) -> Value {
        Value::Array(assets.iter().map(|&asset| Value::Asset(asset)).collect())
    }

    #[test]
    fn limits_keep_members_listed_limit_times() {
        let taken = list(&[ANA, SOLDIER, ANA]);
        assert_eq!(at_limit(taken.clone(), &Value::Integer(2)), [ANA]);
        assert_eq!(at_limit(taken.clone(), &Value::Float(1.0)), [ANA, SOLDIER]);
        assert_eq!(at_limit(taken, &Value::Integer(0)), [ANA, SOLDIER]);
        assert!(at_limit(Value::Null, &Value::Integer(1)).is_empty());
    }

    #[test]
    fn hero_limit_role_limit_and_allow_list_refuse() {
        let open = HeroLimits::default();
        assert_eq!(open.refuses(ANA, || None), Some(false));
        let hero_limit = HeroLimits {
            heroes: vec![ANA],
            ..HeroLimits::default()
        };
        assert_eq!(hero_limit.refuses(ANA, || None), Some(true));
        assert_eq!(hero_limit.refuses(SOLDIER, || None), Some(false));
        let role_limit = HeroLimits {
            roles: vec![SUPPORT],
            ..HeroLimits::default()
        };
        assert_eq!(role_limit.refuses(ANA, || Some(SUPPORT)), Some(true));
        assert_eq!(role_limit.refuses(SOLDIER, || Some(DAMAGE)), Some(false));
        assert_eq!(role_limit.refuses(SOLDIER, || None), None);
        let allow_list = HeroLimits {
            allowed: vec![SOLDIER],
            ..HeroLimits::default()
        };
        assert_eq!(allow_list.refuses(SOLDIER, || None), Some(false));
        assert_eq!(allow_list.refuses(ANA, || None), Some(true));
    }
}
