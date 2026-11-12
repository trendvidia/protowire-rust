// SPDX-License-Identifier: MIT
// Copyright (c) 2026 TrendVidia, LLC.
//! What the SBE wire can hold (#122), held against protowire-go itself:
//! `testdata/conformance/bounds-oracle.tsv` is what protowire-go's codec did
//! with each case of `bounds-cases.tsv`, over the fixtures in
//! `testdata/bounds/` (`bounds.binpb`). Regenerate it with
//! `scripts/gen-sbe-oracle.sh`; `scripts/sbe-oracle/main.go` says what each
//! column holds, and this file computes the same columns.
//!
//! The cases port protowire-go's `encoding_range_test.go` and
//! `header_bounds_test.go`: a value its `(sbe.encoding)` primitive cannot hold
//! is refused on marshal, a wire value its field cannot hold on unmarshal, an
//! encoding that crosses integers and floating point when the codec is built,
//! and every header field or block past 65535; each beside the value at the
//! bound, which must round-trip.

use prost_reflect::{
    DescriptorPool, DynamicMessage, FieldDescriptor, Kind, MessageDescriptor, ReflectMessage, Value,
};
use protowire_sbe::{marshal, unmarshal, Codec};

const FDS: &[u8] = include_bytes!("../testdata/bounds.binpb");
const ORACLE: &str = include_str!("../testdata/conformance/bounds-oracle.tsv");

fn bytes_text(b: &[u8]) -> String {
    let hex = |b: &[u8]| b.iter().map(|x| format!("{x:02x}")).collect::<String>();
    if b.len() <= 64 {
        return hex(b);
    }
    let mut h: u64 = 0xcbf29ce484222325;
    for &x in b {
        h ^= x as u64;
        h = h.wrapping_mul(0x100000001b3);
    }
    format!("{}..len={},fnv={h:016x}", hex(&b[..16]), b.len())
}

fn value_text(fd: &FieldDescriptor, v: &Value) -> String {
    match (fd.kind(), v) {
        (Kind::Float, Value::F32(f)) => format!("f32:{:08x}", f.to_bits()),
        (Kind::Double, Value::F64(f)) => format!("f64:{:016x}", f.to_bits()),
        (_, Value::Bool(b)) => b.to_string(),
        (_, Value::EnumNumber(n)) => n.to_string(),
        (_, Value::I32(n)) => n.to_string(),
        (_, Value::I64(n)) => n.to_string(),
        (_, Value::U32(n)) => n.to_string(),
        (_, Value::U64(n)) => n.to_string(),
        (_, Value::String(s)) => format!("string({})", s.len()),
        (_, Value::Bytes(b)) => format!("bytes({})", b.len()),
        (_, Value::Message(m)) => render(m),
        (k, v) => panic!("no text for {k:?} {v:?}"),
    }
}

fn render(m: &DynamicMessage) -> String {
    let mut parts = Vec::new();
    for fd in m.descriptor().fields() {
        let v = m.get_field(&fd);
        if fd.is_list() {
            let list = v.as_list().expect("list");
            let mut s = format!("{}=[{}]", fd.name(), list.len());
            if !list.is_empty() && list.len() <= 4 {
                let es: Vec<String> = list
                    .iter()
                    .map(|e| render(e.as_message().expect("entry")))
                    .collect();
                s.push_str(&es.join(","));
            }
            parts.push(s);
            continue;
        }
        parts.push(format!("{}={}", fd.name(), value_text(&fd, &v)));
    }
    format!("{{{}}}", parts.join(" "))
}

fn parse_value(fd: &FieldDescriptor, val: &str) -> Value {
    match fd.kind() {
        Kind::Bool => Value::Bool(val.parse().expect("bool")),
        Kind::Enum(_) => Value::EnumNumber(val.parse().expect("enum")),
        Kind::Int32 | Kind::Sint32 | Kind::Sfixed32 => Value::I32(val.parse().expect("i32")),
        Kind::Int64 | Kind::Sint64 | Kind::Sfixed64 => Value::I64(val.parse().expect("i64")),
        Kind::Uint32 | Kind::Fixed32 => Value::U32(val.parse().expect("u32")),
        Kind::Uint64 | Kind::Fixed64 => Value::U64(val.parse().expect("u64")),
        Kind::Float => Value::F32(val.parse::<f64>().expect("float") as f32),
        Kind::Double => Value::F64(val.parse().expect("double")),
        k => panic!("cannot assign a {k:?} field"),
    }
}

