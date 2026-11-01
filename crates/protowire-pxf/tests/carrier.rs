// SPDX-License-Identifier: MIT
// Copyright (c) 2026 TrendVidia, LLC.
//! The `@required` / `@default(v)` spelling, read from the 1327 carrier
//! (RFC-001 §8.1; protowire-rust#65). The fixtures under `testdata/carrier/`
//! come from the reference compiler; see the README there. Each runs in the
//! two shapes a reader meets: 1327 known to the pool, and unknown to it.

use prost_reflect::{DescriptorPool, MessageDescriptor, ReflectMessage, Value};
use protowire_pxf::annotations::{get_default, is_required};
use protowire_pxf::{
    marshal, unmarshal_full, validate_descriptor, MarshalOptions, UnmarshalOptions, ViolationKind,
};

macro_rules! fixture {
    ($name:literal) => {
        [
            include_bytes!(concat!("../testdata/carrier/", $name, ".binpb")).as_slice(),
            include_bytes!(concat!("../testdata/carrier/", $name, ".bare.binpb")).as_slice(),
        ]
    };
}

fn desc(fds: &[u8], name: &str) -> MessageDescriptor {
    DescriptorPool::decode(fds)
        .expect("decode the fixture")
        .get_message_by_name(name)
        .unwrap_or_else(|| panic!("no {name}"))
}

/// The two shapes really do differ: the test would prove nothing of the
/// unknown-field path if both resolved 1327.
#[test]
fn the_two_fixture_shapes_differ_in_whether_1327_is_known() {
    let [full, bare] = fixture!("carrier");
    let known = |fds: &[u8]| {
        let d = desc(fds, "carrier.v1.Req");
        let f = d.get_field_by_name("name").unwrap();
        let o = f.options();
        let known = o.extensions().any(|(e, _)| e.number() == 1327);
        let unknown = o.unknown_fields().any(|u| u.number() == 1327);
        (known, unknown)
    };
    assert_eq!(known(full), (true, false));
    assert_eq!(known(bare), (false, true));
}

#[test]
fn at_required_is_enforced() {
    for fds in fixture!("carrier") {
        let d = desc(fds, "carrier.v1.Req");
        assert!(is_required(&d.get_field_by_name("name").unwrap()));
        assert!(!is_required(&d.get_field_by_name("n").unwrap()));
        let o = UnmarshalOptions::default();
        let e = unmarshal_full("n = 1\n", &d, o).unwrap_err();
        assert_eq!(e.msg, "required field \"name\" is absent");
        unmarshal_full("name = \"x\"\n", &d, o).unwrap();
    }
}

#[test]
fn at_default_is_applied_for_every_argument_type() {
    for fds in fixture!("carrier") {
        let d = desc(fds, "carrier.v1.Def");
        let (m, _) = unmarshal_full("", &d, UnmarshalOptions::default()).unwrap();
        let get = |n: &str| m.get_field_by_name(n).unwrap().into_owned();
        assert_eq!(get("s"), Value::String("x".into()));
        assert_eq!(get("named"), Value::String("y".into()));
        assert_eq!(get("n"), Value::I32(-5));
        assert_eq!(get("b"), Value::Bool(true));
        assert_eq!(get("d"), Value::F64(1.5));
        assert_eq!(get("u"), Value::U64(u64::MAX));
        assert_eq!(get("c"), Value::EnumNumber(1));
        let text = marshal(&m, &d, MarshalOptions::default()).unwrap();
        for want in ["big = 123", "dec = 1.25", "flt = 1.5p"] {
            assert!(text.contains(want), "{want} in {text}");
        }
    }
}

#[test]
fn a_supplied_value_beats_at_default() {
    for fds in fixture!("carrier") {
        let d = desc(fds, "carrier.v1.Def");
        let (m, _) = unmarshal_full("s = \"mine\"\n", &d, UnmarshalOptions::default()).unwrap();
        let s = m.get_field_by_name("s").unwrap();
        assert_eq!(s.as_str(), Some("mine"));
        assert_eq!(m.descriptor().full_name(), "carrier.v1.Def");
    }
}

#[test]
fn at_required_with_at_default_is_a_bind_time_violation() {
    for fds in fixture!("required_default") {
        let d = desc(fds, "carrier.required_default.v1.M");
        let vs = validate_descriptor(&d);
        assert_eq!(vs.len(), 1, "{vs:?}");
        assert_eq!(vs[0].kind, ViolationKind::RequiredOption);
        assert_eq!(vs[0].name, "x");
        let text = vs[0].to_string();
        assert!(text.contains("invalid @required"), "{text}");
        assert!(
            text.contains("@required and @default are exclusive"),
            "{text}"
        );
        assert!(!text.contains("(pxf."), "quotes the form written: {text}");
        let e = unmarshal_full("", &d, UnmarshalOptions::default()).unwrap_err();
        assert!(e.msg.contains("exclusive"), "{}", e.msg);
    }
}

#[test]
fn at_required_on_a_oneof_member_is_a_bind_time_violation() {
    for fds in fixture!("oneof_required") {
        let vs = validate_descriptor(&desc(fds, "carrier.oneof_required.v1.M"));
        assert_eq!(vs.len(), 1, "{vs:?}");
        assert_eq!(vs[0].kind, ViolationKind::RequiredOption);
        assert!(vs[0].to_string().contains("@required is not valid"));
    }
}

#[test]
fn at_default_on_a_repeated_field_is_a_bind_time_violation() {
    for fds in fixture!("repeated_default") {
        let vs = validate_descriptor(&desc(fds, "carrier.repeated_default.v1.M"));
        assert_eq!(vs.len(), 1, "{vs:?}");
        assert_eq!(vs[0].kind, ViolationKind::DefaultOption);
        assert!(vs[0].to_string().contains("@default is not valid"));
    }
}

#[test]
fn two_spellings_that_disagree_are_rejected_and_two_that_agree_are_not() {
    for fds in fixture!("disagree") {
        let vs = validate_descriptor(&desc(fds, "carrier.disagree.v1.M"));
        assert_eq!(vs.len(), 1, "{vs:?}");
        assert_eq!(vs[0].kind, ViolationKind::DefaultOption);
        assert!(vs[0].detail.contains("must not be reconciled"), "{vs:?}");
    }
    for fds in fixture!("agree") {
        let d = desc(fds, "carrier.agree.v1.M");
        assert!(validate_descriptor(&d).is_empty());
        let f = d.get_field_by_name("target").or(d.get_field_by_name("f"));
        assert_eq!(get_default(&f.unwrap()).as_deref(), Some("a"));
    }
}
