// SPDX-License-Identifier: MIT
// Copyright (c) 2026 TrendVidia, LLC.
// Dumps a canonical Envelope's pb-encoded bytes as hex, for cross-port
// wire-compat checking. Mirrors:
//   protowire/scripts/dump_envelope/main.go
//   protowire4cpp/cmd/dump_envelope/main.cc
//   protowire4ts/scripts/dump-envelope.ts
//   protowire4java/dump-envelope

use protowire_envelope::Envelope;
use protowire_pb::marshal;

fn main() {
    let mut env = Envelope::err(
        402,
        "INSUFFICIENT_FUNDS",
        "balance too low",
        vec!["$3.50".into(), "$10.00".into()],
    );
    env.data = vec![0xDE, 0xAD, 0xBE, 0xEF];
    env.error
        .as_mut()
        .expect("err builder sets error")
        .with_field(
            "amount",
            "MIN_VALUE",
            "below minimum",
            vec!["10.00".into()],
        )
        .with_meta("request_id", "req-123");

    let bytes = marshal(&env);
    let hex: String = bytes.iter().map(|b| format!("{:02x}", b)).collect();
    println!("{}", hex);
}
