//! Type-safe ID generation and management
//!
//! This module provides a generic, type-safe ID system with consistent prefixes
//! and UUID-based uniqueness guarantees.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::fmt::{self, Display};
use std::str::FromStr;
use surrealdb::RecordId;
use uuid::Uuid;

/// Trait for types that can be used as ID markers
pub trait IdType: Send + Sync + 'static {
    /// The table name for this ID type (e.g., "agent" for agents, "user" for users)
    const PREFIX: &'static str;

    /// Convert to a string key for RecordId
    fn to_key(&self) -> String;

    /// Convert from a string key
    fn from_key(key: &str) -> Result<Self, IdError>
    where
        Self: Sized;
}

/// Errors that can occur when working with IDs
#[derive(Debug, thiserror::Error, miette::Diagnostic)]
pub enum IdError {
    #[error("Invalid ID format: expected prefix '{expected}', got '{actual}'")]
    #[diagnostic(help("Ensure the ID starts with the correct prefix followed by an underscore"))]
    InvalidPrefix { expected: String, actual: String },

    #[error("Invalid UUID: {0}")]
    #[diagnostic(help("The UUID portion of the ID must be a valid UUID v4 format"))]
    InvalidUuid(#[from] uuid::Error),

    #[error("Invalid ID format: {0}")]
    #[diagnostic(help(
        "IDs must be in the format 'prefix_uuid' where prefix matches the expected type"
    ))]
    InvalidFormat(String),
}

/// Macro to define new ID types with minimal boilerplate
#[macro_export]
macro_rules! define_id_type {
    ($type_name:ident, $table:expr) => {
        #[derive(
            Debug,
            PartialEq,
            Eq,
            Hash,
            Clone,
            ::serde::Serialize,
            ::serde::Deserialize,
            ::schemars::JsonSchema,
        )]
        pub struct $type_name(pub String);

        impl $crate::id::IdType for $type_name {
            const PREFIX: &'static str = $table;

            fn to_key(&self) -> String {
                self.0.clone()
            }

            fn from_key(key: &str) -> Result<Self, $crate::id::IdError> {
                Ok($type_name(key.to_string()))
            }
        }

        impl From<$type_name> for ::surrealdb::RecordIdKey {
            fn from(id: $type_name) -> Self {
                id.0.into()
            }
        }

        impl From<$type_name> for ::surrealdb::RecordId {
            fn from(id: $type_name) -> Self {
                ::surrealdb::RecordId::from_table_key(
                    <$type_name as $crate::id::IdType>::PREFIX,
                    id.0,
                )
            }
        }

        impl From<&$type_name> for ::surrealdb::RecordId {
            fn from(id: &$type_name) -> Self {
                ::surrealdb::RecordId::from_table_key(
                    <$type_name as $crate::id::IdType>::PREFIX,
                    &id.0,
                )
            }
        }

        impl ::std::fmt::Display for $type_name {
            fn fmt(&self, f: &mut ::std::fmt::Formatter<'_>) -> ::std::fmt::Result {
                write!(f, "{}", self.0)
            }
        }

        impl $type_name {
            pub fn generate() -> Self {
                $type_name(::uuid::Uuid::new_v4().simple().to_string())
            }

            pub fn nil() -> Self {
                $type_name(::uuid::Uuid::nil().simple().to_string())
            }

            pub fn from_record(record: ::surrealdb::RecordId) -> Self {
                $type_name(record.key().to_string())
            }

            pub fn to_record_id(&self) -> String {
                self.0.clone()
            }

            pub fn from_uuid(uuid: ::uuid::Uuid) -> Self {
                $type_name(uuid.simple().to_string())
            }

            pub fn is_nil(&self) -> bool {
                self.0 == ::uuid::Uuid::nil().simple().to_string()
            }
        }

        impl ::std::str::FromStr for $type_name {
            type Err = $crate::id::IdError;

            fn from_str(s: &str) -> Result<Self, Self::Err> {
                Ok($type_name(s.to_string()))
            }
        }
    };
}

define_id_type!(RelationId, "rel");

// Define common ID types using the macro
define_id_type!(AgentId, "agent");
define_id_type!(UserId, "user");
define_id_type!(ConversationId, "convo");
define_id_type!(TaskId, "task");
define_id_type!(ToolCallId, "toolcall");

impl Default for UserId {
    fn default() -> Self {
        UserId::generate()
    }
}

/// Unlike other IDs in the system, MessageId doesn't follow the `prefix_uuid`
/// format because it needs to be compatible with Anthropic/OpenAI APIs which
/// expect arbitrary string UUIDs.
#[derive(Debug, PartialEq, Eq, Hash, Clone, Serialize, Deserialize, JsonSchema)]
#[repr(transparent)]
pub struct MessageId(pub String);

impl Display for MessageId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

// MessageId cannot implement Copy because String doesn't implement Copy
// This is intentional as MessageId needs to own its string data

impl MessageId {
    pub fn generate() -> Self {
        let uuid = uuid::Uuid::new_v4().simple();
        MessageId(format!("msg_{}", uuid))
    }

