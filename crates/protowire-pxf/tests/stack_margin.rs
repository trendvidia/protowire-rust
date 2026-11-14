// SPDX-License-Identifier: MIT
// Copyright (c) 2026 TrendVidia, LLC.
//! A document nested to `MAX_NESTING_DEPTH` is read and written on a thread
//! with half of Rust's default 2 MiB stack, debug build included (#179).
//! The depth limit exists so that deep input is an error and not a stack
//! overflow, which aborts the process. That holds only while the deepest
//! accepted document fits the stack it is read on.
//!
//! Every way a body can nest is covered: a block, an assigned block, a
//! list element, a map value, a keyed entry, an Any (as a block and
//! assigned) and an Any inside a list element or map value. Each is read
//! by every reader and written by every writer, and the same nesting is
//! read inside a `@dataset` cell. Each run gets its own named thread, so
//! when the margin is lost the abort names the shape and the operation:
//! `thread 'unmarshal anyassign' has overflowed its stack`.
//!
//! In a debug build on macOS aarch64 the deepest of them needed 512 KiB
//! when this test was written, against 2368 KiB before #179's change. A
//! release build needs a fraction of that.
//!
//! The test has a binary of its own because an overflow aborts the binary.

use prost::Message as _;
use prost_reflect::{DescriptorPool, DynamicMessage, MessageDescriptor};
use protowire_pxf::{
    bind_row, format, marshal, parse, parse_tolerant, unmarshal, unmarshal_full, DatasetReader,
    MarshalOptions, PoolResolver, UnmarshalOptions, MAX_NESTING_DEPTH,
};

/// The stack every operation runs on: half the default, so the default
/// keeps at least twice what the deepest document needs.
const STACK: usize = 1 << 20;

const FDS: &[u8] = include_bytes!("../testdata/marshal-depth-test.binpb");

fn pool() -> &'static DescriptorPool {
    static POOL: std::sync::OnceLock<DescriptorPool> = std::sync::OnceLock::new();
    POOL.get_or_init(|| DescriptorPool::decode(FDS).expect("decode FDS"))
}

fn t() -> MessageDescriptor {
    pool().get_message_by_name("mdepth.v1.T").unwrap()
}

fn resolver() -> PoolResolver<'static> {
    PoolResolver(pool())
}

fn reading<'a>(res: &'a PoolResolver<'static>) -> UnmarshalOptions<'a> {
    UnmarshalOptions {
        type_resolver: Some(res),
        ..Default::default()
    }
}

/// Each way to nest one `T` in another, with the descents (`{` or `[`) one
/// level costs.
const SHAPES: [(&str, usize); 9] = [
    ("block", 1),
    ("assign", 1),
    ("list", 2),
    ("map", 2),
    ("keyed", 2),
    // An Any's body shares the Any's braces.
    ("any", 1),
    ("anyassign", 1),
    ("anylist", 3),
    ("anymap", 3),
];

fn wrap(shape: &str, inner: &str) -> String {
    const ANY: &str = "@type = \"mdepth.v1.T\"";
    match shape {
        "block" => format!("next {{ {inner} }}"),
        "assign" => format!("next = {{ {inner} }}"),
        "list" => format!("kids = [ {{ {inner} }} ]"),
        "map" => format!("by_key = {{ \"a\": {{ {inner} }} }}"),
        "keyed" => format!("keyed {{ \"a\" {{ {inner} }} }}"),
        "any" => format!("payload {{ {ANY} {inner} }}"),
        "anyassign" => format!("payload = {{ {ANY} {inner} }}"),
        "anylist" => format!("kids = [ {{ payload {{ {ANY} {inner} }} }} ]"),
        "anymap" => format!("by_key = {{ \"a\": {{ payload {{ {ANY} {inner} }} }} }}"),
        _ => unreachable!("{shape}"),
    }
}

/// `levels` of `shape` around a scalar.
fn nested(shape: &str, levels: usize) -> String {
    (0..levels).fold(String::from("v = 1"), |inner, _| wrap(shape, &inner))
}

/// A one-row dataset whose cell holds `levels` of `shape`; the cell's own
/// braces are one descent.
fn in_cell(shape: &str, levels: usize) -> String {
    format!(
        "@dataset mdepth.v1.T (next)\n({{ {} }})\n",
        nested(shape, levels)
    )
}

/// Runs `f` on a thread named `name` with [`STACK`] bytes of stack.
fn on_small_stack(name: String, f: impl FnOnce() + Send + 'static) {
    std::thread::Builder::new()
        .name(name.clone())
        .stack_size(STACK)
        .spawn(f)
        .unwrap()
        .join()
        .unwrap_or_else(|_| panic!("{name} panicked"));
}

