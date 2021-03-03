// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at http://mozilla.org/MPL/2.0/.
//
// Copyright (c) DUSK NETWORK. All rights reserved.

//! Tools & traits for PLONK circuits

use crate::commitment_scheme::kzg10::PublicParameters;
use crate::constraint_system::StandardComposer;
use crate::error::Error;
use crate::proof_system::{Proof, ProverKey, VerifierKey};
#[cfg(feature = "canon")]
use canonical::Canon;
#[cfg(feature = "canon")]
use canonical_derive::Canon;
use dusk_bls12_381::BlsScalar;
use dusk_jubjub::{JubJubAffine, JubJubScalar};

#[derive(Default, Debug, Clone)]
#[cfg_attr(feature = "canon", derive(Canon))]
/// Structure that represents a PLONK Circuit Public Input
/// structure converted into it's &[BlsScalar] repr.
pub struct PublicInputValue(pub(crate) Vec<BlsScalar>);

impl From<BlsScalar> for PublicInputValue {
    fn from(scalar: BlsScalar) -> Self {
        Self(vec![scalar])
    }
}

impl From<JubJubScalar> for PublicInputValue {
    fn from(scalar: JubJubScalar) -> Self {
        Self(vec![scalar.into()])
    }
}

impl From<JubJubAffine> for PublicInputValue {
    fn from(point: JubJubAffine) -> Self {
        Self(vec![point.get_x(), point.get_y()])
    }
}

type PublicInputPositions = Vec<usize>;