/// scripts/sbe-oracle's `assign`: path=value;..., where grp+.e appends an
/// entry and sets e on it, and e*N appends N empty entries.
fn assign(m: &mut DynamicMessage, spec: &str) {
    if spec.is_empty() {
        return;
    }
    for a in spec.split(';') {
        if let Some((name, n)) = a.split_once('*') {
            let fd = m.descriptor().get_field_by_name(name).expect("field");
            let Kind::Message(md) = fd.kind() else {
                panic!("{name} is not a group")
            };
            let n: usize = n.parse().expect("count");
            let entries = vec![Value::Message(DynamicMessage::new(md)); n];
            m.set_field(&fd, Value::List(entries));
            continue;
        }
        let (path, val) = a.split_once('=').expect("assignment");
        set(m, &path.split('.').collect::<Vec<_>>(), val);
    }
}

fn set(m: &mut DynamicMessage, path: &[&str], val: &str) {
    let (name, append) = match path[0].strip_suffix('+') {
        Some(n) => (n, true),
        None => (path[0], false),
    };
    let fd = m.descriptor().get_field_by_name(name).expect("field");
    if path.len() == 1 {
        let v = parse_value(&fd, val);
        m.set_field(&fd, v);
        return;
    }
    let Kind::Message(md) = fd.kind() else {
        panic!("{name} is not a message")
    };
    if append {
        let mut el = DynamicMessage::new(md);
        set(&mut el, &path[1..], val);
        let mut list = m.get_field(&fd).as_list().expect("list").to_vec();
        list.push(Value::Message(el));
        m.set_field(&fd, Value::List(list));
        return;
    }
    let mut sub = m
        .get_field(&fd)
        .as_message()
        .cloned()
        .unwrap_or_else(|| DynamicMessage::new(md));
    set(&mut sub, &path[1..], val);
    m.set_field(&fd, Value::Message(sub));
}

fn message(pool: &DescriptorPool, name: &str) -> MessageDescriptor {
    pool.get_message_by_name(name)
        .unwrap_or_else(|| panic!("missing {name}"))
}

fn run(pool: &DescriptorPool, case: &[&str]) -> String {
    let file = pool
        .get_file_by_name(case[1])
        .unwrap_or_else(|| panic!("missing {}", case[1]));
    let codec = Codec::from_files(&[file]);
    match case[0] {
        "bind" => match codec {
            Ok(_) => "OK".to_string(),
            Err(e) => format!("ERR\t{e}"),
        },
        "marshal" => {
            let codec = codec.expect("codec");
            let md = message(pool, case[2]);
            let mut msg = DynamicMessage::new(md.clone());
            assign(&mut msg, case[3]);
            let data = match marshal(&codec, &msg) {
                Ok(d) => d,
                Err(e) => return format!("ERR\t{e}"),
            };
            let mut back = DynamicMessage::new(md);
            let read = match unmarshal(&codec, &mut back, &data) {
                Ok(()) => render(&back),
                Err(e) => format!("ERR:{e}"),
            };
            format!("OK\t{}\t{read}", bytes_text(&data))
        }
        "wire" => {
            let codec = codec.expect("codec");
            let md = message(pool, case[2]);
            let mut data =
                marshal(&codec, &DynamicMessage::new(md.clone())).expect("empty message");
            let bits = u64::from_str_radix(case[3], 16).expect("bits");
            let n = data.len() - 8;
            data[8..].copy_from_slice(&bits.to_le_bytes()[..n]);
            let mut back = DynamicMessage::new(md);
            if let Err(e) = unmarshal(&codec, &mut back, &data) {
                return format!("ERR\t{e}");
            }
            let written = match marshal(&codec, &back) {
                Ok(d) => bytes_text(&d),
                Err(e) => format!("ERR:{e}"),
            };
            format!("OK\t{}\t{written}", render(&back))
        }
        op => panic!("unknown op {op}"),
    }
}

#[test]
fn bounds_as_protowire_go() {
    let pool = DescriptorPool::decode(FDS).expect("decode bounds.binpb");
    let mut checked = 0;
    let mut diffs = Vec::new();
    for line in ORACLE.lines() {
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let cols: Vec<&str> = line.split('\t').collect();
        let n = if cols[0] == "bind" { 2 } else { 4 };
        let want = cols[n..].join("\t");
        // A panic is a difference too, so one case cannot hide the rest.
        let got = std::panic::catch_unwind(|| run(&pool, &cols[..n]))
            .unwrap_or_else(|_| "PANIC".to_string());
        if got != want {
            diffs.push(format!(
                "{}\n   go: {want}\n rust: {got}",
                cols[..n].join(" ")
            ));
        }
        checked += 1;
    }
    assert!(
        diffs.is_empty(),
        "{} of {checked} differ:\n{}",
        diffs.len(),
        diffs.join("\n")
    );
    // 57 files bound, 128 marshals, 23 wire reads: bounds-cases.tsv.
    assert_eq!(checked, 208);
}
