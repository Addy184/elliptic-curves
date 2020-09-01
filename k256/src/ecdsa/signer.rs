//! ECDSA signer

use super::{recoverable, Error, Signature};
use crate::{ElementBytes, NonZeroScalar, ProjectivePoint, Scalar, Secp256k1, SecretKey};
use core::borrow::Borrow;
use ecdsa_core::{
    hazmat::RecoverableSignPrimitive,
    signature::{self, DigestSigner, RandomizedDigestSigner},
    signer::rfc6979,
};
use elliptic_curve::{
    consts::U32,
    digest::{BlockInput, FixedOutput, Reset, Update},
    ops::Invert,
    rand_core::{CryptoRng, RngCore},
    FromBytes, FromDigest,
};
use signature::PrehashSignature;

#[cfg(any(feature = "sha256", feature = "keccak256"))]
use signature::digest::Digest;

/// ECDSA/secp256k1 signer
#[cfg_attr(docsrs, doc(cfg(feature = "ecdsa")))]
pub struct Signer {
    /// Secret scalar value
    secret_scalar: NonZeroScalar,
}

impl Signer {
    /// Create a new signer
    pub fn new(secret_key: &SecretKey) -> Result<Self, Error> {
        let scalar = NonZeroScalar::from_bytes(secret_key.as_bytes());

        // TODO(tarcieri): replace with into conversion when available (see subtle#73)
        if scalar.is_some().into() {
            Ok(Self::from(scalar.unwrap()))
        } else {
            Err(Error::new())
        }
    }
}

impl<S> signature::Signer<S> for Signer
where
    S: PrehashSignature,
    Self: DigestSigner<S::Digest, S>,
{
    fn try_sign(&self, msg: &[u8]) -> Result<S, Error> {
        self.try_sign_digest(Digest::chain(S::Digest::new(), msg))
    }
}

impl<D> DigestSigner<D, Signature> for Signer
where
    D: BlockInput + FixedOutput<OutputSize = U32> + Clone + Default + Reset + Update,
{
    fn try_sign_digest(&self, digest: D) -> Result<Signature, Error> {
        ecdsa_core::Signer::from(self.secret_scalar).try_sign_digest(digest)
    }
}

impl<D> DigestSigner<D, recoverable::Signature> for Signer
where
    D: BlockInput + FixedOutput<OutputSize = U32> + Clone + Default + Reset + Update,
{
    fn try_sign_digest(&self, digest: D) -> Result<recoverable::Signature, Error> {
        let ephemeral_scalar = rfc6979::generate_k(&self.secret_scalar, digest.clone(), &[]);
        let msg_scalar = Scalar::from_digest(digest);
        self.secret_scalar
            .try_sign_recoverable_prehashed(ephemeral_scalar.as_ref(), &msg_scalar)
    }
}

impl<D> RandomizedDigestSigner<D, Signature> for Signer
where
    D: BlockInput + FixedOutput<OutputSize = U32> + Clone + Default + Reset + Update,
{
    fn try_sign_digest_with_rng(
        &self,
        rng: impl CryptoRng + RngCore,
        digest: D,
    ) -> Result<Signature, Error> {
        ecdsa_core::Signer::from(self.secret_scalar).try_sign_digest_with_rng(rng, digest)
    }
}

impl<D> RandomizedDigestSigner<D, recoverable::Signature> for Signer
where
    D: BlockInput + FixedOutput<OutputSize = U32> + Clone + Default + Reset + Update,
{
    fn try_sign_digest_with_rng(
        &self,
        mut rng: impl CryptoRng + RngCore,
        digest: D,
    ) -> Result<recoverable::Signature, Error> {
        let mut added_entropy = ElementBytes::default();
        rng.fill_bytes(&mut added_entropy);

        let ephemeral_scalar =
            rfc6979::generate_k(&self.secret_scalar, digest.clone(), &added_entropy);

        let msg_scalar = Scalar::from_digest(digest);
        self.secret_scalar
            .try_sign_recoverable_prehashed(ephemeral_scalar.as_ref(), &msg_scalar)
    }
}

impl RecoverableSignPrimitive<Secp256k1> for Scalar {
    type RecoverableSignature = recoverable::Signature;

    #[allow(non_snake_case, clippy::many_single_char_names)]
    fn try_sign_recoverable_prehashed<K>(
        &self,
        ephemeral_scalar: &K,
        z: &Scalar,
    ) -> Result<recoverable::Signature, Error>
    where
        K: Borrow<Scalar> + Invert<Output = Scalar>,
    {
        let k_inverse = ephemeral_scalar.invert();
        let k = ephemeral_scalar.borrow();

        if k_inverse.is_none().into() || k.is_zero().into() {
            return Err(Error::new());
        }

        let k_inverse = k_inverse.unwrap();

        // Compute 𝐑 = 𝑘×𝑮
        let R = (ProjectivePoint::generator() * k).to_affine().unwrap();

        // Lift x-coordinate of 𝐑 (element of base field) into a serialized big
        // integer, then reduce it into an element of the scalar field
        let r = Scalar::from_bytes_reduced(&R.x.to_bytes());

        // Compute `s` as a signature over `r` and `z`.
        let s = k_inverse * &(z + (r * self));

        if s.is_zero().into() {
            return Err(Error::new());
        }

        let mut signature = Signature::from_scalars(&r.into(), &s.into());
        let is_r_odd = bool::from(R.y.normalize().is_odd());
        let is_s_high = signature.normalize_s()?;
        let recovery_id = recoverable::Id((is_r_odd ^ is_s_high) as u8);
        recoverable::Signature::new(&signature, recovery_id)
    }
}

impl From<NonZeroScalar> for Signer {
    fn from(secret_scalar: NonZeroScalar) -> Self {
        Self { secret_scalar }
    }
}

#[cfg(test)]
mod tests {
    use crate::{test_vectors::ecdsa::ECDSA_TEST_VECTORS, Secp256k1};
    ecdsa_core::new_signing_test!(Secp256k1, ECDSA_TEST_VECTORS);
}
