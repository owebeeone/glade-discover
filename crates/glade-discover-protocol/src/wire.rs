use crate::{Principal, SignedOp, StreamId, SyncId};

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub struct Head {
    pub origin: Principal,
    pub seq: u64,
    pub hash: Option<[u8; 32]>,
}

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub struct StreamHead {
    pub stream: StreamId,
    pub origin: Principal,
    pub seq: u64,
    pub hash: [u8; 32],
}

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub enum WireMsg {
    DirOp {
        op: Box<SignedOp>,
    },
    SyncStart {
        sync_id: SyncId,
        heads: Vec<StreamHead>,
    },
    SyncOps {
        sync_id: SyncId,
        ops: Vec<SignedOp>,
    },
    SyncEnd {
        sync_id: SyncId,
    },
}
