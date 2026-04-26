//! End-to-end tests for the protowire CLI dispatcher. Drives `run()`
//! directly with an in-memory `read_file` stub so we can fold the on-disk
//! testdata fixture (`test.binpb`) back through the CLI without spawning
//! a subprocess. Mirrors the TS port's `src/cli/main.test.ts`.

use std::collections::HashMap;
use std::io;

use prost::Message as _;
use prost_reflect::{DescriptorPool, DynamicMessage, MessageDescriptor, Value};
use protowire_cli::{run, CliResult};

const TEST_FDS: &[u8] = include_bytes!("../../protowire-pxf/testdata/test.binpb");
const SCHEMA_PATH: &str = "schema.binpb";

fn pool() -> DescriptorPool {
    DescriptorPool::decode(TEST_FDS).expect("decode test.binpb")
}

fn all_types() -> MessageDescriptor {
    pool()
        .get_message_by_name("test.v1.AllTypes")
        .expect("missing test.v1.AllTypes")
}

/// Build a `read_file` stub backed by an in-memory map. The descriptor
/// fixture is always served regardless of the map, mirroring the TS test
/// helper that hardcodes `schema.binpb`.
fn make_read_file(
    files: HashMap<&'static str, Vec<u8>>,
) -> impl Fn(&str) -> io::Result<Vec<u8>> {
    move |path: &str| -> io::Result<Vec<u8>> {
        if path == SCHEMA_PATH {
            return Ok(TEST_FDS.to_vec());
        }
        files
            .get(path)
            .cloned()
            .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, format!("ENOENT: {}", path)))
    }
}

fn run_with(argv: &[&str], files: HashMap<&'static str, Vec<u8>>) -> CliResult {
    run(argv, make_read_file(files))
}

const BASE_ARGS: &[&str] = &["-d", SCHEMA_PATH, "-m", "test.v1.AllTypes"];

// ---------------- argument handling ----------------

#[test]
fn no_args_prints_usage_with_exit_1() {
    let r = run(&[], |_| panic!("should not read"));
    assert_eq!(r.exit, 1);
    assert!(r.stderr.contains("usage: protowire"), "{}", r.stderr);
}

#[test]
fn help_prints_usage_with_exit_0() {
    let r = run(&["--help"], |_| Ok(Vec::new()));
    assert_eq!(r.exit, 0);
    assert!(r.stderr.contains("usage: protowire"), "{}", r.stderr);
}

#[test]
fn missing_descriptor_set_flag_errors() {
    let mut files = HashMap::new();
    files.insert("in.pxf", Vec::new());
    let r = run_with(&["encode", "-m", "test.v1.AllTypes", "in.pxf"], files);
    assert_eq!(r.exit, 1);
    assert!(
        r.stderr.contains("--descriptor-set is required"),
        "{}",
        r.stderr
    );
}

#[test]
fn missing_message_flag_errors() {
    let mut files = HashMap::new();
    files.insert("in.pxf", Vec::new());
    let r = run_with(&["encode", "-d", SCHEMA_PATH, "in.pxf"], files);
    assert_eq!(r.exit, 1);
    assert!(r.stderr.contains("--message is required"), "{}", r.stderr);
}

