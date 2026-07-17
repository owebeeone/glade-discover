use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Write as _;
use std::fs;
use std::path::{Path, PathBuf};

use glade_discover_protocol::{
    CapabilityGrant, CapabilityRevocation, CapabilityVerb, ClaimId, ComputeKey, DefRevId,
    DirectoryRecord, ExecutionScope, GrantId, GrantScope, NodeId, OpEnvelope, Principal, RecordId,
    ServeClaim, ServiceInstanceClaim, Shape, SignedOp, StreamId, decode_directory_record,
    decode_signed_op, encode_directory_record, encode_signed_op, op_hash, record_id,
};
use serde_json::Value;

#[derive(Clone)]
struct Fixture {
    file: &'static str,
    input: &'static str,
    op: SignedOp,
}

fn stream(share: &str, glade_id: &str, key: u8) -> StreamId {
    StreamId {
        share: share.into(),
        glade_id: glade_id.into(),
        key: vec![key],
    }
}

fn record_id_for(stream: &StreamId, origin: &str) -> RecordId {
    RecordId {
        stream: stream.clone(),
        origin: Principal::from(origin),
        seq: 0,
    }
}

fn signed_record(stream: StreamId, origin: &str, record: DirectoryRecord) -> SignedOp {
    let payload = encode_directory_record(&record).expect("encode directory record");
    let envelope = OpEnvelope {
        stream,
        origin: Principal::from(origin),
        seq: 0,
        prev: None,
        lamport: 1,
        refs: Vec::new(),
        shape: Shape::Log,
        payload,
    };
    encode_signed_op(&envelope, &[0xa5, 0x5a]).expect("encode signed op")
}

fn signed_renewal(initial: &SignedOp, lease_expiry_ms: i64) -> SignedOp {
    let DirectoryRecord::ServeClaim(mut claim) =
        decode_directory_record(&initial.envelope().payload).expect("decode initial claim")
    else {
        panic!("renewal anchor must be a workspace claim");
    };
    claim.lease_expiry_ms = lease_expiry_ms;
    let envelope = OpEnvelope {
        stream: initial.envelope().stream.clone(),
        origin: initial.envelope().origin.clone(),
        seq: initial.envelope().seq + 1,
        prev: Some(op_hash(initial)),
        lamport: initial.envelope().lamport + 1,
        refs: initial.envelope().refs.clone(),
        shape: initial.envelope().shape,
        payload: encode_directory_record(&DirectoryRecord::ServeClaim(claim))
            .expect("encode renewal claim"),
    };
    encode_signed_op(&envelope, &[0xa5, 0x5a]).expect("encode renewal op")
}

fn workspace_pair(
    file: &'static str,
    inputs: (&'static str, &'static str),
    claim_spec: (&str, &str, i64, u64),
    grant_key: u8,
) -> Vec<Fixture> {
    let (grant_input, claim_input) = inputs;
    let (node, principal, expiry, epoch) = claim_spec;
    let grant_stream = stream("workspace-a", "directory", grant_key);
    let claim_stream = stream("workspace-a", "directory", 0x01);
    let grant_record_id = record_id_for(&grant_stream, "owner-a");
    let claim_record_id = record_id_for(&claim_stream, principal);
    let grant = DirectoryRecord::CapabilityGrant(CapabilityGrant {
        grant_id: GrantId::from(grant_record_id.clone()),
        issuer: Principal::from("owner-a"),
        principal: Principal::from(principal),
        share: "workspace-a".into(),
        verbs: vec![CapabilityVerb::Serve],
        scope: None,
    });
    let claim = DirectoryRecord::ServeClaim(ServeClaim {
        node: NodeId::from(node),
        share: "workspace-a".into(),
        claim_id: ClaimId::from(claim_record_id),
        grant_ref: GrantId::from(grant_record_id),
        lease_expiry_ms: expiry,
        epoch,
    });
    vec![
        Fixture {
            file,
            input: grant_input,
            op: signed_record(grant_stream, "owner-a", grant),
        },
        Fixture {
            file,
            input: claim_input,
            op: signed_record(claim_stream, principal, claim),
        },
    ]
}

fn definition(name: &str) -> DefRevId {
    DefRevId::from(record_id_for(&stream("svc", "glade-a", 0xd0), name))
}

