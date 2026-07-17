use std::collections::BTreeMap;

use glade_discover_protocol::{
    MAX_MESSAGE_BYTES, MAX_SYNC_ID_BYTES, NodeId, SignedOp, StreamHead, SyncId, WireMsg,
    encode_stream_head, encode_wire_msg, op_hash, sync_list_body_len,
};

use crate::{
    Effect, IngestDisposition, MonoInstant, PersistedState, RoundProgress, State, StepCtx,
    VerificationBatch, WakeToken, ingest,
};

const OPS_PER_CHUNK: usize = 256;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SyncTransition {
    pub persisted: PersistedState,
    pub rounds: BTreeMap<(NodeId, SyncId), RoundProgress>,
    pub effects: Vec<Effect>,
}

#[must_use]
pub fn on_message(
    state: &State,
    _ctx: StepCtx,
    from: &NodeId,
    message: &WireMsg,
    verification: &VerificationBatch,
) -> SyncTransition {
    match message {
        WireMsg::SyncStart { sync_id, heads }
            if state.config().peers.contains(from)
                && sync_id.as_str().len() <= MAX_SYNC_ID_BYTES =>
        {
            let end = WireMsg::SyncEnd {
                sync_id: sync_id.clone(),
            };
            if sync_list_body_len(sync_id, 0, 0).is_none_or(|bytes| bytes > MAX_MESSAGE_BYTES)
                || !wire_message_fits(&end)
            {
                return unchanged(state);
            }
            let ops = missing_ops(state, heads);
            let Some(effects) = bounded_sync_ops(from, sync_id, ops) else {
                return unchanged(state);
            };
            SyncTransition {
                persisted: state.persisted().clone(),
                rounds: state.sync().clone(),
                effects,
            }
        }
        WireMsg::SyncStart { .. } => unchanged(state),
        WireMsg::SyncOps { sync_id, ops }
            if sync_id.as_str().len() <= MAX_SYNC_ID_BYTES
                && matches!(
                    state.sync().get(&(from.clone(), sync_id.clone())),
                    Some(RoundProgress::Active { .. })
                ) =>
        {
            ingest_ops(state, ops, verification)
        }
        WireMsg::SyncOps { .. } => unchanged(state),
        WireMsg::SyncEnd { sync_id } if sync_id.as_str().len() <= MAX_SYNC_ID_BYTES => {
            finish_round(state, from, sync_id)
        }
        WireMsg::SyncEnd { .. } => unchanged(state),
        WireMsg::DirOp { .. } => unchanged(state),
    }
}

#[must_use]
pub fn on_wakeup(state: &State, ctx: StepCtx, token: &WakeToken) -> SyncTransition {
    match token {
        WakeToken::GossipTick { peer } if state.config().peers.contains(peer) => {
            start_round(state, ctx, peer)
        }
        WakeToken::SyncTimeout {
            peer,
            sync_id,
            attempt,
        } => retry_round(state, ctx, peer, sync_id, *attempt),
        WakeToken::ClaimRenew { .. }
        | WakeToken::AppendRetry { .. }
        | WakeToken::GossipTick { .. } => unchanged(state),
    }
}

fn missing_ops(state: &State, peer_heads: &[StreamHead]) -> Vec<SignedOp> {
    let heads = peer_heads
        .iter()
        .map(|head| ((head.stream.clone(), head.origin.clone()), head))
        .collect::<BTreeMap<_, _>>();
    state
        .persisted()
        .retained
        .values()
        .flat_map(|records| records.values())
        .filter(|op| {
            heads
                .get(&(op.envelope().stream.clone(), op.envelope().origin.clone()))
                .is_none_or(|head| {
                    op.envelope().seq > head.seq
                        || (op.envelope().seq == head.seq && op_hash(op) != head.hash)
                })
        })
        .cloned()
        .collect()
}

