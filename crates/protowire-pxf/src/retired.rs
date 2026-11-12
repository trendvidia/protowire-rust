// SPDX-License-Identifier: MIT
// Copyright (c) 2026 TrendVidia, LLC.
//! Retired option numbers (STABILITY.md promise 3, #133). Mirrors
//! protowire-go's `encoding/pxf/retired.go`.
//!
//! Before protowire v1.12.0 the family's extensions lived in an unregistered
//! 50000-59999 range. They moved to the registered block 1314-1363 in one
//! change (protowire#244), and the old numbers are dead. A descriptor
//! compiled before the move still carries them, and a reader that looks only
//! at the registered numbers sees none of its annotations: `(pxf.required)`
//! stops being enforced, `(pxf.default)` stops applying, and the schema binds
//! documents it should reject. Draft -01 says an implementation that meets
//! them should diagnose the descriptor as pre-registration, so bind-time
//! validation reports each one as [`ViolationKind::RetiredNumber`].
//!
//! The check is scoped two ways, because 50000 and up is where every project
//! that never registered its numbers puts them. A retired number counts only
//! on the options kind the retired allocation used, and only in a file that
//! imports one of protowire's own annotation files, which a schema needs in
//! order to have written the option at all.

use prost_reflect::{DynamicMessage, FileDescriptor, MessageDescriptor};

use crate::annotations::AnnotationSurface;
use crate::schema::{Violation, ViolationKind};

/// What a retired option number used to be and where it went.
struct Retired {
    number: u32,
    /// The option as it was spelled.
    was: &'static str,
    /// Its number in the registered block.
    now: u32,
    /// The annotations file to recompile against.
    file: &'static str,
}

const fn retired(number: u32, was: &'static str, now: u32, file: &'static str) -> Retired {
    Retired {
        number,
        was,
        now,
        file,
    }
}

const SBE: &str = "sbe/annotations.proto";
const PXF: &str = "pxf/annotations.proto";
const SCHEMA: &str = "protowire/schema/v1/descriptor.proto";
const PROTOCHECK: &str = "protocheck's annotations";

const FILE_OPTIONS: &[Retired] = &[
    retired(50100, "(sbe.schema_id)", 1319, SBE),
    retired(50101, "(sbe.version)", 1320, SBE),
    retired(50400, "the file_annotations carrier", 1327, SCHEMA),
    retired(50401, "the functions carrier", 1328, SCHEMA),
    retired(50402, "the annotation_decls carrier", 1329, SCHEMA),
    retired(50403, "the type_decls carrier", 1330, SCHEMA),
    retired(50404, "the source_map carrier", 1331, SCHEMA),
];

const MESSAGE_OPTIONS: &[Retired] = &[
    retired(50200, "(sbe.template_id)", 1321, SBE),
    retired(50400, "the message_annotations carrier", 1327, SCHEMA),
    // protocheck's kinds are inferred from the registered names (field,
    // message, oneof); its declarations are not in the protowire repository.
    retired(51001, "a protocheck message constraint", 1348, PROTOCHECK),
];

const FIELD_OPTIONS: &[Retired] = &[
    retired(50000, "(pxf.required)", 1314, PXF),
    retired(50001, "(pxf.default)", 1315, PXF),
    retired(50002, "(pxf.key)", 1316, PXF),
    retired(50300, "(sbe.length)", 1322, SBE),
    retired(50301, "(sbe.encoding)", 1323, SBE),
    retired(50400, "the field_annotations carrier", 1327, SCHEMA),
    retired(51000, "a protocheck field constraint", 1347, PROTOCHECK),
];

const ONEOF_OPTIONS: &[Retired] = &[
    retired(50400, "the oneof_annotations carrier", 1327, SCHEMA),
    retired(51002, "a protocheck oneof constraint", 1349, PROTOCHECK),
];

