// SPDX-License-Identifier: MIT
// Copyright (c) 2026 TrendVidia, LLC.
//! A `pxf.BigFloat` literal is read as protowire-go reads it: its exact
//! value rounded once to its precision, ties to even (#108,
//! protowire-go#260); the `big.Float.Parse` checks run at no more than
//! 16384 bits (#109, protowire-go#274); and a non-zero value past 2^31 - 1
//! bits of precision is refused (#124).
//!
//! Each row is what protowire-go v1.17.0 did with `big_float_field =
//! <literal>` on `bignum_test.v1.BigNumDemo`, its bytes in hex or its
//! error, under the default limits (0) or with `MaxNumericLiteralDigits`
//! raised to 1.3 billion: the issue's tie, #109's and #124's refusals, a
//! spread of bounds and suffixes, and 60 random literals.

use std::time::{Duration, Instant};

use prost::Message;
use prost_reflect::DescriptorPool;
use protowire_pxf::{unmarshal, Limits, UnmarshalOptions};

#[rustfmt::skip]
const ROWS: &[(usize, &str, Result<&str, &str>)] = &[
    (0, "4.2026289288901168828283472716521907532475991810580890501360340372609e-4932p2", Ok("1a100a0102108180ffffffffffffff011802")),
    (1_300_000_000, "1e-300p2147483647", Err(r#"1:19: big float literal 1e-300 is below the wire's exponent range"#)),
    (1_300_000_000, "1e646456993p4294967295", Err(r#"1:19: big float literal 1e646456993 is above big.Float's range"#)),
    (1_300_000_000, "1e-646457000p2147483647", Err(r#"1:19: big float literal 1e-646457000 is below big.Float's range"#)),
    (1_300_000_000, "1e5000p2147483647", Err(r#"1:19: big float literal 1e5000 has binary exponent 16610, and MaxBigFloatExponent=16384 bounds its magnitude"#)),
    (0, "1p2147483648", Err(r#"1:19: big float literal 1p2147483648: precision p2147483648 renders to 646456995 digits; MaxNumericLiteralDigits=4096"#)),
    (1_300_000_000, "1p2147483648", Err(r#"1:19: big float literal 1: precision p2147483648 exceeds the widest mantissa big.Float can hold, 2147483647 bits"#)),
    (0, "0p2147483648", Err(r#"1:19: big float literal 0p2147483648: precision p2147483648 renders to 646456995 digits; MaxNumericLiteralDigits=4096"#)),
    (1_300_000_000, "0p2147483648", Ok("1a06188080808008")),
    (0, "-0p2147483648", Err(r#"1:19: big float literal -0p2147483648: precision p2147483648 renders to 646456995 digits; MaxNumericLiteralDigits=4096"#)),
    (1_300_000_000, "-0p2147483648", Ok("1a081880808080082001")),
    (0, "0p4294967295", Err(r#"1:19: big float literal 0p4294967295: precision p4294967295 renders to 1292913988 digits; MaxNumericLiteralDigits=4096"#)),
    (1_300_000_000, "0p4294967295", Ok("1a0618ffffffff0f")),
    (0, "1p4294967295", Err(r#"1:19: big float literal 1p4294967295: precision p4294967295 renders to 1292913988 digits; MaxNumericLiteralDigits=4096"#)),
    (1_300_000_000, "1p4294967295", Err(r#"1:19: big float literal 1: precision p4294967295 exceeds the widest mantissa big.Float can hold, 2147483647 bits"#)),
    (0, "1p4294967296", Err(r#"1:19: big float literal 1p4294967296: precision suffix p4294967296 is not a positive bit count"#)),
    (1_300_000_000, "1p4294967296", Err(r#"1:19: big float literal 1p4294967296: precision suffix p4294967296 is not a positive bit count"#)),
    (0, "1p0", Err(r#"1:19: big float literal 1p0: precision suffix p0 is not a positive bit count"#)),
    (1_300_000_000, "1p0", Err(r#"1:19: big float literal 1p0: precision suffix p0 is not a positive bit count"#)),
    (0, "0p0", Err(r#"1:19: big float literal 0p0: precision suffix p0 is not a positive bit count"#)),
    (1_300_000_000, "0p0", Err(r#"1:19: big float literal 0p0: precision suffix p0 is not a positive bit count"#)),
    (0, "0.1", Ok("1a300a20cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccd10fdfdffffffffffffff01188002")),
    (0, "0.1p53", Ok("1a160a071999999999999a10c8ffffffffffffffff011835")),
    (0, "0.1p24", Ok("1a120a03cccccd10e5ffffffffffffffff011818")),
    (0, "1e-4932", Ok("1a300a2098491f23ca4d5fc0b5be9c75e007166fc802a30e1d18b3279c250e88aa02efa41081fefeffffffffffff01188002")),
    (0, "1e4932", Ok("1a280a20d72cb2a95c7ef6cce81bf1e825ba7515c2feb521d6cb5dcd71130031feee017b10807e188002")),
    (0, "1.18973149535723176502e+4932", Ok("1a280a20fffffffffffffffefafbc1bed2d93cf52f75d5d18f60a989c407a099533a18eb10807e188002")),
    (0, "3.3621031431120935063e-4932", Ok("1a300a2080000000000000001a360c6a4bec19b6910f25b8fe2df26073114d9f7e507e021083fefeffffffffffff01188002")),
    (0, "2.5p2", Ok("1a050a01021802")),
    (0, "3.5p2", Ok("1a070a010210011802")),
    (0, "0.3p2", Ok("1a100a010210fdffffffffffffffff011802")),
    (0, "1e100p10", Ok("1a090a02024910c302180a")),
    (0, "123456789012345678901234567890", Ok("1a300a20c77487fb61b9f077271f8569000000000000000000000000000000000000000010e1feffffffffffffff01188002")),
    (0, "9.999999999999999999999999999999999999999e4931p100", Ok("1a140a0d0d72cb2a95c7ef6cce81bf1e82109c7f1864")),
    (0, "1e-4900p64", Ok("1a170a08bbb4df56baf6297210ab80ffffffffffffff011840")),
    (0, "-1.5e-10p30", Ok("1a150a04293b4fc710c2ffffffffffffffff01181e2001")),
    (0, "6.02214076e23", Ok("1a300a20ff0c2e52be28b98c00000000000000000000000000000000000000000000000010cffeffffffffffffff01188002")),
    (0, "1.602176634e-19p53", Ok("1a160a0717a4da290c1653108dffffffffffffffff011835")),
    (0, "2.718281828459045235360287471352662497757p200", Ok("1a290a19adf85458a2bb4a9aafdc5620273d3cf1d3582d9a0a031c96c010bafeffffffffffffff0118c801")),
    (0, "1e4931p16384", Err(r#"1:19: big float literal 1e4931p16384: precision p16384 renders to 4934 digits; MaxNumericLiteralDigits=4096"#)),
    (0, "260181590830166131.860e-1243p300", Ok("1a360a260cf506a07d379583b41783f29b2b1f0adfd466da5e6ce39c677d9e3ab8929d8c3c68b6bc97b410edddffffffffffffff0118ac02")),
    (0, "9.960e-2719p8", Ok("1a100a01ff10b3b9ffffffffffffff011808")),
    (0, "1948219935181909378657975432.3194875e-3701p1", Ok("1a100a010110d4a0ffffffffffffff011801")),
    (0, "527601895.55e2574p1", Ok("1a080a01011084431801")),
    (0, "710497465.075291703e-844p53", Ok("1a160a0719efe221ce7cf210f5e9ffffffffffffff011835")),
    (0, "-71276842684656321223307924402685995289.078666617603137215e-4897p256", Ok("1a320a209995b9f7f7d541d8ffddef398506572c679b1e020fbd512e0863dbfb0e19b90010f3fffeffffffffffff011880022001")),
    (0, "59.01396e791p256", Ok("1a280a20b894a4bc65a8edad38b505246e7b36295e8afb74bec63ab84d558f3fc10e642110ca12188002")),
    (0, "1.7777412e-7p64", Ok("1a170a08bee22d925a3b902a10aaffffffffffffffff011840")),
    (0, "80385280841e26p24", Ok("1a090a03c1854110631818")),
    (0, "-38885393363387.500474395e4p24", Ok("1a0b0a03acaf93102318182001")),
    (0, "-0.3735379e736p300", Ok("1a300a260b757e7ce600ee59c292cdb3d5fc1b7bc98c81d2c52cedff2c63719476419623c6f1e3295d5110e01018ac022001")),
    (0, "-16372651676122202972.99752882001826330434839e1965p2", Ok("1a0a0a010310be3318022001")),
    (0, "579868282880.729022279180588871803340187801759898e2511p113", Ok("1a160a0f015ae6ad62fed1bb424dcbdac29ba410cc401871")),
    (0, "83848372616751.36134125242731672e2170p113", Ok("1a160a0f01c93defa4d00f2f31dea528221e3010e6371871")),
    (0, "355150587706589481131144.024e2018p300", Ok("1a2e0a260ec2b20a128bcf5052cbcbc49c38da0cc176bd4d33337f530fffe2907965279fb406901314ee10d23218ac02")),
    (0, "62.889751402614014e-1257p1", Ok("1a100a010110b6dfffffffffffffff011801")),
    (0, "0.70586492e-994p1", Ok("1a100a01011099e6ffffffffffffff011801")),
    (0, "0.02344834782450400e30p3", Ok("1a070a0105105c1803")),
    (0, "-7167.868433532650e-713p53", Ok("1a180a071353e307296c1f1098edffffffffffffff0118352001")),
    (0, "0.684934e-2319p8", Ok("1a100a01ef10e0c3ffffffffffffff011808")),
    (0, "558530.43520561748e-4819p1", Ok("1a100a0101108b83ffffffffffffff011801")),
    (0, "26.9060e-1086p1", Ok("1a100a010110ede3ffffffffffffff011801")),
    (0, "2965724920868288.909310025167800837e-3752p1000", Ok("1a8d010a7db7f0bae85191b90926c9fc43d4afa41506c54e6e96cbf788ef9fd4737105777d75f7036f58a9e888bb908bcd29f0a4262cfd47222da8617af56320771f8b8c5c9accbe8ac317b4f26687250329039bb9b19160f77e349193a74cb43ebfa87f6b2ab1d1b442977123534bab6b4a2e45c17878fa9da9beaeda5ebaa36035109c97ffffffffffffff0118e807")),
    (0, "8181741433377617409319254499207074137484777183417047187463e-14p1", Ok("1a080a01011092011801")),
    (0, "-8452984.153e22p53", Ok("1a0f0a071112172335585b102c18352001")),
    (0, "7e-2595p53", Ok("1a160a07152b847892a30010a2bcffffffffffffff011835")),
    (0, "15055613.0445166915640e-221p300", Ok("1a360a260cf9a93a57299560a10763d9e0a03fe0afd76766e5e5e9836d60bf807d57337f8d27d4c6229b108ef8ffffffffffffff0118ac02")),
    (0, "-4685.356068831067e-211p64", Ok("1a190a089a08f9b872dc12591090faffffffffffffff0118402001")),
    (0, "822765444463478612213878375762833.125815354930666836450749528e-1362p1", Ok("1a100a010110c1ddffffffffffffff011801")),
    (0, "66764020679701.68e-9p1", Ok("1a070a010110101801")),
    (0, "0.8171800239e77p2", Ok("1a080a010310fe011802")),
    (0, "6111489364390084.745378383064003761e2052p24", Ok("1a0a0a038350d510be351818")),
    (0, "565e-115p1000", Ok("1a8d010a7d8b2369e1ca627de43c4cb8eaab1e57243d82b3aa2ea1beb4172ab53e1e243a79ca0566be8ecca725a37e8c0d8b3489958642610d3ed752eb817933ca2a36ed4bd4f52e7ade8371449abb31a00f367f8323519644e3f488417eb9379f021208f2de9382efa6d7e18099c152d42a3da4cd80912f8c128ccaf598db545a7a10a4f5ffffffffffffff0118e807")),
    (0, "-3.7343e-558p8", Ok("1a120a019a10bdf1ffffffffffffff0118082001")),
    (0, "79237609260309260.02675112532870465572101e1984p1", Ok("1a080a010110f7331801")),
    (0, "3654610735873557.063606071043195545904544091031776e23p2", Ok("1a070a0102107f1802")),
    (0, "042.935575918e-2280p3", Ok("1a100a010510edc4ffffffffffffff011803")),
    (0, "0788526114913167723267938144494544373.23324e-3839p53", Ok("1a160a07149a037d44b51810f29cffffffffffffff011835")),
    (0, "8.831701073750431e4655p3", Ok("1a080a010710e8781803")),
    (0, "827940199530552034093.056e215p1", Ok("1a080a01011090061801")),
    (0, "78716168281264644604956605366306261169572200826.1995e-2510p24", Ok("1a120a03d6a61710f2bfffffffffffffff011818")),
    (0, "211673420750961.9239693729306826512e-16", Ok("1a300a20ad67223e95c61138ec95b054f90c7056c39cd7737760799582e6c071d23f085910fbfdffffffffffffff01188002")),
    (0, "-051697846493665787200977379727611256517.880021581086201e-26p3", Ok("1a090a0104102518032001")),
    (0, "74231594259472.4873949835503262456241805788914865465925517e-4109p8", Ok("1a100a019b10d595ffffffffffffff011808")),
    (0, "4.9500324966850273e-4858p256", Ok("1a300a20a6a8a22b5cb4d875d5eeb24bc8937b101a0dc795b649ec2f2050ce2c4388ed1810f9fffeffffffffffff01188002")),
    (0, "-85.83694e2880p2", Ok("1a0a0a010310e44a18022001")),
    (0, "327112.464008599798732000806232010983263898692841407806671723e-4265p1", Ok("1a100a010110ba91ffffffffffffff011801")),
    (0, "-4048684431802433253659.36877800639436991922001192e-37", Ok("1a320a20e96409540fec1efa72958a4ee989cec90045a176f35b919fa965cea578027a0010cdfdffffffffffffff011880022001")),
    (0, "010195381613331001471213455640544.055987490606e4p64", Ok("1a0e0a089d15b0998a293bb810351840")),
    (0, "93194260834005717279.584924337211781e1674p53", Ok("1a0e0a0712f47a321a099810c72b1835")),
    (0, "1605344688263728990595.82785277493257383449223598e475p3", Ok("1a080a010510ee0c1803")),
    (0, "12136224464311436700663847.024960369963932176541e1655p1000", Ok("1a85010a7d8af597dce9df802e7f3d1dad89dcc60438b8f1e9e381b32992b5431786f2ceb92e74e44c1d7171082c3d315c23f5c0b41a087f96d55c22ba33a600b5d407296485af8a841f347fab3dd2f2ae4f479538285035d8fcad790eeb0cff092137b7fe27740b742816418b8f88986dd0241e943a7dc95cd16f144e846b962cb510e62318e807")),
    (0, "467709.68250e-27", Ok("1a300a208d5b48af03c495d64db4b1dc25a7f3c2a220f484ad9f0880fffc2d58c407e18610bafdffffffffffffff01188002")),
    (0, "23851.978378058e2586p3", Ok("1a080a0104109b431803")),
    (0, "6.81950446600e1990p300", Ok("1a2e0a260a9ab28ffddf0a907f4aea7450aad39345012d81ebed4c5584fd687750b6fea7142dd0be196310aa3118ac02")),
    (0, "94134683673221378.325674e2790p24", Ok("1a0a0a03bd5b3f10d5481818")),
    (0, "46.4627045345776e1038p2", Ok("1a080a010310fc1a1802")),
    (0, "6019528590031449192327523682991.843738171814632778077273e-4792p2", Ok("1a100a010310b684ffffffffffffff011802")),
    (0, "97475661250090518.7720362515578e-14p8", Ok("1a070a01f410021808")),
    (0, "-480445765848537153.5429106868e21p1", Ok("1a0a0a010110800118012001")),
];

#[test]
fn a_big_float_literal_reads_as_protowire_go_reads_it() {
    let pool = DescriptorPool::decode(&include_bytes!("../testdata/bignum-test.binpb")[..])
        .expect("bignum-test.binpb");
    let desc = pool
        .get_message_by_name("bignum_test.v1.BigNumDemo")
        .expect("BigNumDemo");
    let mut checked = 0;
    for &(max_digits, lit, want) in ROWS {
        let mut limits = Limits::default();
        if max_digits != 0 {
            limits.max_numeric_literal_digits = max_digits;
        }
        let start = Instant::now();
        let got = unmarshal(
            &format!("big_float_field = {lit}"),
            &desc,
            UnmarshalOptions {
                limits,
                ..Default::default()
            },
        )
        .map(|m| {
            m.encode_to_vec()
                .iter()
                .map(|b| format!("{b:02x}"))
                .collect::<String>()
        })
        .map_err(|e| e.to_string());
        let took = start.elapsed();
        assert_eq!(
            got.as_ref().map(String::as_str).map_err(String::as_str),
            want,
            "{lit}"
        );
        // #109: under a raised digit cap, each is decided in seconds, not
        // the minutes a Parse at the suffix's precision took.
        assert!(took < Duration::from_secs(5), "{lit} took {took:?}");
        checked += 1;
    }
    assert_eq!(checked, 100);
}
