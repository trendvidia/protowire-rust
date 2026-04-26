// Dumps a canonical Envelope's pb-encoded bytes as hex, for cross-port
// wire-compat checking. Mirrors:
//   protowire/scripts/dump_envelope/main.go
//   protowire4cpp/cmd/dump_envelope/main.cc
//   protowire4ts/scripts/dump-envelope.ts
//   protowire4java/dump-envelope
//
// Slice 0 stub. Slice 1 (envelope) replaces this with real envelope::err()
// + pb::marshal() output.

fn main() {
    eprintln!("dump-envelope: not yet implemented (lands in Slice 1: envelope)");
    std::process::exit(2);
}
