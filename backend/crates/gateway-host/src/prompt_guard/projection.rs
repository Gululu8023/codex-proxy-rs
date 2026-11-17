//! 确定性的复审投影；必选完整文本块不截断，可选历史按完整回合追加。

use super::{
    envelope::{Envelope, Origin, Segment},
    rules::Classification,
};
use serde_json::{Value, json};
use std::collections::{BTreeMap, BTreeSet};

pub struct Projection {
    pub payload: Value,
    pub selected_blocks: u32,
    pub omitted_blocks: u32,
    pub omitted_chars: u64,
}

pub fn moderation(envelope: &Envelope, model: &str) -> Projection {
    let texts: Vec<_> = envelope
        .segments
        .iter()
        .filter(|s| s.origin == Origin::CurrentUser && s.representation == "original")
        .map(|s| s.text.as_str())
        .collect();
    Projection {
        payload: json!({"model":model,"input":texts.join("\n")}),
        selected_blocks: texts.len() as u32,
        omitted_blocks: 0,
        omitted_chars: 0,
    }
}

pub fn build(
    envelope: &Envelope,
    classification: &Classification,
    model: &str,
    limit: usize,
) -> Result<Projection, &'static str> {
    let mut latest = BTreeMap::<usize, &Segment>::new();
    for segment in &envelope.segments {
        latest.insert(segment.root, segment);
    }
    let current_turn = latest
        .values()
        .filter(|segment| segment.origin == Origin::CurrentUser)
        .filter_map(|segment| segment.turn)
        .max();
    let evidence_turns: BTreeSet<_> = latest
        .values()
        .filter(|segment| classification.required_roots.contains(&segment.root))
        .filter_map(|segment| segment.turn)
        .collect();
    let mut selected: BTreeSet<_> = latest
        .values()
        .filter(|segment| {
            segment.origin.controls_request()
                || classification.required_roots.contains(&segment.root)
                || segment.turn.is_some_and(|turn| {
                    current_turn.is_some_and(|current| turn >= current)
                        || evidence_turns.contains(&turn)
                })
        })
        .map(|segment| segment.root)
        .collect();
    let payload = |selected: &BTreeSet<usize>| {
        let segments: Vec<_> = selected.iter().map(|index| latest[index]).collect();
        let material = json!({"scope":"current_turn_with_context", "coverage":classification.assessment.coverage,
            "required_categories":classification.categories, "segments":segments});
        json!({"model":model,"stream":false,"temperature":0,
            "messages":[{"role":"system","content":include_str!("review_system.txt")},
                {"role":"user","content":material.to_string()}]})
    };
    if payload(&selected).to_string().chars().count() > limit {
        return Err("prompt_guard_review_input_too_large");
    }
    let turns: BTreeSet<_> = latest.values().filter_map(|segment| segment.turn).collect();
    for turn in turns.into_iter().rev() {
        let additional: Vec<_> = latest
            .values()
            .filter(|segment| segment.turn == Some(turn) && !selected.contains(&segment.root))
            .map(|segment| segment.root)
            .collect();
        if additional.is_empty() {
            continue;
        }
        selected.extend(additional.iter().copied());
        if payload(&selected).to_string().chars().count() > limit {
            for root in additional {
                selected.remove(&root);
            }
            break;
        }
    }
    let omitted: Vec<_> = latest
        .values()
        .filter(|segment| !selected.contains(&segment.root))
        .collect();
    Ok(Projection {
        payload: payload(&selected),
        selected_blocks: selected.len() as u32,
        omitted_blocks: omitted.len() as u32,
        omitted_chars: omitted
            .iter()
            .map(|segment| segment.text.chars().count() as u64)
            .sum(),
    })
}