fn instance_pair(
    file: &'static str,
    grant_input: &'static str,
    claim_input: &'static str,
    claimed_node: &str,
    grant_def: &str,
    claim_def: &str,
    grant_key: u8,
) -> (Vec<Fixture>, GrantId) {
    let grant_stream = stream("svc", "glade-a", grant_key);
    let claim_stream = stream("svc", "glade-a", 0xaa);
    let grant_record_id = record_id_for(&grant_stream, "execution-authority");
    let claim_record_id = record_id_for(&claim_stream, "derived-worker-a");
    let compute_key = ComputeKey::from(vec![0xc0]);
    let grant_id = GrantId::from(grant_record_id.clone());
    let grant = DirectoryRecord::CapabilityGrant(CapabilityGrant {
        grant_id: grant_id.clone(),
        issuer: Principal::from("execution-authority"),
        principal: Principal::from("derived-worker-a"),
        share: "svc".into(),
        verbs: vec![CapabilityVerb::Execute],
        scope: Some(GrantScope::Execution(ExecutionScope {
            def_ref: definition(grant_def),
            compute_key: compute_key.clone(),
        })),
    });
    let claim = DirectoryRecord::ServiceInstanceClaim(ServiceInstanceClaim {
        node: NodeId::from(claimed_node),
        share: "svc".into(),
        glade_id: "glade-a".into(),
        key: vec![0xaa],
        claim_id: ClaimId::from(claim_record_id),
        def_ref: definition(claim_def),
        exec_grant_ref: grant_id.clone(),
        compute_key,
        lease_expiry_ms: 260_000,
        epoch: 0,
    });
    (
        vec![
            Fixture {
                file,
                input: grant_input,
                op: signed_record(grant_stream, "execution-authority", grant),
            },
            Fixture {
                file,
                input: claim_input,
                op: signed_record(claim_stream, "derived-worker-a", claim),
            },
        ],
        grant_id,
    )
}

fn fixtures() -> Vec<Fixture> {
    let mut fixtures = Vec::new();
    fixtures.extend(workspace_pair(
        "authz/authz-boundary.json",
        ("grant", "claim"),
        ("node-b", "node-principal-b", 460_000, 0),
        0xa1,
    ));

    let (ops, _) = instance_pair(
        "authz/inst-authority.json",
        "exec-grant",
        "instance-claim",
        "worker-a",
        "definition-a",
        "definition-a",
        0xe1,
    );
    fixtures.extend(ops);
    let (ops, _) = instance_pair(
        "authz/inst-forged-node.json",
        "exec-grant",
        "forged-claim",
        "worker-b",
        "definition-a",
        "definition-a",
        0xe2,
    );
    fixtures.extend(ops);
    let (ops, _) = instance_pair(
        "authz/inst-wrong-def.json",
        "def-a-grant",
        "def-b-claim",
        "worker-a",
        "definition-a",
        "definition-b",
        0xe3,
    );
    fixtures.extend(ops);
    let (ops, revoked_grant) = instance_pair(
        "authz/inst-revoked-exec.json",
        "exec-grant",
        "instance-claim",
        "worker-a",
        "definition-a",
        "definition-a",
        0xe4,
    );
    fixtures.extend(ops);
    let revocation_stream = stream("svc", "glade-a", 0xe5);
    fixtures.push(Fixture {
        file: "authz/inst-revoked-exec.json",
        input: "revocation",
        op: signed_record(
            revocation_stream,
            "execution-authority",
            DirectoryRecord::CapabilityRevocation(CapabilityRevocation {
                revokes: revoked_grant,
            }),
        ),
    });

    fixtures.extend(workspace_pair(
        "clock/wall-rollback.json",
        ("grant", "claim"),
        ("node-b", "node-principal-b", 115_000, 0),
        0xb1,
    ));
    fixtures.extend(workspace_pair(
        "clock/restart-uncertain.json",
        ("grant", "claim"),
        ("node-b", "node-principal-b", 160_000, 0),
        0xb2,
    ));
    fixtures.extend(workspace_pair(
        "clock/skew.json",
        ("grant", "claim"),
        ("publisher", "node-principal-b", 112_000, 0),
        0xb3,
    ));

    for file in ["claims/epoch-tie.json", "claims/no-ping-pong.json"] {
        fixtures.extend(workspace_pair(
            file,
            ("grant-a", "claim-a"),
            ("node-a", "node-principal-a", 660_000, 7),
            0xc1,
        ));
        fixtures.extend(workspace_pair(
            file,
            ("grant-b", "claim-b"),
            ("node-b", "node-principal-b", 660_000, 7),
            0xc2,
        ));
    }
    let claim_a = fixtures
        .iter()
        .find(|fixture| fixture.file == "claims/no-ping-pong.json" && fixture.input == "claim-a")
        .expect("claim-a fixture")
        .op
        .clone();
    let claim_b = fixtures
        .iter()
        .find(|fixture| fixture.file == "claims/no-ping-pong.json" && fixture.input == "claim-b")
        .expect("claim-b fixture")
        .op
        .clone();
    fixtures.push(Fixture {
        file: "claims/no-ping-pong.json",
        input: "renew-a",
        op: signed_renewal(&claim_a, 680_000),
    });
    fixtures.push(Fixture {
        file: "claims/no-ping-pong.json",
        input: "renew-b",
        op: signed_renewal(&claim_b, 680_000),
    });

    fixtures.extend(workspace_pair(
        "routing/corr-collision.json",
        ("grant", "claim"),
        ("node-b", "node-principal-b", 760_000, 0),
        0xd1,
    ));
    fixtures.extend(workspace_pair(
        "routing/route-terminal.json",
        ("serve-grant", "serve-claim"),
        ("node-b", "node-principal-b", 760_000, 0),
        0xd2,
    ));
    fixtures
}

