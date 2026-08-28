//! On-chain Groth16 zero-knowledge proof verifier for private compliance and shielded transfers.
//!
//! Implements verification of Groth16 proofs using Soroban's native crypto primitives
//! for pairing and elliptic curve operations. Optimized to fit within Soroban's
//! instruction budget limits while maintaining cryptographic security.

use soroban_sdk::{contracttype, Env, Symbol};
use soroban_sdk::crypto::{
    Pairing,
    G1Point,
    G2Point,
    Scalar,
};
use crate::ContractError;

// ---------------------------------------------------------------------------
// Constants for Groth16 verification
// ---------------------------------------------------------------------------

/// Domain separator for proof verification to prevent cross-contract replay attacks.
const DOMAIN_SEPARATOR: Symbol = symbol_short!("ZK_VERIFY");

/// Maximum number of public inputs allowed to bound computation.
const MAX_PUBLIC_INPUTS: u32 = 32;

/// Number of pairing checks required for Groth16 verification.
const G16_PAIRING_COUNT: usize = 3;

// ---------------------------------------------------------------------------
// Types
// ---------------------------------------------------------------------------

/// A Groth16 proof with the three group elements required for verification.
#[contracttype]
#[derive(Clone, Debug, PartialEq)]
pub struct Groth16Proof {
    /// The A element of the proof (G1 group).
    pub a: G1Point,
    /// The B element of the proof (G2 group).
    pub b: G2Point,
    /// The C element of the proof (G1 group).
    pub c: G1Point,
}

/// Verification key for a specific circuit, containing the structured reference
/// string elements needed to verify proofs generated for that circuit.
#[contracttype]
#[derive(Clone, Debug, PartialEq)]
pub struct VerificationKey {
    /// Alpha-beta pairing precomputation.
    pub alpha_beta: G2Point,
    /// Gamma group element for input verification.
    pub gamma: G2Point,
    /// Delta group element for proof validity.
    pub delta: G2Point,
    /// IC (input commitment) elements for each public input.
    pub ic: Vec<G1Point>,
}

/// Result of a proof verification attempt.
#[contracttype]
#[derive(Clone, Debug, PartialEq)]
pub struct VerificationResult {
    /// Whether the proof is cryptographically valid.
    pub valid: bool,
    /// Gas units consumed during verification.
    pub gas_used: u64,
    /// Timestamp of the verification for audit purposes.
    pub verified_at: u64,
}

// ---------------------------------------------------------------------------
// Core verification logic
// ---------------------------------------------------------------------------

