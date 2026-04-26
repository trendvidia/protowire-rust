//! SBE codec: registers proto messages by template ID and dispatches to the
//! marshal / unmarshal / view paths. Mirrors `protowire/encoding/sbe/sbe.go`.
//!
//! The header sizes are part of the SBE wire contract — see SBE 1.0 spec
//! sections 2.1 (message header) and 2.4 (group header).

use std::collections::HashMap;

use prost_reflect::{FileDescriptor, MessageDescriptor};

use crate::annotations::{
    has_template_id, file_uint32, EXT_SCHEMA_ID, EXT_VERSION,
};
use crate::errors::SbeError;
use crate::template::{build_template, MessageTemplate};

/// Message header size: `block_length(2) + template_id(2) + schema_id(2) + version(2)`.
pub const HEADER_SIZE: usize = 8;

/// Repeating-group header size: `block_length(2) + num_in_group(2)`.
pub const GROUP_HEADER_SIZE: usize = 4;

#[derive(Debug, Clone, Default)]
pub struct Codec {
    by_name: HashMap<String, MessageTemplate>,
    by_id: HashMap<u32, MessageTemplate>,
}

impl Codec {
    /// Build a Codec from one or more file descriptors. Each file must
    /// declare `(sbe.schema_id)`. `(sbe.version)` defaults to 0 when absent.
    pub fn from_files(files: &[FileDescriptor]) -> Result<Self, SbeError> {
        let mut codec = Codec::default();
        for file in files {
            let schema_id = file_uint32(file, EXT_SCHEMA_ID).ok_or_else(|| {
                SbeError::new(format!(
                    "sbe: file {} missing (sbe.schema_id) option",
                    file.name()
                ))
            })?;
            let version = file_uint32(file, EXT_VERSION).unwrap_or(0);
            for desc in file.messages() {
                codec.register_message(&desc, schema_id, version)?;
            }
        }
        Ok(codec)
    }

    fn register_message(
        &mut self,
        desc: &MessageDescriptor,
        schema_id: u32,
        version: u32,
    ) -> Result<(), SbeError> {
        if has_template_id(desc) {
            let tmpl = build_template(desc, schema_id, version)?;
            let id = tmpl.template_id;
            self.by_name.insert(desc.full_name().to_string(), tmpl.clone());
            self.by_id.insert(id, tmpl);
        }
        for nested in desc.child_messages() {
            self.register_message(&nested, schema_id, version)?;
        }
        Ok(())
    }

    pub fn template(&self, type_name: &str) -> Result<&MessageTemplate, SbeError> {
        self.by_name
            .get(type_name)
            .ok_or_else(|| SbeError::new(format!("sbe: no template registered for {}", type_name)))
    }

    pub fn template_by_id(&self, id: u32) -> Result<&MessageTemplate, SbeError> {
        self.by_id
            .get(&id)
            .ok_or_else(|| SbeError::new(format!("sbe: unknown template ID {}", id)))
    }

    pub fn by_name(&self) -> &HashMap<String, MessageTemplate> {
        &self.by_name
    }

    pub fn by_id(&self) -> &HashMap<u32, MessageTemplate> {
        &self.by_id
    }
}
