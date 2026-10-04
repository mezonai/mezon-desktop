use std::fmt;
use std::str::FromStr;

use serde::de::{self, Visitor};
use serde::{Deserialize, Deserializer, Serialize, Serializer};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParseIdError(String);

impl fmt::Display for ParseIdError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "invalid id: {:?}", self.0)
    }
}

impl std::error::Error for ParseIdError {}

/// Deserialize an `i64` id from either a JSON number or a JSON string. The string form keeps
/// existing `settings.json` files (which persisted ids as strings) loading after the migration.
fn deserialize_i64<'de, D>(deserializer: D) -> Result<i64, D::Error>
where
    D: Deserializer<'de>,
{
    struct IdVisitor;

    impl Visitor<'_> for IdVisitor {
        type Value = i64;

        fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
            f.write_str("an i64 id as a number or a numeric string")
        }

        fn visit_i64<E: de::Error>(self, value: i64) -> Result<i64, E> {
            Ok(value)
        }

        fn visit_u64<E: de::Error>(self, value: u64) -> Result<i64, E> {
            i64::try_from(value).map_err(|_| E::custom("id out of range for i64"))
        }

        fn visit_str<E: de::Error>(self, value: &str) -> Result<i64, E> {
            value
                .parse::<i64>()
                .map_err(|_| E::custom(format!("invalid numeric id string: {value:?}")))
        }
    }

    deserializer.deserialize_any(IdVisitor)
}

macro_rules! define_id {
    ($name:ident) => {
        #[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, Default, PartialOrd, Ord)]
        pub struct $name(pub i64);

        impl $name {
            pub const fn new(value: i64) -> Self {
                Self(value)
            }

            pub const fn get(self) -> i64 {
                self.0
            }

            pub const fn is_zero(self) -> bool {
                self.0 == 0
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                write!(f, "{}", self.0)
            }
        }

        impl From<i64> for $name {
            fn from(value: i64) -> Self {
                Self(value)
            }
        }

        impl FromStr for $name {
            type Err = ParseIdError;

            fn from_str(s: &str) -> Result<Self, Self::Err> {
                s.parse::<i64>()
                    .map(Self)
                    .map_err(|_| ParseIdError(s.to_string()))
            }
        }

        impl Serialize for $name {
            fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
                serializer.serialize_i64(self.0)
            }
        }

        impl<'de> Deserialize<'de> for $name {
            fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
                deserialize_i64(deserializer).map(Self)
            }
        }
    };
}

define_id!(ClanId);
define_id!(ChannelId);
define_id!(UserId);
define_id!(RoleId);
define_id!(MessageId);

impl MessageId {
    const OPTIMISTIC_BASE: i64 = i64::MAX - (1_i64 << 40);

    pub fn is_optimistic(self) -> bool {
        self.0 >= Self::OPTIMISTIC_BASE
    }

    pub fn next_optimistic() -> Self {
        use std::sync::atomic::{AtomicI64, Ordering};
        static COUNTER: AtomicI64 = AtomicI64::new(MessageId::OPTIMISTIC_BASE);
        MessageId(COUNTER.fetch_add(1, Ordering::Relaxed))
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct MessageRef {
    pub bucket: ChannelId,
    pub id: MessageId,
}

impl MessageRef {
    pub fn new(bucket: ChannelId, id: MessageId) -> Self {
        Self { bucket, id }
    }

    pub fn unbucketed(id: MessageId) -> Self {
        Self::new(ChannelId(0), id)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn display_writes_raw_i64() {
        assert_eq!(ClanId(1840).to_string(), "1840");
        assert_eq!(UserId(0).to_string(), "0");
    }

    #[test]
    fn from_str_parses_numeric() {
        assert_eq!("42".parse::<ChannelId>().unwrap(), ChannelId(42));
    }

    #[test]
    fn from_str_rejects_non_numeric() {
        assert!("abc".parse::<ChannelId>().is_err());
        assert!("".parse::<UserId>().is_err());
    }

    #[test]
    fn from_str_display_round_trip() {
        let id = ClanId(9007199254740993);
        assert_eq!(id.to_string().parse::<ClanId>().unwrap(), id);
    }

    #[test]
    fn serializes_as_number() {
        assert_eq!(serde_json::to_string(&ClanId(7)).unwrap(), "7");
    }

    #[test]
    fn deserializes_from_number_and_string() {
        assert_eq!(serde_json::from_str::<ClanId>("7").unwrap(), ClanId(7));
        assert_eq!(serde_json::from_str::<ClanId>("\"7\"").unwrap(), ClanId(7));
    }

    #[test]
    fn deserializes_optional_and_vec_back_compat() {
        let from_string: Option<ClanId> = serde_json::from_str("\"123\"").unwrap();
        assert_eq!(from_string, Some(ClanId(123)));
        let order: Vec<ClanId> = serde_json::from_str("[\"1\",\"2\",3]").unwrap();
        assert_eq!(order, vec![ClanId(1), ClanId(2), ClanId(3)]);
    }

    #[test]
    fn deserialize_rejects_garbage_string() {
        assert!(serde_json::from_str::<ClanId>("\"nope\"").is_err());
    }

    #[test]
    fn helpers_get_and_is_zero() {
        assert_eq!(ChannelId(5).get(), 5);
        assert!(UserId(0).is_zero());
        assert!(!UserId(1).is_zero());
    }
}