fn scenario_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../scenarios")
}

fn input_hex<'a>(scenario: &'a Value, input_id: &str) -> &'a str {
    scenario["inputs"]
        .as_array()
        .expect("inputs array")
        .iter()
        .find(|input| input["input_id"] == input_id)
        .and_then(|input| input["event"]["msg"]["op"]["canonical_hex"].as_str())
        .expect("deliver input canonical_hex")
}

fn decode_hex(hex: &str) -> Vec<u8> {
    assert_eq!(hex.len() % 2, 0, "hex length");
    hex.as_bytes()
        .chunks_exact(2)
        .map(|pair| {
            let pair = std::str::from_utf8(pair).expect("ASCII hex");
            u8::from_str_radix(pair, 16).expect("valid hex")
        })
        .collect()
}

fn encode_hex(bytes: &[u8]) -> String {
    let mut encoded = String::with_capacity(bytes.len().saturating_mul(2));
    for byte in bytes {
        write!(&mut encoded, "{byte:02x}").expect("writing to String cannot fail");
    }
    encoded
}

fn decoded_input(scenario: &Value, input_id: &str) -> SignedOp {
    decode_signed_op(&decode_hex(input_hex(scenario, input_id))).expect("canonical signed op")
}

fn json_record_id(value: &Value) -> RecordId {
    RecordId {
        stream: StreamId {
            share: value["stream"]["share"]
                .as_str()
                .expect("record share")
                .into(),
            glade_id: value["stream"]["glade_id"]
                .as_str()
                .expect("record glade_id")
                .into(),
            key: decode_hex(value["stream"]["key_hex"].as_str().expect("record key hex")),
        },
        origin: Principal::from(value["origin"].as_str().expect("record origin")),
        seq: value["seq"].as_u64().expect("record seq"),
    }
}

