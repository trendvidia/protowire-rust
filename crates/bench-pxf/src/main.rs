// SPDX-License-Identifier: MIT
// Copyright (c) 2026 TrendVidia, LLC.
//! Cross-port PXF microbench: Rust implementation.
//!
//! Reads `<testdata>/bench-test.binpb` (FileDescriptorSet) and
//! `<testdata>/bench-test.pxf` (text payload), times unmarshal +
//! marshal of `bench.v1.Config` for at least `--seconds` (default 3),
//! and prints one JSON line per op:
//!
//! ```text
//! {"port":"rust","op":"unmarshal","ns_per_op":5800,"mib_per_sec":107.0,"iterations":517248,"bytes":652}
//! {"port":"rust","op":"marshal","ns_per_op":5200,"iterations":576000}
//! ```
//!
//! The other ports' bench-pxf binaries print the same shape; the
//! `protowire/scripts/cross_pxf_bench.sh` runner aggregates them.

use std::path::PathBuf;
use std::time::{Duration, Instant};

use prost_reflect::{DescriptorPool, MessageDescriptor};
use protowire_pxf::{marshal, unmarshal, MarshalOptions, UnmarshalOptions};

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let mut seconds: f64 = 3.0;
    let mut testdata: Option<PathBuf> = None;

    let mut i = 1;
    while i < args.len() {
        match args[i].as_str() {
            "--seconds" => {
                i += 1;
                seconds = args
                    .get(i)
                    .and_then(|s| s.parse().ok())
                    .expect("--seconds takes a float");
            }
            "--testdata" => {
                i += 1;
                testdata = args.get(i).map(PathBuf::from);
            }
            other => {
                eprintln!("bench-pxf: unknown arg {:?}", other);
                std::process::exit(2);
            }
        }
        i += 1;
    }

    let dir = testdata.unwrap_or_else(|| {
        std::env::current_dir()
            .expect("getcwd")
            .join("testdata")
    });

    let fds_bytes = std::fs::read(dir.join("bench-test.binpb"))
        .expect("read bench-test.binpb");
    let pxf_text = std::fs::read_to_string(dir.join("bench-test.pxf"))
        .expect("read bench-test.pxf");

    let desc = load_config_descriptor(&fds_bytes);
    let target = Duration::from_secs_f64(seconds);

    // Warm-up.
    let _ = unmarshal(&pxf_text, &desc, UnmarshalOptions::default())
        .expect("warm-up unmarshal");

    let (iters, elapsed) = time_loop(target, || {
        let _ = unmarshal(&pxf_text, &desc, UnmarshalOptions::default())
            .expect("unmarshal");
    });
    let bytes = pxf_text.len();
    emit_unmarshal(iters, elapsed, bytes);

    let msg = unmarshal(&pxf_text, &desc, UnmarshalOptions::default())
        .expect("seed unmarshal for marshal");
    let (iters2, elapsed2) = time_loop(target, || {
        let _ = marshal(&msg, &desc, MarshalOptions::default());
    });
    emit_marshal(iters2, elapsed2);
}

fn load_config_descriptor(fds: &[u8]) -> MessageDescriptor {
    let pool = DescriptorPool::decode(fds).expect("decode FileDescriptorSet");
    pool.get_message_by_name("bench.v1.Config")
        .expect("missing bench.v1.Config")
}

fn time_loop(target: Duration, mut fn_: impl FnMut()) -> (u64, Duration) {
    let start = Instant::now();
    let mut iters: u64 = 0;
    loop {
        // Run in batches of 64 to keep timer overhead in the noise.
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

fn emit_unmarshal(iters: u64, elapsed: Duration, bytes: usize) {
    let ns_per_op = (elapsed.as_nanos() as u64) / iters;
    let total_bytes = bytes as f64 * iters as f64;
    let mib_per_sec = (total_bytes / (1024.0 * 1024.0)) / elapsed.as_secs_f64();
    println!(
        "{{\"port\":\"rust\",\"op\":\"unmarshal\",\"ns_per_op\":{},\"mib_per_sec\":{},\"iterations\":{},\"bytes\":{}}}",
        ns_per_op, mib_per_sec, iters, bytes
    );
}

fn emit_marshal(iters: u64, elapsed: Duration) {
    let ns_per_op = (elapsed.as_nanos() as u64) / iters;
    println!(
        "{{\"port\":\"rust\",\"op\":\"marshal\",\"ns_per_op\":{},\"iterations\":{}}}",
        ns_per_op, iters
    );
}
