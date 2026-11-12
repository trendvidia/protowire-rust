// SPDX-License-Identifier: MIT
// Copyright (c) 2026 TrendVidia, LLC.
//! A duration literal is valid, and has the value, that Go's
//! `time.ParseDuration` gives it, as protowire-go's lexer and decoder use
//! it (#153): a segment's fraction may stand without a leading digit
//! (`1h.5m`), a long fraction of an hour rounds as Go's float64 arithmetic
//! rounds it, and a total past the signed 64-bit range of nanoseconds is
//! refused by the lexer.
//!
//! Each row is what protowire-go v1.17.0 did with `dur_field = <literal>`
//! on `test.v1.AllTypes`: the message's bytes in hex, or the error.

use prost::Message;
use prost_reflect::DescriptorPool;
use protowire_pxf::unmarshal;

#[rustfmt::skip]
const ROWS: &[(&str, Result<&str, &str>)] = &[
    (r#"1h.5m"#, Ok("92010308ae1c")),
    (r#"5m.5s"#, Ok("92010908ac021080cab5ee01")),
    (r#"1h.5m.5s"#, Ok("92010908ae1c1080cab5ee01")),
    (r#".5s"#, Err(r#"1:13: unexpected character '.'"#)),
    (r#"1.5h"#, Ok("92010308982a")),
    (r#"1h30m"#, Ok("92010308982a")),
    (r#"1h30.5m"#, Ok("92010308b62a")),
    (r#"0s"#, Ok("920100")),
    (r#"0"#, Err(r#"1:13: expected a duration literal or '{' for message field "dur_field", got integer ("0")"#)),
    (r#"-0"#, Err(r#"1:13: expected a duration literal or '{' for message field "dur_field", got integer ("-0")"#)),
    (r#"+0"#, Err(r#"1:13: "+" is valid only in "+inf""#)),
    (r#"1us"#, Ok("92010310e807")),
    (r#"1µs"#, Ok("92010310e807")),
    (r#"1ns"#, Ok("9201021001")),
    (r#"1ms"#, Ok("92010410c0843d")),
    (r#"01s"#, Ok("9201020801")),
    (r#"1h1h"#, Ok("92010308a038")),
    (r#"-1h.5m"#, Ok("92010b08d2e3ffffffffffffff01")),
    (r#"+1.5s"#, Err(r#"1:13: "+" is valid only in "+inf""#)),
    (r#"1.0000000001h"#, Ok("92010608901c10e802")),
    (r#"1.123456789123h"#, Ok("92010908cc1f108ac2f6d301")),
    (r#"0.333333333333333333333h"#, Ok("92010308b009")),
    (r#"1.9999999999999999999h"#, Ok("92010308a038")),
    (r#".000000001s"#, Err(r#"1:13: unexpected character '.'"#)),
    (r#".0000000001s"#, Err(r#"1:13: unexpected character '.'"#)),
    (r#"2562047h"#, Ok("92010608f0e385ae22")),
    (r#"2562047h47m16.854775807s"#, Ok("92010c0884fa85ae2210ffafcb9703")),
    (r#"2562047h47m16.854775808s"#, Err(r#"1:13: invalid duration: 2562047h47m16.854775808s"#)),
    (r#"-2562047h47m16.854775808s"#, Ok("92011608fc85fad1ddffffffff011080d0b4e8fcffffffff01")),
    (r#"-2562047h47m16.854775809s"#, Err(r#"1:13: invalid duration: -2562047h47m16.854775809s"#)),
    (r#"9223372036854775807ns"#, Ok("92010c0884fa85ae2210ffafcb9703")),
    (r#"9223372036854775808ns"#, Err(r#"1:13: invalid duration: 9223372036854775808ns"#)),
    (r#"99999999999999999999ns"#, Err(r#"1:13: invalid duration: 99999999999999999999ns"#)),
    (r#"1.00000000000000000000000001h"#, Ok("92010308901c")),
    (r#"123456789.987654321s"#, Ok("92010b08959aef3a10b1d1f9d603")),
    (r#"0.1s"#, Ok("9201051080c2d72f")),
    (r#"0.7m"#, Ok("920102082a")),
    (r#"2.5h.25m"#, Ok("92010308b746")),
];

#[test]
fn a_duration_reads_as_protowire_go_reads_it() {
    let pool =
        DescriptorPool::decode(&include_bytes!("../testdata/test.binpb")[..]).expect("test.binpb");
    let desc = pool
        .get_message_by_name("test.v1.AllTypes")
        .expect("test.v1.AllTypes");
    let mut checked = 0;
    for &(lit, want) in ROWS {
        let got = unmarshal(&format!("dur_field = {lit}"), &desc, Default::default())
            .map(|m| {
                m.encode_to_vec()
                    .iter()
                    .map(|b| format!("{b:02x}"))
                    .collect::<String>()
            })
            .map_err(|e| e.to_string());
        assert_eq!(
            got.as_ref().map(String::as_str).map_err(String::as_str),
            want,
            "{lit}"
        );
        checked += 1;
    }
    assert_eq!(checked, 38);
}