/// Circuit representation for a gadget with all of the tools that it
/// should implement.
pub trait Circuit<'a, const N: usize>
where
    Self: Sized,
{
    /// Initialization string used to fill the transcript for both parties.
    const TRANSCRIPT_INIT: &'static [u8];
    /// Trimming size for the keys of the circuit.
    const TRIM_SIZE: usize = N;
    /// Gadget implementation used to fill the composer.
    fn gadget(&mut self, composer: &mut StandardComposer) -> Result<(), Error>;
    /// Compiles the circuit by using a function that returns a `Result`
    /// with the `ProverKey`, `VerifierKey` and the circuit size.
    fn compile(
        &mut self,
        pub_params: &PublicParameters,
    ) -> Result<(ProverKey, VerifierKey, PublicInputPositions), Error> {
        use crate::proof_system::{Prover, Verifier};
        // Setup PublicParams
        let (ck, _) = pub_params.trim(Self::TRIM_SIZE)?;
        // Generate & save `ProverKey` with some random values.
        let mut prover = Prover::new(b"CircuitCompilation");
        self.gadget(prover.mut_cs())?;
        let pi_pos = prover.mut_cs().pi_positions();
        prover.preprocess(&ck)?;

        // Generate & save `VerifierKey` with some random values.
        let mut verifier = Verifier::new(b"CircuitCompilation");
        self.gadget(verifier.mut_cs())?;
        verifier.preprocess(&ck)?;
        Ok((
            prover
                .prover_key
                .expect("Unexpected error. Missing ProverKey in compilation"),
            verifier
                .verifier_key
                .expect("Unexpected error. Missing VerifierKey in compilation"),
            pi_pos,
        ))
    }

    /// Build PI vector for Proof verifications.
    fn build_pi(
        &self,
        pub_input_values: &[PublicInputValue],
        pub_input_pos: &PublicInputPositions,
    ) -> Vec<BlsScalar> {
        let mut pi = vec![BlsScalar::zero(); Self::TRIM_SIZE];
        pub_input_values
            .iter()
            .map(|pub_input| pub_input.0.clone())
            .flatten()
            .zip(pub_input_pos.iter())
            .for_each(|(value, pos)| {
                pi[*pos] = -value;
            });
        pi
    }

    /// Generates a proof using the provided `CircuitInputs` & `ProverKey` instances.
    fn gen_proof(
        &mut self,
        pub_params: &PublicParameters,
        prover_key: &ProverKey,
    ) -> Result<Proof, Error> {
        use crate::proof_system::Prover;
        let (ck, _) = pub_params.trim(Self::TRIM_SIZE)?;
        // New Prover instance
        let mut prover = Prover::new(Self::TRANSCRIPT_INIT);
        // Fill witnesses for Prover
        self.gadget(prover.mut_cs())?;
        // Add ProverKey to Prover
        prover.prover_key = Some(prover_key.clone());
        prover.prove(&ck)
    }

    /// Verifies a proof using the provided `CircuitInputs` & `VerifierKey` instances.
    fn verify_proof(
        &mut self,
        pub_params: &PublicParameters,
        verifier_key: &VerifierKey,
        proof: &Proof,
        pub_inputs_values: &[PublicInputValue],
        pub_inputs_positions: &PublicInputPositions,
    ) -> Result<(), Error> {
        use crate::proof_system::Verifier;
        let (_, vk) = pub_params.trim(Self::TRIM_SIZE)?;

        let mut verifier = Verifier::new(Self::TRANSCRIPT_INIT);
        verifier.verifier_key = Some(*verifier_key);
        verifier.verify(
            proof,
            &vk,
            &self.build_pi(pub_inputs_values, pub_inputs_positions),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::constraint_system::{ecc::*, StandardComposer};
    use crate::proof_system::{ProverKey, VerifierKey};

    // Implements a circuit that checks:
    // 1) a + b = c where C is a PI
    // 2) a <= 2^6
    // 3) b <= 2^5
    // 4) a * b = d where D is a PI
    // 5) JubJub::GENERATOR * e(JubJubScalar) = f where F is a PI
    #[derive(Debug, Default)]
    pub struct TestCircuit {
        a: BlsScalar,
        b: BlsScalar,
        c: BlsScalar,
        d: BlsScalar,
        e: JubJubScalar,
        f: JubJubAffine,
    }

    impl Circuit<'_, { 1 << 11 }> for TestCircuit {
        const TRANSCRIPT_INIT: &'static [u8] = b"Test";
        fn gadget(&mut self, composer: &mut StandardComposer) -> Result<(), Error> {
            let a = composer.add_input(self.a);
            let b = composer.add_input(self.b);
            // Make first constraint a + b = c
            composer.poly_gate(
                a,
                b,
                composer.zero_var,
                BlsScalar::zero(),
                BlsScalar::one(),
                BlsScalar::one(),
                BlsScalar::zero(),
                BlsScalar::zero(),
                Some(-self.c),
            );
            // Check that a and b are in range
            composer.range_gate(a, 1 << 6);
            composer.range_gate(b, 1 << 5);
            // Make second constraint a * b = d
            composer.poly_gate(
                a,
                b,
                composer.zero_var,
                BlsScalar::one(),
                BlsScalar::zero(),
                BlsScalar::zero(),
                BlsScalar::one(),
                BlsScalar::zero(),
                Some(-self.d),
            );

            // This adds a PI also constraining `generator` to actually be `dusk_jubjub::GENERATOR`
            let generator = Point::from_public_affine(composer, dusk_jubjub::GENERATOR);
            let e = composer.add_input(self.e.into());
            let scalar_mul_result =
                scalar_mul::variable_base::variable_base_scalar_mul(composer, e, generator);
            // Apply the constrain
            composer.assert_equal_public_point(scalar_mul_result.into(), self.f);
            println!("{:?}", composer.public_inputs_sparse_store.values());
            Ok(())
        }
    }

    #[test]
    fn test_full() {
        use std::fs::{self, File};
        use std::io::Write;
        use tempdir::TempDir;

        let tmp = TempDir::new("plonk-keys-test-full").unwrap().into_path();
        let pp_path = tmp.clone().join("pp_testcirc");
        let pk_path = tmp.clone().join("pk_testcirc");
        let vk_path = tmp.clone().join("vk_testcirc");

        // Generate CRS
        let pp_p = PublicParameters::setup(1 << 12, &mut rand::thread_rng()).unwrap();
        File::create(&pp_path)
            .and_then(|mut f| f.write(pp_p.to_raw_bytes().as_slice()))
            .unwrap();

        // Read PublicParameters
        let pp = fs::read(pp_path).unwrap();
        let pp = unsafe { PublicParameters::from_slice_unchecked(pp.as_slice()).unwrap() };

        // Initialize the circuit
        let mut circuit = TestCircuit::default();

        // Compile the circuit
        let (pk_p, vk_p, pi_pos) = circuit.compile(&pp).unwrap();

        // Write the keys
        File::create(&pk_path)
            .and_then(|mut f| f.write(pk_p.to_bytes().as_slice()))
            .unwrap();
        File::create(&vk_path)
            .and_then(|mut f| f.write(vk_p.to_bytes().as_slice()))
            .unwrap();

        // Read ProverKey
        let pk = fs::read(pk_path).unwrap();
        let pk = ProverKey::from_bytes(pk.as_slice()).unwrap();

        // Read VerifierKey
        let vk = fs::read(vk_path).unwrap();
        let vk = VerifierKey::from_bytes(vk.as_slice()).unwrap();

        assert_eq!(pk, pk_p);
        assert_eq!(vk, vk_p);

        // Prover POV
        let proof = {
            let mut circuit = TestCircuit {
                a: BlsScalar::from(20u64),
                b: BlsScalar::from(5u64),
                c: BlsScalar::from(25u64),
                d: BlsScalar::from(100u64),
                e: JubJubScalar::from(2u64),
                f: JubJubAffine::from(dusk_jubjub::GENERATOR_EXTENDED * JubJubScalar::from(2u64)),
            };

            circuit.gen_proof(&pp, &pk)
        }
        .unwrap();

        // Verifier POV
        let mut circuit = TestCircuit::default();
        let public_inputs2: Vec<PublicInputValue> = vec![
            BlsScalar::from(25u64).into(),
            BlsScalar::from(100u64).into(),
            dusk_jubjub::GENERATOR.into(),
            JubJubAffine::from(dusk_jubjub::GENERATOR_EXTENDED * JubJubScalar::from(2u64)).into(),
        ];

        assert!(circuit
            .verify_proof(&pp, &vk, &proof, &public_inputs2, &pi_pos)
            .is_ok());
    }
}
