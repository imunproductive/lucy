//! FilterArray (`STU_0A6BE10E`): while it runs, its output (`m_F2805C84`)
//! holds the values of its source (`m_814F8EC3`, as an assignment reads it,
//! arrays opened) that pass its filter (`m_filter`), in order. It filters
//! again when a variable it read changes and on every update, and writes the
//! output only when the result changed. Client function 1B9E450
//! (leafall903/ORPHEUS (MIT), `FilterArray` in `nodes.py`: "collects
//! candidates before queueing a changed array"; upstream queues the write
//! at now, here it is written at once).
//!
//! The client's refilter (0x7FF78AAAE450, from begin 0x7FF78AAA7870 and
//! update 0x7FF78AAA7940) tests an entity candidate with
//! `ScriptHost_EntityPassesFilter` (host slot `+0x150`): the filter's team
//! and kind masks against the entity's filter word, its relation words,
//! its tag lists and its condition ([`expr::passes_filter_as_candidate`]).
//! The tag test (0x7FF789C1EC70 → 0x7FF789CC2FD0) keeps an entity carrying
//! any tag of the list, and every entity when the list is empty: its
//! builder (0x7FF789CC2C10) leaves out a null tag, as `0F4A` rid280's
//! `#20031.id(10550)` is while the objective is unset. Any other candidate
//! meets only the condition (the candidate is every parameter, as
//! CycleIndex's condition reads it: the authored conditions read it as
//! parameter 8 or 142). The capture points' `0E8E` splits the bodies by
//! team so (st2, st3: masks 0x800000 and 0x1000000).
//!
//! The byte `m_C485751C` (+0x110): the end (0x7FF78AAA78A0) removes the
//! output variable when it is set (`sub_7FF78AAB56C0`, `sub_7FF78AAB5C40`),
//! as every FilterArray of `0E8E` has it: the point's lists empty when the
//! entry that made them ends. The update refilters while the state is not
//! ending (+448) and the instance's host has a world (`+504`); the server
//! refilters on every update [assumption: `+504` is always set on a server
//! host]. Other forms (`m_17A88AEF`, the change plug `m_A70DB860`) stay
//! inert with an assumption ([`ASSUMED_INERT_STATES`](super::nodes)).

use super::*;

/// A filter's flag words.
const FLAG_WORDS: &[&str] = &[
    "m_84065FD1",
    "m_1E14CCA8",
    "m_4EA686DB",
    "m_27CF71D5",
    "m_66747E7D",
    "m_89831602",
];

/// Whether the server runs this FilterArray (see the module comment).
pub(super) fn is_runnable_filter_array(node: &Node) -> bool {
    let filter = field(&node.fields, "m_filter");
    let filter_runnable = filter.is_null()
        || (field(filter, "$").as_str() == Some("STUConfigVarFilter")
            && filter.as_object().is_some_and(|fields| {
                fields.iter().all(|(key, value)| match key.as_str() {
                    "$" | "m_condition" | "m_9C7BD8B8" => true,
                    key if FLAG_WORDS.contains(&key) => value.as_u64().is_some(),
                    "m_EE729DCB" => value.as_u64() == Some(0),
                    _ => value.is_null() || value.as_array().is_some_and(Vec::is_empty),
                })
            }));
    filter_runnable
        && !field(&node.fields, "m_814F8EC3").is_null()
        && !field(&node.fields, "m_F2805C84").is_null()
        && field(&node.fields, "m_17A88AEF").is_null()
        && node.links("m_A70DB860").next().is_none()
}

/// The any-of team mask that names every team: the client then asks
/// nothing of the entity's teams (`ScriptHost_EntityPassesFilter`).
const ANY_TEAM: u64 = 0x3FFF_FFFF;

/// Whether a filter tests nothing but its condition: no team, relation or
/// tag rule, and an any-of team mask of none or [`ANY_TEAM`].
pub(super) fn condition_only(filter: &Json) -> bool {
    filter.as_object().is_some_and(|fields| {
        fields.iter().all(|(key, value)| match key.as_str() {
            "$" | "m_condition" => true,
            "m_84065FD1" => matches!(value.as_u64(), Some(0 | ANY_TEAM)),
            key if FLAG_WORDS.contains(&key) || key == "m_EE729DCB" => value.as_u64() == Some(0),
            _ => value.is_null() || value.as_array().is_some_and(Vec::is_empty),
        })
    })
}