#[test]
fn wave_one_ops_are_canonical_typed_records_with_self_consistent_identity() {
    let mut replacements = Vec::new();
    let mut record_ids: BTreeMap<&str, BTreeSet<RecordId>> = BTreeMap::new();
    for fixture in fixtures() {
        let path = scenario_root().join(fixture.file);
        let scenario: Value = serde_json::from_str(&fs::read_to_string(&path).expect("scenario"))
            .expect("scenario JSON");
        let actual_hex = input_hex(&scenario, fixture.input);
        let expected_hex = encode_hex(fixture.op.canonical_bytes());
        if actual_hex != expected_hex {
            replacements.push(format!(
                "{}\t{}\t{}\t{}",
                fixture.file, fixture.input, actual_hex, expected_hex
            ));
            continue;
        }

        let decoded = decode_signed_op(&decode_hex(actual_hex)).expect("canonical signed op");
        let fixture_signer = scenario["verifier_fixtures"]
            .as_array()
            .expect("verifier fixtures")
            .iter()
            .find(|candidate| {
                candidate["selector"]["kind"] == "input"
                    && candidate["selector"]["input_id"] == fixture.input
                    && candidate["selector"]["op_ordinal"] == 0
            })
            .and_then(|candidate| candidate["outcome"]["signer"].as_str())
            .expect("explicit valid signer fixture");
        assert_eq!(fixture_signer, decoded.envelope().origin.as_str());
        record_ids
            .entry(fixture.file)
            .or_default()
            .insert(record_id(&decoded));
        let record = decode_directory_record(&decoded.envelope().payload)
            .expect("typed directory record payload");
        match &record {
            DirectoryRecord::CapabilityGrant(grant) => {
                assert_eq!(grant.grant_id.record(), &record_id(&decoded));
                assert_eq!(grant.issuer, decoded.envelope().origin);
            }
            DirectoryRecord::ServeClaim(claim) => {
                if fixture.input.starts_with("renew-") {
                    assert_eq!(claim.claim_id.record().seq, 0);
                    assert_eq!(record_id(&decoded).seq, 1);
                    assert_eq!(claim.claim_id.record().stream, decoded.envelope().stream);
                    assert_eq!(claim.claim_id.record().origin, decoded.envelope().origin);
                } else {
                    assert_eq!(claim.claim_id.record(), &record_id(&decoded));
                }
                assert_eq!(claim.grant_ref.record().stream.share, claim.share);
            }
            DirectoryRecord::ServiceInstanceClaim(claim) => {
                assert_eq!(claim.claim_id.record(), &record_id(&decoded));
                assert_eq!(claim.share, "svc");
                assert_eq!(claim.glade_id, decoded.envelope().stream.glade_id);
                assert_eq!(claim.key, decoded.envelope().stream.key);
            }
            DirectoryRecord::CapabilityRevocation(_) => {}
        }
        assert_eq!(
            decode_directory_record(&decoded.envelope().payload),
            Ok(record)
        );
    }
    assert!(
        replacements.is_empty(),
        "canonical fixture replacements required:\n{}",
        replacements.join("\n")
    );

    for (file, ids) in record_ids {
        let scenario: Value = serde_json::from_str(
            &fs::read_to_string(scenario_root().join(file)).expect("scenario"),
        )
        .expect("scenario JSON");
        for expectation in scenario["expect"].as_array().expect("expectations") {
            if expectation["kind"] == "state" && expectation["assertion"]["kind"] == "retained" {
                let expected_id = json_record_id(&expectation["assertion"]["record_id"]);
                assert!(
                    ids.contains(&expected_id),
                    "{file} retained expectation does not identify an embedded op: {expected_id:?}"
                );
            }
        }
    }
}

#[test]
fn no_ping_pong_renewals_chain_from_the_stable_claims_and_only_extend_expiry() {
    let path = scenario_root().join("claims/no-ping-pong.json");
    let scenario: Value =
        serde_json::from_str(&fs::read_to_string(path).expect("scenario")).expect("scenario JSON");

    for (initial_input, renewal_input) in [("claim-a", "renew-a"), ("claim-b", "renew-b")] {
        let initial = decoded_input(&scenario, initial_input);
        let renewal = decoded_input(&scenario, renewal_input);

        assert_eq!(initial.envelope().seq, 0, "initial claim starts its chain");
        assert_eq!(initial.envelope().prev, None);
        assert_eq!(renewal.envelope().stream, initial.envelope().stream);
        assert_eq!(renewal.envelope().origin, initial.envelope().origin);
        assert_eq!(renewal.envelope().seq, 1, "renewal advances the chain");
        assert_eq!(renewal.envelope().prev, Some(op_hash(&initial)));

        let DirectoryRecord::ServeClaim(initial_claim) =
            decode_directory_record(&initial.envelope().payload).expect("initial claim")
        else {
            panic!("initial record must be a workspace claim");
        };
        let DirectoryRecord::ServeClaim(renewal_claim) =
            decode_directory_record(&renewal.envelope().payload).expect("renewal claim")
        else {
            panic!("renewal record must be a workspace claim");
        };
        assert!(
            renewal_claim.lease_expiry_ms > initial_claim.lease_expiry_ms,
            "renewal extends the lease"
        );
        assert_eq!(renewal_claim.claim_id, initial_claim.claim_id);
        assert_eq!(renewal_claim.epoch, initial_claim.epoch);

        let mut normalized_renewal = renewal_claim;
        normalized_renewal.lease_expiry_ms = initial_claim.lease_expiry_ms;
        assert_eq!(
            normalized_renewal, initial_claim,
            "expiry is the only changed claim field"
        );
    }
}
