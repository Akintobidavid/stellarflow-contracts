use soroban_sdk::{contracttype, Env, Bytes, BytesN, Vec, Address};
use crate::ContractError;

#[contracttype]
pub enum RelayerStorageKey {
    Threshold,
    Validator(BytesN<32>),
    Nonce(BytesN<32>),
}

pub fn configure_threshold(env: &Env, admin: &Address, threshold: u32) {
    admin.require_auth();
    env.storage().instance().set(&RelayerStorageKey::Threshold, &threshold);
}

pub fn add_validator(env: &Env, admin: &Address, pubkey: BytesN<32>) {
    admin.require_auth();
    env.storage().instance().set(&RelayerStorageKey::Validator(pubkey), &true);
}

pub fn remove_validator(env: &Env, admin: &Address, pubkey: BytesN<32>) {
    admin.require_auth();
    env.storage().instance().remove(&RelayerStorageKey::Validator(pubkey));
}

/// Verify a cross-chain payload hash against a list of multi-sig signatures.
/// 
/// Prevents message replay and enforces a minimum required threshold of authorized validators.
pub fn verify_cross_chain_payload(
    env: &Env,
    payload_hash: BytesN<32>,
    signatures: Vec<(BytesN<32>, BytesN<64>)>,
) -> Result<(), ContractError> {
    // 1. Prevent replay attacks
    let nonce_key = RelayerStorageKey::Nonce(payload_hash.clone());
    if env.storage().persistent().has(&nonce_key) {
        return Err(ContractError::InvalidProof);
    }

    // 2. Load threshold
    let threshold: u32 = env
        .storage()
        .instance()
        .get(&RelayerStorageKey::Threshold)
        .unwrap_or(0);

    if threshold == 0 || signatures.len() < threshold {
        return Err(ContractError::InvalidProof);
    }

    // 3. Verify signatures
    let mut valid_count = 0;
    let mut payload_bytes = Bytes::new(env);
    payload_bytes.append(&Bytes::from_slice(env, &payload_hash.to_array()));

    for sig in signatures.iter() {
        let (pubkey, signature) = sig;
        
        let val_key = RelayerStorageKey::Validator(pubkey.clone());
        if !env.storage().instance().has(&val_key) {
            continue;
        }

        // Verify the signature using the native SDK.
        env.crypto().ed25519_verify(&pubkey, &payload_bytes, &signature);
        
        valid_count += 1;
        if valid_count >= threshold {
            break;
        }
    }

    if valid_count < threshold {
        return Err(ContractError::InvalidProof);
    }

    // 4. Record the nonce to prevent replay attacks
    env.storage().persistent().set(&nonce_key, &true);

    Ok(())
}
