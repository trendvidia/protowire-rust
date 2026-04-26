//! `protowire` CLI — Rust port of `protowire/cmd/protowire/main.go`.
//!
//! Subcommands: `encode`, `decode`, `validate`, `fmt`. The schema source is
//! a pre-compiled `FileDescriptorSet` binary (`-d <file.binpb>`), produced
//! by e.g. `protoc --include_imports --descriptor_set_out=schema.binpb` or
//! `buf build --as-file-descriptor-set`. The Go CLI's `--proto` (compile
//! from sources) and `--server` (protoregistry gRPC client) modes are out
//! of scope for this port.
//!
//! [`run`] takes argv plus a `read_file` callback and returns a [`CliResult`]
//! — no direct stdio touched — so tests can drive it as a pure function.
//! The `protowire` binary is a thin wrapper that streams the result.
//!
//! Mirrors the TS port's `src/cli/main.ts` line-for-line.

use std::io;

use prost::Message as _;
use prost_reflect::{DescriptorPool, DynamicMessage, MessageDescriptor};
use protowire_pxf::{marshal, unmarshal, MarshalOptions, UnmarshalOptions};

pub const USAGE: &str = "usage: protowire <command> -d <descriptor.binpb> -m <message-name> [args...]

commands:
  encode <file.pxf>     PXF text  -> protobuf binary (stdout)
  decode <file.pb>      protobuf binary -> PXF text (stdout)
  validate <file.pxf>   parse PXF and report success / error
  fmt <file.pxf>        round-trip PXF (decode + encode) and write to stdout

flags:
  -d, --descriptor-set <file>  binary FileDescriptorSet (FDSet) produced by
                                protoc --descriptor_set_out
  -m, --message <name>         fully-qualified message name (e.g. test.v1.AllTypes)
";

#[derive(Debug, Clone)]
pub struct CliResult {
    pub stdout: Vec<u8>,
    pub stderr: String,
    pub exit: i32,
}

/// Run the CLI dispatcher. `read_file` resolves any file paths the user
/// supplies (descriptor set + input file). Returns a [`CliResult`] capturing
/// stdout bytes, stderr text, and the exit code.
pub fn run<F>(argv: &[&str], read_file: F) -> CliResult
where
    F: Fn(&str) -> io::Result<Vec<u8>>,
{
    if argv.is_empty() {
        return CliResult {
            stdout: Vec::new(),
            stderr: USAGE.to_string(),
            exit: 1,
        };
    }
    if argv[0] == "-h" || argv[0] == "--help" {
        return CliResult {
            stdout: Vec::new(),
            stderr: USAGE.to_string(),
            exit: 0,
        };
    }

    let cmd = argv[0];
    let parsed = match parse_flags(&argv[1..]) {
        Ok(p) => p,
        Err(e) => {
            return fail(format!("error: {}\n\n{}", e, USAGE));
        }
    };

    if parsed.help {
        return CliResult {
            stdout: Vec::new(),
            stderr: USAGE.to_string(),
            exit: 0,
        };
    }

    let Some(desc_path) = parsed.descriptor_set.as_deref() else {
        return fail("error: -d/--descriptor-set is required\n".to_string());
    };
    let Some(msg_name) = parsed.message.as_deref() else {
        return fail("error: -m/--message is required\n".to_string());
    };
    if parsed.positionals.len() != 1 {
        return fail(format!(
            "error: {} expects exactly one input file argument\n",
            cmd
        ));
    }
    let input_path = &parsed.positionals[0];

    let target: MessageDescriptor;
    let pool: DescriptorPool;
    match read_file(desc_path) {
        Ok(bytes) => match DescriptorPool::decode(bytes.as_slice()) {
            Ok(p) => {
                pool = p;
                match pool.get_message_by_name(msg_name) {
                    Some(m) => target = m,
                    None => {
                        return fail(format!(
                            "error: message {} not found in descriptor set\n",
                            msg_name
                        ));
                    }
                }
            }
            Err(e) => return fail(format!("error: load descriptor: {}\n", e)),
        },
        Err(e) => return fail(format!("error: load descriptor: {}\n", e)),
    }

    let input_data = match read_file(input_path) {
        Ok(b) => b,
        Err(e) => return fail(format!("error: read {}: {}\n", input_path, e)),
    };

    match cmd {
        "encode" => run_encode(&input_data, &target),
        "decode" => run_decode(&input_data, &target),
        "validate" => run_validate(&input_data, &target),
        "fmt" => run_fmt(&input_data, &target, msg_name),
        other => fail(format!("error: unknown command {:?}\n\n{}", other, USAGE)),
    }
}

