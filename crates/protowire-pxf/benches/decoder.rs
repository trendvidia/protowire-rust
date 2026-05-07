// SPDX-License-Identifier: MIT
// Copyright (c) 2026 TrendVidia, LLC.
//! PXF decoder + encoder microbenchmarks against a representative payload.
//!
//! Mirrors the shape of `BenchmarkPXF{Unmarshal,Marshal}` in
//! `protowire/encoding/pxf/benchmark_test.go` so numbers can be compared
//! across ports against the same `bench.v1.Config` schema and PXF text.
//!
//! Run with `cargo bench -p protowire-pxf` (release profile is implicit).

use criterion::{black_box, criterion_group, criterion_main, Criterion, Throughput};
use prost_reflect::{DescriptorPool, MessageDescriptor};
use protowire_pxf::{marshal, unmarshal, MarshalOptions, UnmarshalOptions};

const BENCH_FDS: &[u8] = include_bytes!("../testdata/bench-test.binpb");

const BENCH_PXF: &str = r#"
hostname = "web-01.prod.example.com"
port = 8443
enabled = true
weight = 0.85
status = STATUS_SERVING
tags = ["production", "us-east", "frontend", "critical"]
tls {
  cert_file = "/etc/ssl/certs/server.pem"
  key_file = "/etc/ssl/private/server.key"
  verify = true
}
labels = {
  env: "production"
  team: "platform"
  region: "us-east-1"
  tier: "frontend"
}
endpoints = [
  {
    path = "/api/v1/users"
    method = "GET"
    timeout_ms = 5000
  },
  {
    path = "/api/v1/orders"
    method = "POST"
    timeout_ms = 10000
  },
  {
    path = "/health"
    method = "GET"
    timeout_ms = 1000
  }
]
created_at = 2024-06-15T12:00:00Z
timeout = 30s
"#;

fn config_descriptor() -> MessageDescriptor {
    let pool = DescriptorPool::decode(BENCH_FDS).expect("decode bench-test.binpb");
    pool.get_message_by_name("bench.v1.Config")
        .expect("missing bench.v1.Config")
}

fn bench_unmarshal(c: &mut Criterion) {
    let desc = config_descriptor();
    let mut group = c.benchmark_group("pxf");
    group.throughput(Throughput::Bytes(BENCH_PXF.len() as u64));
    group.bench_function("unmarshal", |b| {
        b.iter(|| {
            let m = unmarshal(black_box(BENCH_PXF), &desc, UnmarshalOptions::default())
                .expect("decode");
            black_box(m);
        });
    });
    group.finish();
}

fn bench_marshal(c: &mut Criterion) {
    let desc = config_descriptor();
    let msg = unmarshal(BENCH_PXF, &desc, UnmarshalOptions::default()).expect("seed decode");
    let mut group = c.benchmark_group("pxf");
    group.bench_function("marshal", |b| {
        b.iter(|| {
            let s = marshal(black_box(&msg), &desc, MarshalOptions::default());
            black_box(s);
        });
    });
    group.finish();
}

criterion_group!(benches, bench_unmarshal, bench_marshal);
criterion_main!(benches);
