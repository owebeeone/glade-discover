use core::fmt;

macro_rules! text_id {
    ($name:ident) => {
        #[derive(Clone, Debug, Default, Eq, Hash, Ord, PartialEq, PartialOrd)]
        pub struct $name(String);

        impl $name {
            #[must_use]
            pub fn as_str(&self) -> &str {
                &self.0
            }
        }

        impl From<&str> for $name {
            fn from(value: &str) -> Self {
                Self(value.to_owned())
            }
        }

        impl From<String> for $name {
            fn from(value: String) -> Self {
                Self(value)
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                self.0.fmt(formatter)
            }
        }
    };
}

text_id!(NodeId);
text_id!(Principal);
text_id!(IngressId);
text_id!(Corr);
text_id!(IntentId);
text_id!(SyncId);

#[derive(Clone, Debug, Default, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct ComputeKey(Vec<u8>);

impl ComputeKey {
    #[must_use]
    pub fn as_bytes(&self) -> &[u8] {
        &self.0
    }
}

impl From<Vec<u8>> for ComputeKey {
    fn from(value: Vec<u8>) -> Self {
        Self(value)
    }
}

#[derive(Clone, Debug, Default, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct StreamId {
    pub share: String,
    pub glade_id: String,
    pub key: Vec<u8>,
}

#[derive(Clone, Debug, Default, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct RecordId {
    pub stream: StreamId,
    pub origin: Principal,
    pub seq: u64,
}

macro_rules! record_id {
    ($name:ident) => {
        #[derive(Clone, Debug, Default, Eq, Hash, Ord, PartialEq, PartialOrd)]
        pub struct $name(RecordId);

        impl $name {
            #[must_use]
            pub fn record(&self) -> &RecordId {
                &self.0
            }
        }

        impl From<RecordId> for $name {
            fn from(value: RecordId) -> Self {
                Self(value)
            }
        }
    };
}

record_id!(GrantId);
record_id!(ClaimId);
record_id!(DefRevId);

#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum Slot {
    Workspace {
        share: String,
    },
    Binding {
        share: String,
        glade_id: String,
        key: Vec<u8>,
    },
}

#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct RouteQuery {
    pub slot: Slot,
}

#[derive(Clone, Copy, Debug, Default, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct Generation(pub u64);