fn bounded_sync_ops(peer: &NodeId, sync_id: &SyncId, ops: Vec<SignedOp>) -> Option<Vec<Effect>> {
    if sync_id.as_str().len() > MAX_SYNC_ID_BYTES {
        return None;
    }
    if sync_list_body_len(sync_id, 0, 0)? > MAX_MESSAGE_BYTES {
        return None;
    }
    let mut effects = Vec::new();
    let mut chunk = Vec::new();
    let mut item_bytes = 0_usize;
    for op in ops {
        let op_bytes = op.canonical_bytes().len();
        let candidate_items = item_bytes.checked_add(op_bytes)?;
        let candidate_len =
            sync_list_body_len(sync_id, chunk.len().checked_add(1)?, candidate_items)?;
        if chunk.len() == OPS_PER_CHUNK || candidate_len > MAX_MESSAGE_BYTES {
            if chunk.is_empty() {
                return None;
            }
            let message = WireMsg::SyncOps {
                sync_id: sync_id.clone(),
                ops: std::mem::take(&mut chunk),
            };
            if !wire_message_fits(&message) {
                return None;
            }
            effects.push(Effect::Gossip {
                to: peer.clone(),
                msg: Box::new(message),
            });
            item_bytes = 0;
        }
        let single_or_next_len = sync_list_body_len(
            sync_id,
            chunk.len().checked_add(1)?,
            item_bytes.checked_add(op_bytes)?,
        )?;
        if single_or_next_len > MAX_MESSAGE_BYTES {
            return None;
        }
        item_bytes = item_bytes.checked_add(op_bytes)?;
        chunk.push(op);
    }
    if !chunk.is_empty() {
        let message = WireMsg::SyncOps {
            sync_id: sync_id.clone(),
            ops: chunk,
        };
        if !wire_message_fits(&message) {
            return None;
        }
        effects.push(Effect::Gossip {
            to: peer.clone(),
            msg: Box::new(message),
        });
    }
    let end = WireMsg::SyncEnd {
        sync_id: sync_id.clone(),
    };
    effects.push(Effect::Gossip {
        to: peer.clone(),
        msg: Box::new(end),
    });
    Some(effects)
}

fn ingest_ops(state: &State, ops: &[SignedOp], verification: &VerificationBatch) -> SyncTransition {
    if ops.len() != verification.0.len() {
        return unchanged(state);
    }
    let mut persisted = state.persisted().clone();
    for (op, verification) in ops.iter().zip(&verification.0) {
        let outcome = ingest::ingest(op, verification, &persisted, state.config(), state.clock());
        if !matches!(
            outcome.disposition,
            IngestDisposition::Folded
                | IngestDisposition::TimeDeferred
                | IngestDisposition::Duplicate
        ) {
            return SyncTransition {
                persisted,
                rounds: state.sync().clone(),
                effects: Vec::new(),
            };
        }
        persisted = outcome.persisted;
    }
    SyncTransition {
        persisted,
        rounds: state.sync().clone(),
        effects: Vec::new(),
    }
}

fn start_round(state: &State, ctx: StepCtx, peer: &NodeId) -> SyncTransition {
    let mut persisted = state.persisted().clone();
    let ordinal = persisted.next_sync;
    let Some(next) = ordinal.checked_add(1) else {
        return unchanged(state);
    };
    persisted.next_sync = next;
    let sync_id = SyncId::from(format!("{}:{ordinal}", state.config().local_node));
    let mut rounds = state.sync().clone();
    rounds.insert(
        (peer.clone(), sync_id.clone()),
        RoundProgress::Active { attempt: 0 },
    );
    scheduled_round(state, ctx, persisted, rounds, peer, sync_id, 0)
}

fn retry_round(
    state: &State,
    ctx: StepCtx,
    peer: &NodeId,
    sync_id: &SyncId,
    attempt: u8,
) -> SyncTransition {
    let key = (peer.clone(), sync_id.clone());
    if state.sync().get(&key) != Some(&RoundProgress::Active { attempt }) {
        return unchanged(state);
    }
    let mut rounds = state.sync().clone();
    if attempt >= state.config().sync_retries {
        rounds.insert(key, RoundProgress::Stale);
        return SyncTransition {
            persisted: state.persisted().clone(),
            rounds,
            effects: Vec::new(),
        };
    }
    let Some(next_attempt) = attempt.checked_add(1) else {
        rounds.insert(key, RoundProgress::Stale);
        return SyncTransition {
            persisted: state.persisted().clone(),
            rounds,
            effects: Vec::new(),
        };
    };
    rounds.insert(
        key,
        RoundProgress::Active {
            attempt: next_attempt,
        },
    );
    scheduled_round(
        state,
        ctx,
        state.persisted().clone(),
        rounds,
        peer,
        sync_id.clone(),
        next_attempt,
    )
}

