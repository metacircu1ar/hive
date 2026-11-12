//! Shared literal domain and open wire enum definitions.

macro_rules! domain_enum {
    (
        $(#[$enum_attribute:meta])*
        $name:ident {
            $($(#[$attribute:meta])* $variant:ident => $value:literal),+ $(,)?
        }
    ) => {
        $(#[$enum_attribute])*
        #[derive(Clone, Copy, Debug, Eq, PartialEq, serde::Serialize, schemars::JsonSchema)]
        #[serde(rename_all = "snake_case")]
        pub enum $name {
            $($(#[$attribute])* #[serde(rename = $value)] $variant),+
        }

        impl $name {
            pub const VALUES: &'static [&'static str] = &[$($(#[$attribute])* $value),+];

            pub const fn as_str(self) -> &'static str {
                match self {
                    $($(#[$attribute])* Self::$variant => $value),+
                }
            }

            pub fn parse(value: &str) -> Option<Self> {
                match value {
                    $($(#[$attribute])* $value => Some(Self::$variant)),+,
                    _ => None,
                }
            }
        }

        impl<'de> serde::Deserialize<'de> for $name {
            fn deserialize<D: serde::Deserializer<'de>>(
                deserializer: D,
            ) -> std::result::Result<Self, D::Error> {
                struct Literal;
                impl serde::de::Visitor<'_> for Literal {
                    type Value = $name;
                    fn expecting(
                        &self,
                        formatter: &mut std::fmt::Formatter<'_>,
                    ) -> std::fmt::Result {
                        formatter.write_str(concat!("a literal ", stringify!($name), " string"))
                    }
                    fn visit_str<E: serde::de::Error>(
                        self,
                        value: &str,
                    ) -> std::result::Result<$name, E> {
                        $name::parse(value).ok_or_else(|| E::unknown_variant(value, $name::VALUES))
                    }
                }
                deserializer.deserialize_str(Literal)
            }
        }

        impl rusqlite::ToSql for $name {
            fn to_sql(&self) -> rusqlite::Result<rusqlite::types::ToSqlOutput<'_>> {
                Ok(rusqlite::types::ToSqlOutput::Borrowed(
                    rusqlite::types::ValueRef::Text(self.as_str().as_bytes()),
                ))
            }
        }

        impl rusqlite::types::FromSql for $name {
            fn column_result(
                value: rusqlite::types::ValueRef<'_>,
            ) -> rusqlite::types::FromSqlResult<Self> {
                Self::parse(value.as_str()?).ok_or_else(|| {
                    rusqlite::types::FromSqlError::Other(
                        concat!("Invalid ", stringify!($name), ".").into(),
                    )
                })
            }
        }
    };
}

pub(crate) use domain_enum;

// Open provider vocabularies classify only known strings. The original frame remains
// available for evidence and forwarding; unknown strings are never reconstructed.
macro_rules! wire_enum {
    ($name:ident { $($variant:ident => $wire:literal),+ $(,)? }) => {
        #[derive(Clone, Copy, Debug, Eq, PartialEq, serde::Deserialize)]
        #[serde(from = "String")]
        pub(crate) enum $name {
            $(#[serde(rename = $wire)] $variant,)+
            #[serde(other)]
            Other,
        }
        impl $name {
            pub fn parse(value: &str) -> Self {
                match value {
                    $($wire => Self::$variant,)+
                    _ => Self::Other
                }
            }
        }
        impl From<String> for $name {
            fn from(value: String) -> Self {
                Self::parse(&value)
            }
        }
    };
}
pub(crate) use wire_enum;
