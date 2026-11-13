// SPDX-License-Identifier: MIT
// Copyright (c) 2026 TrendVidia, LLC.
//! Writing `@dataset` tables (draft §3.4.4): the header and one row per
//! message. Mirrors protowire-go's `encoding/pxf/dataset_write.go`. Cells are
//! rendered by the encoder [`crate::marshal`] uses, so a cell reads back
//! exactly as the same field would in a `marshal` document.
//!
//! A row depends only on its message and the column list, never on the rows
//! before it, so appending rows to a file that already holds the header
//! needs no rewrite of earlier bytes.

use std::borrow::Cow;
use std::collections::HashSet;

use prost_reflect::{
    DynamicMessage, FieldDescriptor, Kind, MessageDescriptor, ReflectMessage, Value,
};

use super::{declaration_order, is_negative_zero, read_null_mask, Encoder, MarshalOptions};
use crate::annotations::find_null_mask_field;
use crate::dataset_paths::check_column_paths;
use crate::errors::PxfError;
use crate::limits::MAX_NESTING_DEPTH;
use crate::post_decode_plan::{post_decode_plan, PlanField};
use crate::schema::{as_validation_error_message, validate_descriptor};
use crate::token::Position;

/// Append the header `@dataset <type> (<columns>)` for rows of type `desc`,
/// and a newline, to `dst`.
///
/// Any field of `desc` can be a column: a cell may hold a list or a block
/// (protowire#341). A column may also be a path into a sub-message, such as
/// `inner.x` (protowire#342), through singular message fields only. It
/// returns an error, and `dst` unchanged, for an empty column list and for a
/// column the reader would refuse: a name that is not a field, a path with
/// an empty segment, a column twice, a field beside a path inside it
/// (`inner` with `inner.x`), or a path through a repeated or map field or
/// into a type written as a literal. A `(pxf.required)` field of `desc` that
/// is neither a column nor under one is an error too: every row would be
/// rejected on reading. So is a schema that [`crate::validate_descriptor`]
/// refuses, which [`crate::unmarshal`] would refuse to read.
///
/// A path deeper than [`MAX_NESTING_DEPTH`] is refused as well, because
/// this port's reader refuses it (protowire-go's writer writes it).
pub fn append_dataset_header<S: AsRef<str>>(
    dst: &mut String,
    desc: &MessageDescriptor,
    columns: &[S],
) -> Result<(), PxfError> {
    dataset_columns(desc, columns)?;
    dst.push_str("@dataset ");
    dst.push_str(desc.full_name());
    dst.push_str(" (");
    for (i, c) in columns.iter().enumerate() {
        if i > 0 {
            dst.push_str(", ");
        }
        dst.push_str(c.as_ref());
    }
    dst.push_str(")\n");
    Ok(())
}

