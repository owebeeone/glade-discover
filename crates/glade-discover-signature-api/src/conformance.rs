//! Reusable suite; the fixture signer MUST permit the probe envelope and the
//! verifier MUST have its authentic key. No real cryptographic backend is shipped.
//! Implementations also need algorithm vectors and key-resolution security tests.

use crate::{SignError, SignatureStatus, Signer, VerificationError, Verifier};
use glade_discover_protocol::{
    OpEnvelope, Principal, Shape, StreamId, encode_signed_op, unsigned_canonical_bytes,
};

fn envelope(origin: Principal) -> OpEnvelope {
    OpEnvelope {
        stream: StreamId::default(),
        origin,
        seq: 1,
        prev: None,
        lamport: 1,
        refs: vec![],
        shape: Shape::Log,
        payload: vec![1, 2, 3],
    }
}

pub async fn round_trip_and_tamper<S: Signer, V: Verifier>(signer: &S, verifier: &V) {
    let identity = signer.principal();
    let envelope = envelope(identity.clone());
    let unsigned = encode_signed_op(&envelope, &[]).unwrap();
    let signature = signer
        .sign(&unsigned_canonical_bytes(&unsigned))
        .await
        .expect("SG-001 signing");
    assert_eq!(signer.principal(), identity, "SG-001 stable identity");
    let op = encode_signed_op(&envelope, &signature).unwrap();
    assert_eq!(
        verifier.verify(&op).await,
        Ok(SignatureStatus::Valid { signer: identity }),
        "SG-001 authenticated identity"
    );

    let mut changed = envelope.clone();
    changed.payload.push(99);
    let tampered = encode_signed_op(&changed, &signature).unwrap();
    assert_eq!(
        verifier.verify(&tampered).await,
        Ok(SignatureStatus::Invalid),
        "SG-002 payload binding"
    );

    changed = envelope.clone();
    changed.origin = Principal::from(format!("{}-other", envelope.origin));
    let forged_origin = encode_signed_op(&changed, &signature).unwrap();
    assert!(
        matches!(
            verifier.verify(&forged_origin).await,
            Ok(SignatureStatus::Invalid) | Err(VerificationError::Unavailable)
        ),
        "SG-002 origin binding must fail closed; an unknown key is not proof of invalidity"
    );

    let mut bad_signature = signature;
    if let Some(first) = bad_signature.first_mut() {
        *first ^= 0xff;
    } else {
        bad_signature.push(0xff);
    }
    let forged = encode_signed_op(&envelope, &bad_signature).unwrap();
    assert_eq!(
        verifier.verify(&forged).await,
        Ok(SignatureStatus::Invalid),
        "SG-002 signature binding"
    );
}

/// Both backends MUST be configured as unavailable, not merely invalid inputs.
pub async fn unavailable<S: Signer, V: Verifier>(signer: &S, verifier: &V) {
    let op = encode_signed_op(&envelope(signer.principal()), &[]).unwrap();
    assert_eq!(
        signer.sign(&unsigned_canonical_bytes(&op)).await,
        Err(SignError::Unavailable),
        "SG-003 signer error"
    );
    assert_eq!(
        verifier.verify(&op).await,
        Err(VerificationError::Unavailable),
        "SG-003 verifier error"
    );
}

/// An independent observer MUST count started backend effects (not just successes).
pub fn unpolled<S: Signer, V: Verifier>(signer: &S, verifier: &V, effects: impl Fn() -> usize) {
    let op = encode_signed_op(&envelope(signer.principal()), &[]).unwrap();
    let bytes = unsigned_canonical_bytes(&op);
    let before = effects();
    let signing = signer.sign(&bytes);
    assert_eq!(effects(), before, "SG-004 eager signing");
    drop(signing);
    assert_eq!(effects(), before, "SG-004 signing drop");
    let verification = verifier.verify(&op);
    assert_eq!(effects(), before, "SG-004 eager verification");
    drop(verification);
    assert_eq!(effects(), before, "SG-004 verification drop");
}