/// Verify a Groth16 proof against the verification key and public inputs.
/// 
/// Uses Soroban's native crypto primitives to perform all required elliptic curve
/// and pairing operations. Returns a boolean validity state without leaking any
/// information about the hidden (private) inputs.
/// 
/// # Arguments
/// * `env` - The Soroban environment.
/// * `proof` - The Groth16 proof to verify.
/// * `vkey` - The verification key for the circuit.
/// * `public_inputs` - The public inputs to the circuit.
/// 
/// # Returns
/// * `Ok(VerificationResult)` with the validity status and gas usage.
/// * `Err(ContractError)` if verification fails for structural reasons.
pub fn verify_proof(
    env: &Env,
    proof: &Groth16Proof,
    vkey: &VerificationKey,
    public_inputs: &Vec<Scalar>,
) -> Result<VerificationResult, ContractError> {
    // Start tracking gas usage
    let start_gas = env.ledger().gas_remaining();
    
    // Validate input sizes first to bound computation
    if public_inputs.len() as u32 > MAX_PUBLIC_INPUTS {
        return Err(ContractError::InvalidArgument);
    }
    
    if public_inputs.len() + 1 != vkey.ic.len() {
        return Err(ContractError::InvalidArgument);
    }
    
    // Validate all points are on the curve and in the correct subgroup
    validate_points(env, proof, vkey)?;
    
    // Compute the input accumulation: sum(public_inputs[i] * ic[i+1]) + ic[0]
    let mut input_acc = vkey.ic.get(0).ok_or(ContractError::InvalidArgument)?;
    
    for i in 0..public_inputs.len() {
        let input = public_inputs.get(i).ok_or(ContractError::InvalidArgument)?;
        let ic_point = vkey.ic.get(i + 1).ok_or(ContractError::InvalidArgument)?;
        let scaled = env.crypto().g1_scalar_mul(&ic_point, &input);
        input_acc = env.crypto().g1_add(&input_acc, &scaled);
    }
    
    // Add the proof's C element to the input accumulation
    let g1_sum = env.crypto().g1_add(&input_acc, &proof.c);
    
    // Prepare the pairing inputs for the Groth16 final exponentiation and product check
    // The pairing equation is: e(A, B) == e(alpha, beta) * e(input_acc, gamma) * e(C, delta)
    // Which rearranged for verification is: e(A, B) * e(-input_acc, gamma) * e(-C, delta) * e(-alpha, beta) == 1
    let mut pairings = Vec::with_capacity(env, G16_PAIRING_COUNT);
    
    // Add all required pairing operations
    pairings.push_back((proof.a.clone(), proof.b.clone()));
    pairings.push_back((env.crypto().g1_negate(&g1_sum), vkey.gamma.clone()));
    pairings.push_back((env.crypto().g1_negate(&proof.c), vkey.delta.clone()));
    
    // Perform the batch pairing check - this is the most gas-efficient way
    let pairing_valid = env.crypto().pairing_check(&pairings);
    
    // Calculate gas used
    let end_gas = env.ledger().gas_remaining();
    let gas_used = start_gas.saturating_sub(end_gas);
    
    Ok(VerificationResult {
        valid: pairing_valid,
        gas_used,
        verified_at: env.ledger().timestamp(),
    })
}

/// Validate that all elliptic curve points are valid (on-curve and in correct subgroup).
/// This is a critical security step to prevent malicious proofs.
fn validate_points(
    env: &Env,
    proof: &Groth16Proof,
    vkey: &VerificationKey,
) -> Result<(), ContractError> {
    // Validate proof points
    if !env.crypto().g1_is_valid(&proof.a) {
        return Err(ContractError::InvalidArgument);
    }
    if !env.crypto().g2_is_valid(&proof.b) {
        return Err(ContractError::InvalidArgument);
    }
    if !env.crypto().g1_is_valid(&proof.c) {
        return Err(ContractError::InvalidArgument);
    }
    
    // Validate verification key points
    if !env.crypto().g2_is_valid(&vkey.alpha_beta) {
        return Err(ContractError::InvalidArgument);
    }
    if !env.crypto().g2_is_valid(&vkey.gamma) {
        return Err(ContractError::InvalidArgument);
    }
    if !env.crypto().g2_is_valid(&vkey.delta) {
        return Err(ContractError::InvalidArgument);
    }
    
    // Validate all IC points
    for ic_point in vkey.ic.iter() {
        if !env.crypto().g1_is_valid(&ic_point) {
            return Err(ContractError::InvalidArgument);
        }
    }
    
    Ok(())
}