    pub fn to_record_id(&self) -> String {
        // Return the full string as the record key
        // MessageId can be arbitrary strings for API compatibility
        self.0.clone()
    }

    pub fn from_uuid(uuid: Uuid) -> Self {
        MessageId(format!("msg_{}", uuid))
    }

    pub fn from_record(record_id: RecordId) -> Self {
        MessageId(record_id.key().to_string())
    }

    pub fn nil() -> Self {
        MessageId("msg_nil".to_string())
    }
}

impl From<MessageId> for RecordId {
    fn from(value: MessageId) -> Self {
        // Use the full string as the key - MessageId can be arbitrary
        RecordId::from_table_key("msg", value.0)
    }
}

impl From<&MessageId> for RecordId {
    fn from(value: &MessageId) -> Self {
        // Use the full string as the key - MessageId can be arbitrary
        RecordId::from_table_key("msg", &value.0)
    }
}

impl IdType for MessageId {
    const PREFIX: &'static str = "msg";

    fn to_key(&self) -> String {
        self.0.clone()
    }

    fn from_key(key: &str) -> Result<Self, IdError> {
        Ok(MessageId(key.to_string()))
    }
}

impl FromStr for MessageId {
    type Err = IdError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Ok(MessageId(s.to_string()))
    }
}

impl JsonSchema for Did {
    fn schema_name() -> std::borrow::Cow<'static, str> {
        "did".into()
    }

    fn json_schema(generator: &mut schemars::SchemaGenerator) -> schemars::Schema {
        generator.root_schema_for::<String>()
    }
}

/// Unlike other IDs in the system, Did doesn't follow the `prefix_uuid`
/// format because it follows the DID standard (did:plc, did:web)
#[derive(Debug, PartialEq, Eq, Hash, Clone, Serialize, Deserialize)]
#[repr(transparent)]
pub struct Did(pub atrium_api::types::string::Did);

// Did cannot implement Copy because String doesn't implement Copy
// This is intentional as Did needs to own its string data

impl Did {
    pub fn to_record_id(&self) -> String {
        // Return the full string as the record key
        self.0.to_string()
    }

    pub fn from_record(record_id: RecordId) -> Self {
        Did(
            atrium_api::types::string::Did::new(record_id.key().to_string())
                .expect("should be valid did"),
        )
    }
}

impl From<Did> for RecordId {
    fn from(value: Did) -> Self {
        RecordId::from_table_key(Did::PREFIX, value.0.to_string())
    }
}

impl From<&Did> for RecordId {
    fn from(value: &Did) -> Self {
        RecordId::from_table_key(Did::PREFIX, &value.0.to_string())
    }
}

impl std::fmt::Display for Did {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0.to_string())
    }
}

impl FromStr for Did {
    type Err = IdError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Ok(Did(atrium_api::types::string::Did::new(s.to_string())
            .map_err(|_| {
                IdError::InvalidFormat(format!("Invalid DID format: {}", s))
            })?))
    }
}

impl IdType for Did {
    const PREFIX: &'static str = "atproto_identity";

    fn to_key(&self) -> String {
        self.0.to_string()
    }

    fn from_key(key: &str) -> Result<Self, IdError> {
        Ok(Did(atrium_api::types::string::Did::new(key.to_string())
            .map_err(|_| {
                IdError::InvalidFormat(format!("Invalid DID format: {}", key))
            })?))
    }
}

// More ID types using the macro
define_id_type!(MemoryId, "mem");
define_id_type!(EventId, "event");
define_id_type!(SessionId, "session");

// Define new ID types using the macro
define_id_type!(ModelId, "model");
define_id_type!(RequestId, "request");
define_id_type!(GroupId, "group");
define_id_type!(ConstellationId, "const");
define_id_type!(OAuthTokenId, "oauth");
define_id_type!(AtprotoIdentityId, "atproto_identity");

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_id_generation() {
        let id1 = AgentId::generate();
        let id2 = AgentId::generate();

        // IDs should be unique
        assert_ne!(id1, id2);

        // IDs should have correct table name
        assert_eq!(AgentId::PREFIX, "agent");
    }

    #[test]
    fn test_id_serialization() {
        let id = AgentId::generate();

        // JSON serialization
        let json = serde_json::to_string(&id).unwrap();
        let deserialized: AgentId = serde_json::from_str(&json).unwrap();
        assert_eq!(id, deserialized);
    }

    #[test]
    fn test_different_id_types() {
        let agent_id = AgentId::generate();
        let user_id = UserId::generate();
        let task_id = TaskId::generate();

        // All should be different UUIDs
        assert_ne!(agent_id.0, user_id.0);
        assert_ne!(user_id.0, task_id.0);
    }

    #[test]
    fn test_record_id_conversion() {
        let agent_id = AgentId::generate();
        let record_id: RecordId = agent_id.clone().into();

        assert_eq!(record_id.table(), "agent");
        assert_eq!(record_id.key().to_string(), agent_id.0);
    }
}