/// Append `msg` as one row of a `@dataset` table with these columns, and a
/// newline, to `dst`. Use the columns given to [`append_dataset_header`].
///
/// A set field is written as [`crate::marshal`] writes its value. An unset
/// field is an empty cell, which reads back as absent, except where reading
/// an empty cell would not give back the unset field:
///
/// - A field with `(pxf.default)` would take the default. A proto3 scalar
///   without presence is written as its zero value, which the default does
///   not replace. A message-typed field (a wrapper, a Timestamp, ...) is
///   written `null`, which keeps the default off and leaves the field
///   unset. A scalar with presence (`optional`, or a oneof member) stays
///   empty and reads back as its default, since an absent field with a
///   default means the default (protowire-go#146). A oneof member stays
///   empty when another member of its oneof is set and among the columns,
///   since that member keeps the default off. A set member that is not a
///   column is not written, so the oneof is treated as having no member
///   set.
/// - A `(pxf.required)` field would be rejected. A proto3 scalar is written
///   as its zero value, an empty repeated field `[]` and an empty map `{}`,
///   which the reader counts as supplied; an unset field with presence is
///   an error.
///
/// A message-typed field named in the message's `_null` `FieldMask` is
/// written `null`, as `marshal` writes it. A scalar field named there is
/// written as if the mask did not name it, since the draft rejects `null`
/// on a scalar field (trendvidia/protowire#339).
///
/// A dotted column is written through its parents: an unset parent makes
/// the cell empty, and the leaf follows the rules above inside its parent.
/// A sub-message is present on reading exactly when a cell under it is
/// non-empty (protowire#342), so one that is set but has every column under
/// it empty reads back unset; that loss is accepted, as for a field outside
/// the columns. A row that the reader would reject for it is refused
/// instead: a required sub-message whose cells are all empty, or a
/// sub-message that reads back present while one of its required fields is
/// not a column.
///
/// Fields not among the columns are not written, so a table can export a
/// subset of a message's fields. They read back absent, taking their
/// `(pxf.default)` if they have one. A `(pxf.required)` field cannot be
/// left out: the column list is refused.
///
/// A list, map or message that needs a block is written as `marshal` writes
/// it, on one line, so a table keeps one row per line. Inside it, an unset
/// field with `(pxf.default)` or `(pxf.required)` follows the rules above,
/// since the reader applies both in every body of a cell
/// (protowire-go#161).
///
/// It returns an error, and `dst` unchanged, for the column errors of
/// [`append_dataset_header`] and for a value `marshal` refuses.
pub fn append_dataset_row<S: AsRef<str>>(
    dst: &mut String,
    msg: &DynamicMessage,
    columns: &[S],
) -> Result<(), PxfError> {
    let desc = msg.descriptor();
    let cols = dataset_columns(&desc, columns)?;
    // protowire-go's writer: no indent, no Any sugar, and a null set read
    // from the message alone. The `_null` field is not skipped, since a
    // column names what is written.
    let mut enc = Encoder::new(MarshalOptions {
        indent: "",
        ..Default::default()
    });
    if let Some(nfd) = find_null_mask_field(&desc) {
        if msg.has_field(&nfd) {
            enc.null_set = read_null_mask(msg, &nfd);
        }
    }
    // Path prefixes with a non-empty cell.
    let mut filled: HashSet<String> = HashSet::new();
    enc.buf.push('(');
    for (i, c) in cols.iter().enumerate() {
        if i > 0 {
            enc.buf.push_str(", ");
        }
        let Some(parent) = c.parent_in(msg) else {
            // A parent is unset: an empty cell, and it reads back unset.
            continue;
        };
        let leaf = &c.path[c.path.len() - 1];
        let before = enc.buf.len();
        let plan = post_decode_plan(&parent.descriptor());
        enc.write_cell(
            &parent,
            leaf,
            &c.name,
            &c.siblings,
            cell_options(&plan, leaf),
        )
        .map_err(|e| error(format!("pxf: @dataset column {:?}: {e}", c.name)))?;
        if enc.buf.len() > before {
            filled.extend(c.prefixes());
        }
    }
    enc.buf.push_str(")\n");
    check_row_presence(&desc, &cols, &filled).map_err(error)?;
    dst.push_str(&enc.buf);
    Ok(())
}

fn error(msg: String) -> PxfError {
    PxfError::new(Position::default(), msg)
}

/// One resolved column: its name as written and the fields from the row
/// type down to the leaf (one for a plain column).
struct DatasetColumn {
    name: String,
    path: Vec<FieldDescriptor>,
    /// The leaves of the columns with the same parent, this one included.
    siblings: Vec<FieldDescriptor>,
}

impl DatasetColumn {
    /// The message holding the leaf, walking the path in `m`; `None` when a
    /// parent on the way is unset.
    fn parent_in<'m>(&self, m: &'m DynamicMessage) -> Option<Cow<'m, DynamicMessage>> {
        let mut at = Cow::Borrowed(m);
        for fd in &self.path[..self.path.len() - 1] {
            if !at.has_field(fd) {
                return None;
            }
            at = match at {
                Cow::Borrowed(m) => match m.get_field(fd) {
                    Cow::Borrowed(Value::Message(sub)) => Cow::Borrowed(sub),
                    Cow::Owned(Value::Message(sub)) => Cow::Owned(sub),
                    _ => return None,
                },
                Cow::Owned(m) => match m.get_field(fd).into_owned() {
                    Value::Message(sub) => Cow::Owned(sub),
                    _ => return None,
                },
            };
        }
        Some(at)
    }

    /// The dotted names of the column's parents: `a` and `a.b` for `a.b.c`.
    fn prefixes(&self) -> impl Iterator<Item = String> + '_ {
        self.name
            .match_indices('.')
            .map(|(i, _)| self.name[..i].to_string())
    }
}

/// What a row needs of a field's PXF annotations.
#[derive(Clone, Copy)]
struct CellOptions {
    has_default: bool,
    required: bool,
}

/// The annotations of `fd`, from its message's post-decode plan, which is
/// cached per type: every row needs every field's, for the required-column
/// check.
fn cell_options(plan: &[PlanField], fd: &FieldDescriptor) -> CellOptions {
    match plan.binary_search_by_key(&fd.number(), |p| p.number) {
        Ok(i) => CellOptions {
            has_default: plan[i].default.is_some(),
            required: plan[i].required,
        },
        Err(_) => CellOptions {
            has_default: false,
            required: false,
        },
    }
}