/// Whether a filter's flag words test anything (all zero: no mask).
fn has_masks(filter: &Json) -> bool {
    FLAG_WORDS
        .iter()
        .any(|word| field(filter, word).as_u64().is_some_and(|value| value != 0))
}

impl<W: World + ?Sized> Vm<'_, '_, '_, W> {
    /// The begin filters and puts the state on the update list.
    pub(super) fn begin_filter_array(&mut self, key: StateKey) -> Result<(), InitFault> {
        self.refresh_filter_array(key)?;
        self.register_tick(key, self.machine.now_ms);
        Ok(())
    }

    /// The end removes the output when `m_C485751C` is set.
    pub(super) fn end_filter_array(&mut self, key: StateKey, node: &Node) -> Result<(), InitFault> {
        if field(&node.fields, "m_C485751C").as_u64().unwrap_or(0) == 0 {
            return Ok(());
        }
        match expr::lvalue(field(&node.fields, "m_F2805C84")) {
            Ok(Some(variable)) => {
                let output = source_key(key.instance, variable);
                if self.machine.value(output, None) != Value::Null {
                    self.write(output, Value::Null)?;
                }
            }
            Ok(None) => {}
            Err(error) => self.report(self.site(key.instance, node), Issue::Expression(error)),
        }
        Ok(())
    }

    /// Filter the source again and write the output if it changed.
    pub(super) fn refresh_filter_array(&mut self, key: StateKey) -> Result<(), InitFault> {
        let id = key.instance;
        let node = self
            .graph(id)
            .state(usize::from(key.raw))
            .expect("FilterArray state");
        let site = self.site(id, node);
        let output = match expr::lvalue(field(&node.fields, "m_F2805C84")) {
            Ok(Some(variable)) => source_key(id, variable),
            Ok(None) => return Ok(()),
            Err(error) => {
                self.report(site, Issue::Expression(error));
                return Ok(());
            }
        };
        let Some(source) =
            self.evaluate_value(id, node, field(&node.fields, "m_814F8EC3"), Some(key))
        else {
            return Ok(());
        };
        let mut candidates = Vec::new();
        if let Err(error) = expr::value_rows(source, &mut candidates) {
            self.report(site, Issue::Expression(error));
            return Ok(());
        }
        let filter = field(&node.fields, "m_filter");
        let condition = field(filter, "m_condition");
        let tag = field(filter, "m_9C7BD8B8");
        let masks = has_masks(filter).then_some(filter);
        let mut kept = Vec::new();
        for candidate in candidates {
            self.spend("array filter candidate")?;
            let outer = self.candidate.replace(candidate.clone());
            let accepted = self.filter_accepts(id, node, key, masks, condition, tag, &candidate);
            self.candidate = outer;
            match accepted {
                Some(true) => kept.push(candidate),
                Some(false) => {}
                None => return Ok(()),
            }
        }
        let kept = Value::Array(kept);
        if self.machine.value(output, None) != kept {
            self.write(output, kept)?;
        }
        Ok(())
    }

    /// Whether `candidate` passes; None: reported as unsupported.
    #[allow(clippy::too_many_arguments)]
    fn filter_accepts(
        &mut self,
        id: u16,
        node: &Node,
        key: StateKey,
        masks: Option<&Json>,
        condition: &Json,
        tag: &Json,
        candidate: &Value,
    ) -> Option<bool> {
        if let (Some(filter), Value::Entity(entity)) = (masks, candidate)
            && !self.evaluate_with(id, node, |context| {
                expr::masks_pass(filter, *entity, context)
            })?
        {
            return Some(false);
        }
        if !tag.is_null()
            && let Value::Entity(entity) = *candidate
        {
            let site = self.site(id, node);
            match self.evaluate(id, node, tag, Some(key))? {
                Value::Null => {}
                Value::Asset(tag) if tag >> 48 == crate::graph::TARGET_TAG_TYPE => {
                    match self.world.has_target_tag(site, entity, tag) {
                        Some(false) => return Some(false),
                        Some(true) => {}
                        None => {
                            self.report(site, Issue::UnsupportedField("array filter target tag"));
                            return None;
                        }
                    }
                }
                _ => {
                    self.report(site, Issue::UnsupportedField("array filter target tag"));
                    return None;
                }
            }
        }
        if condition.is_null() {
            return Some(true);
        }
        let value = self.evaluate(id, node, condition, Some(key))?;
        Some(truthy(&value))
    }
}
