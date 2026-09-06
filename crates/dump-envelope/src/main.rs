// SPDX-License-Identifier: MIT
// Copyright (c) 2026 TrendVidia, LLC.
// Cross-port wire-compatibility dumper, driven by protowire's
// scripts/cross_envelope_check.sh. Every port carries the same program and
// the script compares their output byte for byte. Mirrors
// protowire-go/scripts/dump_envelope.
//
//   dump-envelope                        canonical Envelope → pb hex
//   dump-envelope --pb  FDS MESSAGE DOC  PXF DOC decoded against MESSAGE in FDS → pb hex
//   dump-envelope --sbe FDS MESSAGE DOC  same → SBE hex
//
// The fixture modes apply the PXF annotations the descriptor carries, which
// is how the gate proves this port reads (pxf.required) = 1314,
// (pxf.default) = 1315 and the SBE numbers 1319–1323 from a descriptor it did
// not compile itself (STABILITY.md promise 3, protowire#244). A port looking
// for the wrong number decodes to different bytes, or accepts a document it
// must reject.
//
// Exit 0 with hex on stdout; 1 with "reject: <reason>" on stderr when the
// schema rejects DOC; 2 for anything that is the harness's fault.

use prost::Message as _;
use prost_reflect::DescriptorPool;
use protowire_envelope::Envelope;
use protowire_pxf::{unmarshal_full, UnmarshalOptions};
use protowire_sbe::Codec;

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.as_slice() {
        [] => dump_envelope(),
        [mode, fds, message, doc] if mode == "--pb" || mode == "--sbe" => {
            dump_fixture(mode, fds, message, doc)
        }
        _ => fatal(2, "usage: dump-envelope [--pb|--sbe FDS MESSAGE DOC]"),
    }
}

fn fatal(code: i32, msg: impl std::fmt::Display) -> ! {
    eprintln!("dump-envelope: {msg}");
    std::process::exit(code)
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

fn dump_envelope() {
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
        .with_field("amount", "MIN_VALUE", "below minimum", vec!["10.00".into()])
        .with_meta("request_id", "req-123");

    println!("{}", hex(&protowire_pb::marshal(&env)));
}

fn dump_fixture(mode: &str, fds_path: &str, message: &str, doc_path: &str) {
    let fds_bytes =
        std::fs::read(fds_path).unwrap_or_else(|e| fatal(2, format!("{fds_path}: {e}")));
    let pool = DescriptorPool::decode(fds_bytes.as_slice())
        .unwrap_or_else(|e| fatal(2, format!("{fds_path}: {e}")));
    let desc = pool
        .get_message_by_name(message)
        .unwrap_or_else(|| fatal(2, format!("{fds_path}: {message} not found")));
    let doc =
        std::fs::read_to_string(doc_path).unwrap_or_else(|e| fatal(2, format!("{doc_path}: {e}")));

    // The full decode is the one that validates (pxf.required) and applies
    // (pxf.default); plain unmarshal leaves both to the caller.
    let (msg, _presence) = match unmarshal_full(&doc, &desc, UnmarshalOptions::default()) {
        Ok(v) => v,
        Err(e) => {
            eprintln!("reject: {e}");
            std::process::exit(1)
        }
    };

    let out = if mode == "--pb" {
        msg.encode_to_vec()
    } else {
        let codec = Codec::from_files(&[desc.parent_file()]).unwrap_or_else(|e| fatal(2, e));
        protowire_sbe::marshal(&codec, &msg).unwrap_or_else(|e| fatal(2, e))
    };
    println!("{}", hex(&out));
}