/// Resolve `columns` against `desc`, refusing what a v1 row cannot carry;
/// see [`append_dataset_header`].
fn dataset_columns<S: AsRef<str>>(
    desc: &MessageDescriptor,
    columns: &[S],
) -> Result<Vec<DatasetColumn>, PxfError> {
    if columns.is_empty() {
        return Err(error("pxf: @dataset needs at least one column".into()));
    }
    // A schema the reader would refuse is refused here instead of written:
    // a field with a reserved name makes the header unreadable, and a oneof
    // with two defaulted members would get two null cells.
    if let Some(msg) = as_validation_error_message(&validate_descriptor(desc)) {
        return Err(error(format!("pxf: @dataset {}: {msg}", desc.full_name())));
    }
    // The reader's rules for column paths (protowire#342), with its
    // messages, so the writer cannot write a header its reader rejects.
    if let Some(msg) = check_column_paths(desc, columns, MAX_NESTING_DEPTH) {
        return Err(error(format!("pxf: {msg}")));
    }
    let mut cols: Vec<DatasetColumn> = columns
        .iter()
        .map(|c| {
            let name = c.as_ref();
            let mut md = desc.clone();
            let mut path = Vec::new();
            let mut segs = name.split('.').peekable();
            while let Some(seg) = segs.next() {
                let fd = md
                    .get_field_by_name(seg)
                    .expect("check_column_paths resolved every segment");
                if segs.peek().is_some() {
                    let Kind::Message(inner) = fd.kind() else {
                        unreachable!("check_column_paths: a parent is a message field")
                    };
                    md = inner;
                }
                path.push(fd);
            }
            DatasetColumn {
                name: name.to_string(),
                path,
                siblings: Vec::new(),
            }
        })
        .collect();
    for i in 0..cols.len() {
        let parent = &cols[i].path[..cols[i].path.len() - 1];
        let siblings = cols
            .iter()
            .filter(|o| o.path.len() == cols[i].path.len() && o.path[..parent.len()] == *parent)
            .map(|o| o.path[o.path.len() - 1].clone())
            .collect();
        cols[i].siblings = siblings;
    }
    let plan = post_decode_plan(desc);
    for fd in declaration_order(desc) {
        if cell_options(&plan, &fd).required && !cols.iter().any(|c| c.path[0] == fd) {
            return Err(error(format!(
                "pxf: @dataset columns omit {:?}, which is (pxf.required): every row would be rejected",
                fd.name()
            )));
        }
    }
    Ok(cols)
}

/// Refuse a row that the reader would reject for a missing `(pxf.required)`
/// field below the root, the one way a dotted column can make a row
/// unreadable. A sub-message is present on reading exactly when one of the
/// cells under it is non-empty (draft §3.4.4, protowire#342 decision 1). So
/// a sub-message that is present in the message but whose cells all come
/// out empty reads back absent: that loss is accepted (the owner's decision
/// on protowire-go#155) unless the sub-message is required, and then the
/// row is refused. And a sub-message that reads back present needs each of
/// its required fields as a column, or under one.
fn check_row_presence(
    desc: &MessageDescriptor,
    cols: &[DatasetColumn],
    filled: &HashSet<String>,
) -> Result<(), String> {
    // Whether a column is exactly `path`, or goes inside it.
    let covered = |path: &str| -> (bool, bool) {
        let mut prefix = false;
        for c in cols {
            if c.name == path {
                return (true, false);
            }
            if c.name.len() > path.len()
                && c.name.starts_with(path)
                && c.name.as_bytes()[path.len()] == b'.'
            {
                prefix = true;
            }
        }
        (false, prefix)
    };
    fn check(
        md: &MessageDescriptor,
        at: &str,
        covered: &dyn Fn(&str) -> (bool, bool),
        filled: &HashSet<String>,
    ) -> Result<(), String> {
        let plan = post_decode_plan(md);
        for fd in declaration_order(md) {
            let path = if at.is_empty() {
                fd.name().to_string()
            } else {
                format!("{at}.{}", fd.name())
            };
            let (exact, prefix) = covered(&path);
            let required = cell_options(&plan, &fd).required;
            if prefix && !filled.contains(&path) && required {
                return Err(format!(
                    "pxf: @dataset columns under {path:?} are all empty in this row, so it reads back unset, and it is (pxf.required)"
                ));
            }
            if !at.is_empty() && required && !exact && !prefix {
                return Err(format!(
                    "pxf: @dataset {at:?} reads back present, and its (pxf.required) field {path:?} is not a column: the row would be rejected"
                ));
            }
            if prefix && filled.contains(&path) {
                if let Kind::Message(sub) = fd.kind() {
                    check(&sub, &path, covered, filled)?;
                }
            }
        }
        Ok(())
    }
    check(desc, "", &covered, filled)
}