fn bind_parsed(text: &str) -> Result<usize, String> {
    let doc = parse(text).map_err(|e| e.to_string())?;
    let res = resolver();
    let mut rows = 0;
    for ds in &doc.datasets {
        for row in &ds.rows {
            bind_row(&t(), &ds.columns, row, reading(&res)).map_err(|e| e.to_string())?;
            rows += 1;
        }
    }
    Ok(rows)
}

fn bind_streamed(text: &str) -> Result<usize, String> {
    let res = resolver();
    let mut reader = DatasetReader::new(text.as_bytes()).map_err(|e| e.to_string())?;
    let columns = reader.columns().to_vec();
    let mut rows = 0;
    while let Some(row) = reader.next_row() {
        let row = row.map_err(|e| e.to_string())?;
        bind_row(&t(), &columns, &row, reading(&res)).map_err(|e| e.to_string())?;
        rows += 1;
    }
    Ok(rows)
}

/// The documents are the deepest the reader takes: one level more is
/// refused. Checked on a thread with room to spare, so this holds whatever
/// the stack test finds.
#[test]
fn the_documents_are_at_the_limit() {
    std::thread::Builder::new()
        .stack_size(64 << 20)
        .spawn(|| {
            let res = resolver();
            let too_deep = format!("nesting depth exceeds MaxNestingDepth={MAX_NESTING_DEPTH}");
            for (shape, cost) in SHAPES {
                let levels = MAX_NESTING_DEPTH / cost;
                unmarshal(&nested(shape, levels), &t(), reading(&res))
                    .unwrap_or_else(|e| panic!("{shape} x{levels}: {e}"));
                let e = unmarshal(&nested(shape, levels + 1), &t(), reading(&res))
                    .expect_err(&format!("{shape} x{}", levels + 1));
                assert!(e.to_string().contains(&too_deep), "{shape}: {e}");

                let levels = (MAX_NESTING_DEPTH - 1) / cost;
                assert_eq!(bind_parsed(&in_cell(shape, levels)), Ok(1), "{shape} cell");
                let e = bind_parsed(&in_cell(shape, levels + 1)).expect_err(shape);
                assert!(e.contains(&too_deep), "{shape} cell: {e}");
            }
        })
        .unwrap()
        .join()
        .unwrap();
}

#[test]
fn the_deepest_documents_fit_half_the_default_stack() {
    let mut ran = 0;
    for (shape, cost) in SHAPES {
        let text = nested(shape, MAX_NESTING_DEPTH / cost);
        let cell = in_cell(shape, (MAX_NESTING_DEPTH - 1) / cost);

        let doc = text.clone();
        on_small_stack(format!("parse {shape}"), move || {
            drop(parse(&doc).unwrap());
        });
        let doc = text.clone();
        on_small_stack(format!("parse_tolerant {shape}"), move || {
            let (_, errors) = parse_tolerant(&doc);
            assert!(errors.is_empty(), "{errors:?}");
        });
        let doc = text.clone();
        on_small_stack(format!("format {shape}"), move || {
            drop(format(&parse(&doc).unwrap()));
        });
        let doc = text.clone();
        on_small_stack(format!("unmarshal {shape}"), move || {
            drop(unmarshal(&doc, &t(), reading(&resolver())).unwrap());
        });
        let doc = text.clone();
        on_small_stack(format!("unmarshal_full {shape}"), move || {
            drop(unmarshal_full(&doc, &t(), reading(&resolver())).unwrap());
        });
        // The message is built on a thread with room to spare, so the small
        // one measures marshal alone. Writing an Any reads its body back.
        let doc = text.clone();
        let bytes = std::thread::Builder::new()
            .stack_size(64 << 20)
            .spawn(move || {
                unmarshal(&doc, &t(), reading(&resolver()))
                    .unwrap()
                    .encode_to_vec()
            })
            .unwrap()
            .join()
            .unwrap();
        on_small_stack(format!("marshal {shape}"), move || {
            let m = DynamicMessage::decode(t(), bytes.as_slice()).unwrap();
            let res = resolver();
            let opts = MarshalOptions {
                type_resolver: Some(&res),
                ..Default::default()
            };
            drop(marshal(&m, &t(), opts).unwrap());
        });
        let doc = cell.clone();
        on_small_stack(format!("bind_row {shape}"), move || {
            assert_eq!(bind_parsed(&doc), Ok(1));
        });
        on_small_stack(format!("DatasetReader {shape}"), move || {
            assert_eq!(bind_streamed(&cell), Ok(1));
        });
        ran += 8;
    }
    assert_eq!(ran, 8 * SHAPES.len());
}