fn run_encode(input: &[u8], target: &MessageDescriptor) -> CliResult {
    let text = match std::str::from_utf8(input) {
        Ok(s) => s,
        Err(e) => return fail(format!("error: {}\n", e)),
    };
    match unmarshal(text, target, UnmarshalOptions::default()) {
        Ok(msg) => CliResult {
            stdout: msg.encode_to_vec(),
            stderr: String::new(),
            exit: 0,
        },
        Err(e) => fail(format!("error: {}\n", e)),
    }
}

fn run_decode(input: &[u8], target: &MessageDescriptor) -> CliResult {
    match DynamicMessage::decode(target.clone(), input) {
        Ok(msg) => CliResult {
            stdout: marshal(&msg, target, MarshalOptions::default()).into_bytes(),
            stderr: String::new(),
            exit: 0,
        },
        Err(e) => fail(format!("error: {}\n", e)),
    }
}

fn run_validate(input: &[u8], target: &MessageDescriptor) -> CliResult {
    let text = match std::str::from_utf8(input) {
        Ok(s) => s,
        Err(e) => return fail(format!("error: {}\n", e)),
    };
    match unmarshal(text, target, UnmarshalOptions::default()) {
        Ok(_) => CliResult {
            stdout: Vec::new(),
            stderr: "valid\n".to_string(),
            exit: 0,
        },
        Err(e) => fail(format!("error: {}\n", e)),
    }
}

fn run_fmt(input: &[u8], target: &MessageDescriptor, type_url: &str) -> CliResult {
    let text = match std::str::from_utf8(input) {
        Ok(s) => s,
        Err(e) => return fail(format!("error: {}\n", e)),
    };
    match unmarshal(text, target, UnmarshalOptions::default()) {
        Ok(msg) => CliResult {
            stdout: marshal(
                &msg,
                target,
                MarshalOptions {
                    type_url: Some(type_url),
                    ..Default::default()
                },
            )
            .into_bytes(),
            stderr: String::new(),
            exit: 0,
        },
        Err(e) => fail(format!("error: {}\n", e)),
    }
}

fn fail(stderr: String) -> CliResult {
    CliResult {
        stdout: Vec::new(),
        stderr,
        exit: 1,
    }
}

#[derive(Debug, Default)]
struct ParsedFlags {
    descriptor_set: Option<String>,
    message: Option<String>,
    help: bool,
    positionals: Vec<String>,
}

/// Parse the small flag set we accept (`-d`/`--descriptor-set`,
/// `-m`/`--message`, `-h`/`--help`), allowing space-separated and
/// `=`-separated values. Anything not matching becomes a positional.
fn parse_flags(args: &[&str]) -> Result<ParsedFlags, String> {
    let mut out = ParsedFlags::default();
    let mut i = 0;
    while i < args.len() {
        let a = args[i];
        if a == "-h" || a == "--help" {
            out.help = true;
            i += 1;
            continue;
        }
        if a == "-d" || a == "--descriptor-set" {
            i += 1;
            if i >= args.len() {
                return Err(format!("missing value for {}", a));
            }
            out.descriptor_set = Some(args[i].to_string());
            i += 1;
            continue;
        }
        if let Some(rest) = a.strip_prefix("--descriptor-set=") {
            out.descriptor_set = Some(rest.to_string());
            i += 1;
            continue;
        }
        if a == "-m" || a == "--message" {
            i += 1;
            if i >= args.len() {
                return Err(format!("missing value for {}", a));
            }
            out.message = Some(args[i].to_string());
            i += 1;
            continue;
        }
        if let Some(rest) = a.strip_prefix("--message=") {
            out.message = Some(rest.to_string());
            i += 1;
            continue;
        }
        if a.starts_with('-') && a.len() > 1 {
            return Err(format!("unknown flag {:?}", a));
        }
        out.positionals.push(a.to_string());
        i += 1;
    }
    Ok(out)
}