fn scheduled_round(
    state: &State,
    ctx: StepCtx,
    persisted: PersistedState,
    mut rounds: BTreeMap<(NodeId, SyncId), RoundProgress>,
    peer: &NodeId,
    sync_id: SyncId,
    attempt: u8,
) -> SyncTransition {
    let Some(at_mono) = ctx
        .mono
        .0
        .checked_add(state.config().sync_timeout_ms.get())
        .map(MonoInstant)
    else {
        rounds.insert((peer.clone(), sync_id), RoundProgress::Stale);
        return SyncTransition {
            persisted,
            rounds,
            effects: Vec::new(),
        };
    };
    let Some(start) = bounded_sync_start(state, &sync_id) else {
        rounds.insert((peer.clone(), sync_id), RoundProgress::Stale);
        return SyncTransition {
            persisted,
            rounds,
            effects: Vec::new(),
        };
    };
    SyncTransition {
        persisted,
        rounds,
        effects: vec![
            Effect::Gossip {
                to: peer.clone(),
                msg: Box::new(start),
            },
            Effect::Schedule {
                token: WakeToken::SyncTimeout {
                    peer: peer.clone(),
                    sync_id,
                    attempt,
                },
                at_mono,
            },
        ],
    }
}

fn finish_round(state: &State, peer: &NodeId, sync_id: &SyncId) -> SyncTransition {
    let key = (peer.clone(), sync_id.clone());
    let mut rounds = state.sync().clone();
    if matches!(rounds.get(&key), Some(RoundProgress::Active { .. })) {
        rounds.insert(key, RoundProgress::Complete);
    }
    SyncTransition {
        persisted: state.persisted().clone(),
        rounds,
        effects: Vec::new(),
    }
}

fn local_heads(state: &State) -> Vec<StreamHead> {
    let mut heads = BTreeMap::<
        (
            glade_discover_protocol::StreamId,
            glade_discover_protocol::Principal,
        ),
        SignedOp,
    >::new();
    for op in state
        .persisted()
        .retained
        .values()
        .flat_map(|records| records.values())
    {
        let key = (op.envelope().stream.clone(), op.envelope().origin.clone());
        if heads
            .get(&key)
            .is_none_or(|current| current.envelope().seq < op.envelope().seq)
        {
            heads.insert(key, op.clone());
        }
    }
    heads
        .into_values()
        .map(|op| StreamHead {
            stream: op.envelope().stream.clone(),
            origin: op.envelope().origin.clone(),
            seq: op.envelope().seq,
            hash: op_hash(&op),
        })
        .collect()
}

fn bounded_sync_start(state: &State, sync_id: &SyncId) -> Option<WireMsg> {
    if sync_id.as_str().len() > MAX_SYNC_ID_BYTES {
        return None;
    }
    if sync_list_body_len(sync_id, 0, 0)? > MAX_MESSAGE_BYTES {
        return None;
    }
    let mut heads = Vec::new();
    let mut item_bytes = 0_usize;
    for head in local_heads(state) {
        let candidate_items = item_bytes.checked_add(encode_stream_head(&head).len())?;
        let candidate_len =
            sync_list_body_len(sync_id, heads.len().checked_add(1)?, candidate_items)?;
        if candidate_len > MAX_MESSAGE_BYTES {
            break;
        }
        item_bytes = candidate_items;
        heads.push(head);
    }
    let message = WireMsg::SyncStart {
        sync_id: sync_id.clone(),
        heads,
    };
    if wire_message_fits(&message) {
        Some(message)
    } else {
        None
    }
}

fn wire_message_fits(message: &WireMsg) -> bool {
    encode_wire_msg(message).1.len() <= MAX_MESSAGE_BYTES
}

fn unchanged(state: &State) -> SyncTransition {
    SyncTransition {
        persisted: state.persisted().clone(),
        rounds: state.sync().clone(),
        effects: Vec::new(),
    }
}
