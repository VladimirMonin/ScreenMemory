use schemars::{json_schema, JsonSchema, Schema, SchemaGenerator};
use serde::{de::Error as _, Deserialize, Deserializer, Serialize, Serializer};
use time::{format_description::well_known::Rfc3339, OffsetDateTime};

macro_rules! bounded {
    ($name:ident, $max:literal, $pattern:literal) => {
        #[derive(Clone, Debug, PartialEq, Eq)]
        pub struct $name(String);
        impl $name {
            pub fn parse(raw: impl Into<String>) -> Result<Self, &'static str> {
                let raw = raw.into();
                if raw.is_empty() || raw.chars().count() > $max || !raw.is_ascii() ||
                    !raw.bytes().all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'-')) ||
                    !raw.as_bytes()[0].is_ascii_alphanumeric() {
                    return Err("invalid bounded identifier");
                }
                Ok(Self(raw))
            }
            pub fn as_str(&self) -> &str { &self.0 }
        }
        impl Serialize for $name {
            fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> { s.serialize_str(&self.0) }
        }
        impl<'de> Deserialize<'de> for $name {
            fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
                Self::parse(String::deserialize(d)?).map_err(D::Error::custom)
            }
        }
        impl JsonSchema for $name {
            fn schema_name() -> std::borrow::Cow<'static, str> { stringify!($name).into() }
            fn json_schema(_: &mut SchemaGenerator) -> Schema {
                json_schema!({"type": "string", "minLength": 1, "maxLength": $max, "pattern": $pattern})
            }
        }
    };
}
bounded!(ProfileId, 128, "^[A-Za-z0-9][A-Za-z0-9._-]*$");
bounded!(BuildId, 128, "^[A-Za-z0-9][A-Za-z0-9._-]*$");
bounded!(MethodName, 96, "^[A-Za-z0-9][A-Za-z0-9._-]*$");
bounded!(CheckId, 96, "^[A-Za-z0-9][A-Za-z0-9._-]*$");

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SafeMessage(String);
impl SafeMessage {
    pub fn parse(raw: impl Into<String>) -> Result<Self, &'static str> {
        let raw = raw.into();
        if raw.is_empty() || raw.chars().count() > 512 || raw.chars().any(char::is_control) {
            return Err("invalid safe message");
        }
        Ok(Self(raw))
    }
}
impl Serialize for SafeMessage {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(&self.0)
    }
}
impl<'de> Deserialize<'de> for SafeMessage {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        Self::parse(String::deserialize(d)?).map_err(D::Error::custom)
    }
}
impl JsonSchema for SafeMessage {
    fn schema_name() -> std::borrow::Cow<'static, str> {
        "SafeMessage".into()
    }
    fn json_schema(_: &mut SchemaGenerator) -> Schema {
        // Rust char::is_control covers C0, DEL and C1. An unanchored negative
        // match also rejects a trailing LF (unlike $ in Python/ECMAScript).
        json_schema!({"type": "string", "minLength": 1, "maxLength": 512,
            "not": {"pattern": r"[\u0000-\u001F\u007F-\u009F]"}})
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ConsentLifetime(u16);
impl ConsentLifetime {
    pub fn new(value: u16) -> Result<Self, &'static str> {
        if !(1..=300).contains(&value) {
            return Err("consent lifetime outside 1..300");
        }
        Ok(Self(value))
    }
    pub fn seconds(self) -> u16 {
        self.0
    }
}
impl Serialize for ConsentLifetime {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_u16(self.0)
    }
}
impl<'de> Deserialize<'de> for ConsentLifetime {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        Self::new(u16::deserialize(d)?).map_err(D::Error::custom)
    }
}
impl JsonSchema for ConsentLifetime {
    fn schema_name() -> std::borrow::Cow<'static, str> {
        "ConsentLifetime".into()
    }
    fn json_schema(_: &mut SchemaGenerator) -> Schema {
        json_schema!({"type":"integer", "minimum":1, "maximum":300})
    }
}

/// UTC RFC 3339 timestamp with literal Z; offsets and noncanonical forms are rejected.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct UtcTime(String);
impl UtcTime {
    pub fn parse(raw: impl Into<String>) -> Result<Self, &'static str> {
        let raw = raw.into();
        let parsed =
            OffsetDateTime::parse(&raw, &Rfc3339).map_err(|_| "invalid RFC3339 timestamp")?;
        if !raw.ends_with('Z') || parsed.offset().whole_seconds() != 0 {
            return Err("timestamp must be UTC Z");
        }
        Ok(Self(raw))
    }
}
impl Serialize for UtcTime {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(&self.0)
    }
}
impl<'de> Deserialize<'de> for UtcTime {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        Self::parse(String::deserialize(d)?).map_err(D::Error::custom)
    }
}
impl JsonSchema for UtcTime {
    fn schema_name() -> std::borrow::Cow<'static, str> {
        "UtcTime".into()
    }
    fn json_schema(_: &mut SchemaGenerator) -> Schema {
        json_schema!({"type":"string", "format":"date-time", "pattern":"Z$"})
    }
}
