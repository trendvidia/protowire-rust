// SPDX-License-Identifier: MIT
// Copyright (c) 2026 TrendVidia, LLC.
//! What the post-decode pass needs to know of a message type before it looks
//! at the document, cached per type. The pass visits every list element and
//! map value, so without this it would build a path and look up presence for
//! every field of every element, annotated or not. Mirrors the reference's
//! `post_decode_plan.go`.

use std::collections::{HashMap, HashSet};
use std::sync::{Arc, LazyLock, Mutex};

use prost_reflect::{FieldDescriptor, Kind, MessageDescriptor};

use crate::annotations::field_annotations;
use crate::decode::is_wkt_skip_recursion;
use crate::schema::MAX_CACHED_DESCRIPTORS;

/// One field of a message, in `MessageDescriptor::fields` order.
#[derive(Debug)]
pub(crate) struct PlanField {
    /// The field's number; the plan is sorted by it.
    pub number: u32,
    pub required: bool,
    pub default: Option<String>,
    /// The field holds message bodies (a message, or a list or map of them)
    /// in which some field carries `(pxf.default)` or `(pxf.required)`.
    pub visit: bool,
}

type Plan = Arc<[PlanField]>;

/// Keyed by the address of the type's descriptor proto. A hit is confirmed by
/// descriptor equality, and the entry keeps that address alive. Bounded like
/// the schema caches, for the same reason (`MAX_CACHED_DESCRIPTORS`).
static PLANS: LazyLock<Mutex<HashMap<usize, (MessageDescriptor, Plan)>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

pub(crate) fn post_decode_plan(desc: &MessageDescriptor) -> Plan {
    let key = desc.descriptor_proto() as *const _ as usize;
    {
        let plans = PLANS.lock().unwrap_or_else(|e| e.into_inner());
        if let Some((cached, plan)) = plans.get(&key) {
            if cached == desc {
                return plan.clone();
            }
        }
    }
    let plan: Plan = desc
        .fields()
        .map(|fd| {
            let a = field_annotations(&fd);
            PlanField {
                number: fd.number(),
                required: a.required,
                default: a.default,
                visit: body_of(&fd)
                    .is_some_and(|body| has_default_or_required(&body, &mut HashSet::new())),
            }
        })
        .collect();
    let mut plans = PLANS.lock().unwrap_or_else(|e| e.into_inner());
    if plans.len() < MAX_CACHED_DESCRIPTORS {
        plans
            .entry(key)
            .or_insert_with(|| (desc.clone(), plan.clone()));
    }
    plan
}

/// The message type of the bodies `fd` holds, if it holds any.
fn body_of(fd: &FieldDescriptor) -> Option<MessageDescriptor> {
    match fd.kind() {
        Kind::Message(entry) if fd.is_map() => match entry.map_entry_value_field().kind() {
            Kind::Message(value) => Some(value),
            _ => None,
        },
        Kind::Message(m) => Some(m),
        _ => None,
    }
}

/// Whether a field of `desc`, or of a message reachable from it, carries
/// `(pxf.default)` or `(pxf.required)`. Unlike the reference's, this stops at
/// an `Any`: its body took the pass when it was decoded, under the Any's path,
/// and here it is only bytes. `seen` guards recursive types.
fn has_default_or_required(desc: &MessageDescriptor, seen: &mut HashSet<String>) -> bool {
    if is_wkt_skip_recursion(crate::wkt::dispatch_name(desc))
        || !seen.insert(desc.full_name().to_string())
    {
        return false;
    }
    desc.fields().any(|fd| {
        let a = field_annotations(&fd);
        a.required
            || a.default.is_some()
            || body_of(&fd).is_some_and(|body| has_default_or_required(&body, seen))
    })
}