#[test]
fn unknown_command_errors() {
    let mut files = HashMap::new();
    files.insert("in.pxf", Vec::new());
    let mut argv = vec!["bogus"];
    argv.extend_from_slice(BASE_ARGS);
    argv.push("in.pxf");
    let r = run_with(&argv, files);
    assert_eq!(r.exit, 1);
    assert!(
        r.stderr.contains(r#"unknown command "bogus""#),
        "{}",
        r.stderr
    );
}

#[test]
fn unknown_message_name_errors() {
    let mut files = HashMap::new();
    files.insert("in.pxf", Vec::new());
    let r = run_with(
        &["encode", "-d", SCHEMA_PATH, "-m", "test.v1.NoSuch", "in.pxf"],
        files,
    );
    assert_eq!(r.exit, 1);
    assert!(
        r.stderr.contains("not found in descriptor set"),
        "{}",
        r.stderr
    );
}

#[test]
fn missing_input_file_errors() {
    let mut argv = vec!["encode"];
    argv.extend_from_slice(BASE_ARGS);
    argv.push("missing.pxf");
    let r = run_with(&argv, HashMap::new());
    assert_eq!(r.exit, 1);
    assert!(r.stderr.contains("read missing.pxf"), "{}", r.stderr);
}

// ---------------- encode ----------------

#[test]
fn encode_pxf_text_to_protobuf_binary() {
    let mut files = HashMap::new();
    files.insert(
        "in.pxf",
        b"string_field = \"hi\"\nint32_field = 42".to_vec(),
    );
    let mut argv = vec!["encode"];
    argv.extend_from_slice(BASE_ARGS);
    argv.push("in.pxf");
    let r = run_with(&argv, files);
    assert_eq!(r.exit, 0, "stderr: {}", r.stderr);
    assert!(r.stderr.is_empty());

    let target = all_types();
    let msg = DynamicMessage::decode(target.clone(), r.stdout.as_slice()).expect("decode binary");
    let s_fd = target.get_field_by_name("string_field").unwrap();
    let i_fd = target.get_field_by_name("int32_field").unwrap();
    assert!(matches!(msg.get_field(&s_fd).into_owned(), Value::String(s) if s == "hi"));
    assert!(matches!(msg.get_field(&i_fd).into_owned(), Value::I32(42)));
}

#[test]
fn encode_reports_decode_errors_with_exit_1() {
    let mut files = HashMap::new();
    files.insert("in.pxf", b"bogus_field = 1".to_vec());
    let mut argv = vec!["encode"];
    argv.extend_from_slice(BASE_ARGS);
    argv.push("in.pxf");
    let r = run_with(&argv, files);
    assert_eq!(r.exit, 1);
    assert!(r.stderr.contains("unknown field"), "{}", r.stderr);
}

// ---------------- decode ----------------

#[test]
fn decode_protobuf_binary_to_pxf_text() {
    let target = all_types();
    let mut msg = DynamicMessage::new(target.clone());
    let s_fd = target.get_field_by_name("string_field").unwrap();
    let i_fd = target.get_field_by_name("int32_field").unwrap();
    msg.set_field(&s_fd, Value::String("world".into()));
    msg.set_field(&i_fd, Value::I32(7));
    let bin = msg.encode_to_vec();

    let mut files = HashMap::new();
    files.insert("in.pb", bin);
    let mut argv = vec!["decode"];
    argv.extend_from_slice(BASE_ARGS);
    argv.push("in.pb");
    let r = run_with(&argv, files);
    assert_eq!(r.exit, 0, "stderr: {}", r.stderr);
    let text = std::str::from_utf8(&r.stdout).expect("utf8");
    assert!(text.contains(r#"string_field = "world""#), "{text}");
    assert!(text.contains("int32_field = 7"), "{text}");
}

// ---------------- validate ----------------

#[test]
fn validate_prints_valid_on_well_formed_input() {
    let mut files = HashMap::new();
    files.insert("in.pxf", b"string_field = \"ok\"".to_vec());
    let mut argv = vec!["validate"];
    argv.extend_from_slice(BASE_ARGS);
    argv.push("in.pxf");
    let r = run_with(&argv, files);
    assert_eq!(r.exit, 0);
    assert!(r.stdout.is_empty());
    assert_eq!(r.stderr, "valid\n");
}

#[test]
fn validate_reports_parse_errors_with_nonzero_exit() {
    let mut files = HashMap::new();
    files.insert("in.pxf", b"int32_field = \"not a number\"".to_vec());
    let mut argv = vec!["validate"];
    argv.extend_from_slice(BASE_ARGS);
    argv.push("in.pxf");
    let r = run_with(&argv, files);
    assert_eq!(r.exit, 1);
    assert!(r.stderr.contains("expected integer"), "{}", r.stderr);
}

// ---------------- fmt ----------------

#[test]
fn fmt_normalizes_pxf_and_prepends_at_type() {
    let mut files = HashMap::new();
    files.insert("in.pxf", b"int32_field=42\nstring_field=\"hi\"".to_vec());
    let mut argv = vec!["fmt"];
    argv.extend_from_slice(BASE_ARGS);
    argv.push("in.pxf");
    let r = run_with(&argv, files);
    assert_eq!(r.exit, 0, "stderr: {}", r.stderr);
    let out = std::str::from_utf8(&r.stdout).expect("utf8");
    assert!(
        out.starts_with("@type test.v1.AllTypes\n\n"),
        "out: {out:?}"
    );
    assert!(out.contains(r#"string_field = "hi""#), "{out}");
    assert!(out.contains("int32_field = 42"), "{out}");
}
