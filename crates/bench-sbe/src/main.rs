// SPDX-License-Identifier: MIT
// Copyright (c) 2026 TrendVidia, LLC.
//! Cross-port SBE microbench: Rust implementation.
//!
//! Loads `<testdata>/sbe-bench.binpb` (FileDescriptorSet), populates a
//! canonical `bench.v1.Order` (10 scalars + 2-entry Fill group), and
//! times marshal + unmarshal for at least `--seconds` (default 3).
//! Prints one JSON line per op:
//!
//! ```text
//! {"port":"rust","op":"sbe-marshal","ns_per_op":300,"iterations":...,"bytes":94}
//! {"port":"rust","op":"sbe-unmarshal","ns_per_op":1100,"mib_per_sec":81.5,"iterations":...,"bytes":94}
//! ```

use std::path::PathBuf;
use std::time::{Duration, Instant};

use prost_reflect::{DescriptorPool, DynamicMessage, MessageDescriptor, Value};
use protowire_sbe::{marshal, unmarshal, Codec};

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let mut seconds: f64 = 3.0;
    let mut testdata: Option<PathBuf> = None;
    let mut i = 1;
    while i < args.len() {
        match args[i].as_str() {
            "--seconds" => {
                i += 1;
                seconds = args[i].parse().expect("--seconds float");
            }
            "--testdata" => {
                i += 1;
                testdata = Some(PathBuf::from(&args[i]));
            }
            other => {
                eprintln!("bench-sbe: unknown arg {:?}", other);
                std::process::exit(2);
            }
        }
        i += 1;
    }
    let dir = testdata.unwrap_or_else(|| std::env::current_dir().expect("getcwd").join("testdata"));

    let fds_bytes = std::fs::read(dir.join("sbe-bench.binpb")).expect("read sbe-bench.binpb");
    let pool = DescriptorPool::decode(fds_bytes.as_slice()).expect("decode FDS");
    let order_desc = pool
        .get_message_by_name("bench.v1.Order")
        .expect("missing bench.v1.Order");
    let fill_desc = pool
        .get_message_by_name("bench.v1.Order.Fill")
        .expect("missing Fill");

    let file = order_desc.parent_file();
    let codec = Codec::from_files(&[file]).expect("build codec");

    let msg = build_order(&order_desc, &fill_desc);
    let target = Duration::from_secs_f64(seconds);

    // Warm-up + capture wire size.
    let wire_bytes = marshal(&codec, &msg).expect("warm-up marshal");
    let n = wire_bytes.len();

    let (iters_m, elapsed_m) = time_loop(target, || {
        let _ = marshal(&codec, &msg).expect("marshal");
    });
    println!(
        "{{\"port\":\"rust\",\"op\":\"sbe-marshal\",\"ns_per_op\":{},\"iterations\":{},\"bytes\":{}}}",
        elapsed_m.as_nanos() as u64 / iters_m,
        iters_m,
        n
    );

    let (iters_u, elapsed_u) = time_loop(target, || {
        let mut out = DynamicMessage::new(order_desc.clone());
        unmarshal(&codec, &mut out, &wire_bytes).expect("unmarshal");
    });
    let mib = (n as f64 * iters_u as f64 / (1024.0 * 1024.0)) / elapsed_u.as_secs_f64();
    println!(
        "{{\"port\":\"rust\",\"op\":\"sbe-unmarshal\",\"ns_per_op\":{},\"mib_per_sec\":{},\"iterations\":{},\"bytes\":{}}}",
        elapsed_u.as_nanos() as u64 / iters_u,
        mib,
        iters_u,
        n
    );
}

fn build_order(desc: &MessageDescriptor, fill_desc: &MessageDescriptor) -> DynamicMessage {
    let mut msg = DynamicMessage::new(desc.clone());
    let set = |msg: &mut DynamicMessage, name: &str, v: Value| {
        let fd = msg
            .descriptor()
            .get_field_by_name(name)
            .unwrap_or_else(|| panic!("missing field {name}"));
        msg.set_field(&fd, v);
    };
    use prost_reflect::ReflectMessage;
    set(&mut msg, "order_id", Value::U64(1001));
    set(&mut msg, "symbol", Value::String("AAPL".into()));
    set(&mut msg, "price", Value::I64(19150));
    set(&mut msg, "quantity", Value::U32(100));
    set(&mut msg, "side", Value::EnumNumber(1));
    set(&mut msg, "active", Value::Bool(true));
    set(&mut msg, "weight", Value::F64(0.85));
    set(&mut msg, "score", Value::F32(2.5));

    let fills_fd = desc.get_field_by_name("fills").expect("fills");
    let mut fills: Vec<Value> = Vec::with_capacity(2);
    for &(price, qty, id) in &[(19155i64, 25u32, 5001u64), (19160, 50, 5002)] {
        let mut f = DynamicMessage::new(fill_desc.clone());
        let fp = fill_desc
            .get_field_by_name("fill_price")
            .expect("fill_price");
        let fq = fill_desc.get_field_by_name("fill_qty").expect("fill_qty");
        let fid = fill_desc.get_field_by_name("fill_id").expect("fill_id");
        f.set_field(&fp, Value::I64(price));
        f.set_field(&fq, Value::U32(qty));
        f.set_field(&fid, Value::U64(id));
        fills.push(Value::Message(f));
    }
    msg.set_field(&fills_fd, Value::List(fills));
    msg
}

fn time_loop(target: Duration, mut fn_: impl FnMut()) -> (u64, Duration) {
    let start = Instant::now();
    let mut iters: u64 = 0;
    loop {
        for _ in 0..64 {
            fn_();
        }
        iters += 64;
        if start.elapsed() >= target {
            break;
        }
    }
    (iters, start.elapsed())
}