impl Encoder<'_> {
    /// Write the cell for `fd` of `m`, in a row whose columns under `m` are
    /// `siblings`; see [`append_dataset_row`]. `path` is the column's name,
    /// which the null set is read at.
    fn write_cell(
        &mut self,
        m: &DynamicMessage,
        fd: &FieldDescriptor,
        path: &str,
        siblings: &[FieldDescriptor],
        o: CellOptions,
    ) -> Result<(), String> {
        let is_msg = matches!(fd.kind(), Kind::Message(_));
        if self.null_set.as_ref().is_some_and(|s| s.contains(path)) && crate::decode::nullable(fd) {
            self.buf.push_str("null");
            return Ok(());
        }
        if m.has_field(fd) || is_negative_zero(m, fd) {
            if fd.is_list() || fd.is_map() {
                return self.write_value_cell(m, fd, None);
            }
            if !is_msg {
                self.write_scalar_or_enum(fd, m);
                return Ok(());
            }
            let value = m.get_field(fd);
            let (Kind::Message(mdesc), Value::Message(sub)) = (fd.kind(), value.as_ref()) else {
                return Ok(());
            };
            return match self.shorthand(&mdesc, sub) {
                Some(text) => {
                    self.buf.push_str(&text);
                    self.take_error()
                }
                None => self.write_value_cell(m, fd, Some(sub)),
            };
        }
        if !o.has_default && !o.required {
            // Empty: absent, and absent is what it reads back as.
            return Ok(());
        }
        if fd.is_list() || fd.is_map() {
            // Only required gets here: a default is refused on these.
            self.buf.push_str(if fd.is_map() { "{}" } else { "[]" });
            return Ok(());
        }
        if !fd.supports_presence() {
            // The zero value, explicitly.
            self.write_scalar_or_enum(fd, m);
            return Ok(());
        }
        if o.required {
            return Err("unset, and the field is (pxf.required)".into());
        }
        if let Some(oneof) = fd.containing_oneof() {
            if !oneof.is_synthetic() {
                // A member written in this row keeps the default off. (At
                // most one member of a oneof may carry a default, which the
                // schema check has seen to, so no second null can arise.)
                if let Some(set) = oneof.fields().find(|f| m.has_field(f)) {
                    if siblings.contains(&set) {
                        return Ok(());
                    }
                }
            }
        }
        if !is_msg {
            // An unset scalar with presence is an empty cell, which reads
            // back as its default (decided on protowire-go#146).
            return Ok(());
        }
        self.buf.push_str("null");
        Ok(())
    }

    /// Write a list, map or block value as one cell (protowire#341): the text
    /// `marshal` writes for the field, on one line, so a table keeps one row
    /// per line. The field is encoded as `marshal` encodes it at no indent,
    /// the field name that leads it is cut, and its newlines become spaces.
    /// That is safe because `marshal` never writes a raw newline inside a
    /// token (strings escape it, bytes are base64), so every newline it
    /// writes separates tokens.
    fn write_value_cell(
        &mut self,
        parent: &DynamicMessage,
        fd: &FieldDescriptor,
        sub: Option<&DynamicMessage>,
    ) -> Result<(), String> {
        let row = std::mem::take(&mut self.buf);
        match sub {
            _ if fd.is_map() => self.encode_map_field(parent, fd, 0),
            _ if fd.is_list() => self.encode_list_field(parent, fd, 0),
            Some(sub) => self.encode_message_field(fd, sub, 0),
            None => {}
        }
        let cell = std::mem::replace(&mut self.buf, row);
        self.take_error()?;
        let mut out = cell.as_str();
        out = out.strip_prefix(fd.name()).unwrap_or(out);
        out = out.strip_prefix(" = ").unwrap_or(out);
        out = out.strip_prefix(' ').unwrap_or(out);
        out = out.strip_suffix('\n').unwrap_or(out);
        self.buf
            .extend(out.chars().map(|c| if c == '\n' { ' ' } else { c }));
        Ok(())
    }

    /// The fault the encoder recorded, as the error of the cell it was in.
    fn take_error(&mut self) -> Result<(), String> {
        match self.error.take() {
            Some(fault) => Err(fault.message()),
            None => Ok(()),
        }
    }
}