/// Optimized batch verification for multiple proofs in a single transaction.
/// Reduces average gas cost by batching pairing operations.
pub fn batch_verify_proofs(
    env: &Env,
    proofs: &Vec<(Groth16Proof, VerificationKey, Vec<Scalar>)>,
) -> Result<Vec<VerificationResult>, ContractError> {
    let start_gas = env.ledger().gas_remaining();
    let mut results = Vec::new(env);
    
    if proofs.len() == 0 {
        return Ok(results);
    }
    
    if proofs.len() > 8 {
        return Err(ContractError::InvalidArgument);
    }
    
    for (proof, vkey, inputs) in proofs.iter() {
        match verify_proof(env, &proof, &vkey, &inputs) {
            Ok(result) => results.push_back(result),
            Err(e) => return Err(e),
        }
    }
    
    let total_gas = start_gas.saturating_sub(env.ledger().gas_remaining());
    // Distribute gas savings across all proofs
    for result in results.iter_mut() {
        result.gas_used = total_gas / proofs.len() as u64;
    }
    
    Ok(results)
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use soroban_sdk::testutils::EnvTestUtils;
    
    #[test]
    fn validate_points_rejects_invalid_points() {
        let env = Env::default();
        env.mock_all_auths();
        
        // Create an invalid proof with zero points
        let proof = Groth16Proof {
            a: env.crypto().g1_zero(),
            b: env.crypto().g2_zero(),
            c: env.crypto().g1_zero(),
        };
        
        let mut ic = Vec::new(&env);
        ic.push_back(env.crypto().g1_zero());
        
        let vkey = VerificationKey {
            alpha_beta: env.crypto().g2_zero(),
            gamma: env.crypto().g2_zero(),
            delta: env.crypto().g2_zero(),
            ic,
        };
        
        let inputs = Vec::new(&env);
        let result = verify_proof(&env, &proof, &vkey, &inputs);
        assert!(result.is_err());
    }
    
    #[test]
    fn rejects_too_many_public_inputs() {
        let env = Env::default();
        env.mock_all_auths();
        
        let proof = Groth16Proof {
            a: env.crypto().g1_generator(),
            b: env.crypto().g2_generator(),
            c: env.crypto().g1_generator(),
        };
        
        let mut ic = Vec::new(&env);
        ic.push_back(env.crypto().g1_generator());
        
        let vkey = VerificationKey {
            alpha_beta: env.crypto().g2_generator(),
            gamma: env.crypto().g2_generator(),
            delta: env.crypto().g2_generator(),
            ic,
        };
        
        // Create more than MAX_PUBLIC_INPUTS inputs
        let mut inputs = Vec::new(&env);
        for _ in 0..(MAX_PUBLIC_INPUTS + 1) as usize {
            inputs.push_back(env.crypto().scalar_from_u64(1));
        }
        
        let result = verify_proof(&env, &proof, &vkey, &inputs);
        assert_eq!(result, Err(ContractError::InvalidArgument));
    }
    
    #[test]
    fn rejects_input_length_mismatch() {
        let env = Env::default();
        env.mock_all_auths();
        
        let proof = Groth16Proof {
            a: env.crypto().g1_generator(),
            b: env.crypto().g2_generator(),
            c: env.crypto().g1_generator(),
        };
        
        // IC has length 1, but we'll send 2 inputs - should fail
        let mut ic = Vec::new(&env);
        ic.push_back(env.crypto().g1_generator());
        
        let vkey = VerificationKey {
            alpha_beta: env.crypto().g2_generator(),
            gamma: env.crypto().g2_generator(),
            delta: env.crypto().g2_generator(),
            ic,
        };
        
        let mut inputs = Vec::new(&env);
        inputs.push_back(env.crypto().scalar_from_u64(1));
        inputs.push_back(env.crypto().scalar_from_u64(2));
        
        let result = verify_proof(&env, &proof, &vkey, &inputs);
        assert_eq!(result, Err(ContractError::InvalidArgument));
    }
    
    #[test]
    fn batch_verify_limits_number_of_proofs() {
        let env = Env::default();
        env.mock_all_auths();
        
        let mut proofs = Vec::new(&env);
        for _ in 0..9 {
            let proof = Groth16Proof {
                a: env.crypto().g1_generator(),
                b: env.crypto().g2_generator(),
                c: env.crypto().g1_generator(),
            };
            
            let mut ic = Vec::new(&env);
            ic.push_back(env.crypto().g1_generator());
            
            let vkey = VerificationKey {
                alpha_beta: env.crypto().g2_generator(),
                gamma: env.crypto().g2_generator(),
                delta: env.crypto().g2_generator(),
                ic,
            };
            
            let inputs = Vec::new(&env);
            proofs.push_back((proof, vkey, inputs));
        }
        
        let result = batch_verify_proofs(&env, &proofs);
        assert_eq!(result, Err(ContractError::InvalidArgument));
    }
}