/// Enum, enum value, service and method options carried only the
/// annotation carrier.
const CARRIER_ONLY: &[Retired] = &[retired(50400, "the *_annotations carrier", 1327, SCHEMA)];

/// The imports that mark a file as compiled against protowire's
/// annotations, in either era.
const PROTOWIRE_ANNOTATION_FILES: &[&str] = &[
    "pxf/annotations.proto",
    "sbe/annotations.proto",
    "protowire/schema/v1/annotations.proto",
    "protowire/schema/v1/descriptor.proto",
];

/// Whether `fd` imports one of protowire's annotation files.
pub(crate) fn imports_protowire(fd: &FileDescriptor) -> bool {
    fd.dependencies()
        .any(|dep| PROTOWIRE_ANNOTATION_FILES.contains(&dep.name()))
}

/// Appends a violation for every retired option number the file's elements
/// carry: protowire-go's `walkRetired`, in its order.
pub(crate) fn walk_retired(path: &str, fd: &FileDescriptor, out: &mut Vec<Violation>) {
    retired_in(path, path, "FileOptions", &fd.options(), FILE_OPTIONS, out);
    for md in fd.messages() {
        walk_message(path, &md, out);
    }
    for en in fd.enums() {
        walk_enum(path, &en, out);
    }
    for svc in fd.services() {
        let opts = svc.options();
        retired_in(
            path,
            svc.full_name(),
            "ServiceOptions",
            &opts,
            CARRIER_ONLY,
            out,
        );
        for m in svc.methods() {
            let opts = m.options();
            retired_in(
                path,
                m.full_name(),
                "MethodOptions",
                &opts,
                CARRIER_ONLY,
                out,
            );
        }
    }
}

fn walk_message(path: &str, md: &MessageDescriptor, out: &mut Vec<Violation>) {
    let opts = md.options();
    retired_in(
        path,
        md.full_name(),
        "MessageOptions",
        &opts,
        MESSAGE_OPTIONS,
        out,
    );
    for f in md.fields() {
        let opts = f.options();
        retired_in(
            path,
            f.full_name(),
            "FieldOptions",
            &opts,
            FIELD_OPTIONS,
            out,
        );
    }
    for o in md.oneofs() {
        let opts = o.options();
        retired_in(
            path,
            o.full_name(),
            "OneofOptions",
            &opts,
            ONEOF_OPTIONS,
            out,
        );
    }
    for en in md.child_enums() {
        walk_enum(path, &en, out);
    }
    for nested in md.child_messages() {
        walk_message(path, &nested, out);
    }
}

fn walk_enum(path: &str, en: &prost_reflect::EnumDescriptor, out: &mut Vec<Violation>) {
    retired_in(
        path,
        en.full_name(),
        "EnumOptions",
        &en.options(),
        CARRIER_ONLY,
        out,
    );
    for v in en.values() {
        let opts = v.options();
        retired_in(
            path,
            v.full_name(),
            "EnumValueOptions",
            &opts,
            CARRIER_ONLY,
            out,
        );
    }
}

/// Appends a violation for each of `opts`'s unknown fields whose number is
/// in `table`. Only unknown fields are read: nothing declares the retired
/// numbers any more, so that is where they land.
fn retired_in(
    path: &str,
    element: &str,
    kind: &str,
    opts: &DynamicMessage,
    table: &[Retired],
    out: &mut Vec<Violation>,
) {
    for field in opts.unknown_fields() {
        let Some(r) = table.iter().find(|r| r.number == field.number()) else {
            continue;
        };
        out.push(Violation {
            file: path.to_string(),
            element: element.to_string(),
            name: r.number.to_string(),
            kind: ViolationKind::RetiredNumber,
            detail: format!(
                "option number {} on {kind} was {} before protowire v1.12.0 and is {} now; the descriptor predates the registered extension block and must be recompiled against the current {}",
                r.number, r.was, r.now, r.file
            ),
            surface: AnnotationSurface::Option,
        });
    }
}
