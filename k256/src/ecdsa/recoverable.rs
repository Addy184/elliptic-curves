//! Ethereum-style "recoverable signatures".
//!
//! These signatures include an additional [`Id`] field which allows for
//! recovery of the [`EncodedPoint`] used to create them. This is helpful in
//! cases where the hash/fingerprint of a key used to create a signature is
//! known in advance.
//!
//! ## Signing/Recovery Example
//!
//! ```
//! # #[cfg(feature = "ecdsa")]
//! # {
//! use k256::{
//!     ecdsa::{SigningKey, recoverable, signature::Signer},
//!     elliptic_curve::{Generate, rand_core::OsRng},
//!     EncodedPoint
//! };
//!
//! // Signing
//! let signing_key = SigningKey::generate(&mut OsRng); // Serialize with `::to_bytes()`
//! let verify_key = signing_key.verify_key();
//! let message = b"ECDSA proves knowledge of a secret number in the context of a single message";
//!
//! // Note: the signature type must be annotated or otherwise inferrable as
//! // `Signer` has many impls of the `Signer` trait (for both regular and
//! // recoverable signature types).
//! let signature: recoverable::Signature = signing_key.sign(message);
//! let recovered_key = signature.recover_verify_key(message).expect("couldn't recover pubkey");
//!
//! assert_eq!(&verify_key, &recovered_key);
//! # }
//! ```

use core::{
    convert::{TryFrom, TryInto},
    fmt::{self, Debug},
};
use ecdsa_core::{signature::Signature as _, Error};

#[cfg(feature = "ecdsa")]
use crate::{
    ecdsa::VerifyKey,
    elliptic_curve::{
        consts::U32, ops::Invert, subtle::Choice, weierstrass::point::Decompress, Digest,
        FromBytes, FromDigest,
    },
    AffinePoint, NonZeroScalar, ProjectivePoint, Scalar,
};

#[cfg(any(feature = "ecdsa", docsrs))]
use crate::EncodedPoint;

#[cfg(feature = "keccak256")]
use sha3::Keccak256;

/// Size of an Ethereum-style recoverable signature in bytes
pub const SIZE: usize = 65;

/// Ethereum-style "recoverable signatures" which allow for the recovery of
/// the signer's [`EncodedPoint`] from the signature itself.
///
/// This format consists of [`Signature`] followed by a 1-byte recovery [`Id`]
/// (65-bytes total):
///
/// - `r`: 32-byte integer, big endian
/// - `s`: 32-byte integer, big endian
/// - `v`: 1-byte recovery [`Id`]
#[derive(Copy, Clone)]
pub struct Signature {
    bytes: [u8; SIZE],
}

impl Signature {
    /// Create a new recoverable ECDSA/secp256k1 signature from a regular
    /// fixed-size signature and an associated recovery [`Id`].
    ///
    /// This is an "unchecked" conversion and assumes the provided [`Id`]
    /// is valid for this signature.
    pub fn new(signature: &super::Signature, recovery_id: Id) -> Result<Self, Error> {
        #[cfg(feature = "ecdsa")]
        super::check_scalars(signature)?;

        let mut bytes = [0u8; SIZE];
        bytes[..64].copy_from_slice(signature.as_ref());
        bytes[64] = recovery_id.0;
        Ok(Self { bytes })
    }

    /// Get the recovery [`Id`] for this signature
    pub fn recovery_id(self) -> Id {
        self.bytes[64].try_into().expect("invalid recovery ID")
    }

    /// Given a public key, message, and signature, use trial recovery for both
    /// possible recovery IDs in an attempt to determine if a suitable
    /// recovery ID exists, or return an error otherwise.
    #[cfg(feature = "ecdsa")]
    #[cfg_attr(docsrs, doc(cfg(feature = "ecdsa")))]
    pub fn from_trial_recovery(
        public_key: &EncodedPoint,
        msg: &[u8],
        signature: &super::Signature,
    ) -> Result<Self, Error> {
        let mut signature = *signature;
        signature.normalize_s()?;

        for recovery_id in 0..=1 {
            if let Ok(recoverable_signature) = Signature::new(&signature, Id(recovery_id)) {
                if let Ok(recovered_key) = recoverable_signature.recover_verify_key(msg) {
                    if public_key == &EncodedPoint::from(&recovered_key) {
                        return Ok(recoverable_signature);
                    }
                }
            }
        }

        Err(Error::new())
    }

    /// Recover the public key used to create the given signature as an
    /// [`EncodedPoint`].
    #[cfg(all(feature = "ecdsa", feature = "keccak256"))]
    #[cfg_attr(docsrs, doc(cfg(feature = "ecdsa")), doc(cfg(feature = "keccak256")))]
    pub fn recover_verify_key(&self, msg: &[u8]) -> Result<VerifyKey, Error> {
        self.recover_verify_key_from_digest(Keccak256::new().chain(msg))
    }

    /// Recover the public key used to create the given signature as an
    /// [`EncodedPoint`] from the provided precomputed [`Digest`].
    #[cfg(feature = "ecdsa")]
    #[cfg_attr(docsrs, doc(cfg(feature = "ecdsa")))]
    #[allow(non_snake_case, clippy::many_single_char_names)]
    pub fn recover_verify_key_from_digest<D>(&self, msg_prehash: D) -> Result<VerifyKey, Error>
    where
        D: Digest<OutputSize = U32>,
    {
        let r = self.r();
        let s = self.s();
        let z = Scalar::from_digest(msg_prehash);
        let R = AffinePoint::decompress(&r.to_bytes(), self.recovery_id().is_y_odd());

        // TODO(tarcieri): replace with into conversion when available (see subtle#73)
        if R.is_some().into() {
            let R = ProjectivePoint::from(R.unwrap());
            let r_inv = r.invert().unwrap();
            let u1 = -(r_inv * &z);
            let u2 = r_inv * &*s;
            let pk = ((&ProjectivePoint::generator() * &u1) + &(R * &u2)).to_affine();

            if pk.is_some().into() {
                return Ok(VerifyKey::from(&pk.unwrap()));
            }
        }

        Err(Error::new())
    }

    /// Parse the `r` component of this signature to a [`Scalar`]
    #[cfg(feature = "ecdsa")]
    #[cfg_attr(docsrs, doc(cfg(feature = "ecdsa")))]
    pub fn r(&self) -> NonZeroScalar {
        let r = NonZeroScalar::from_bytes(self.bytes[..32].try_into().unwrap());

        if r.is_some().into() {
            r.unwrap()
        } else {
            unreachable!("r-component ensured valid in constructor");
        }
    }

    /// Parse the `s` component of this signature to a [`Scalar`]
    #[cfg(feature = "ecdsa")]
    #[cfg_attr(docsrs, doc(cfg(feature = "ecdsa")))]
    pub fn s(&self) -> NonZeroScalar {
        let s = NonZeroScalar::from_bytes(self.bytes[32..64].try_into().unwrap());

        if s.is_some().into() {
            s.unwrap()
        } else {
            unreachable!("s-component ensured valid in constructor");
        }
    }
}

impl ecdsa_core::signature::Signature for Signature {
    fn from_bytes(bytes: &[u8]) -> Result<Self, Error> {
        bytes.try_into()
    }
}

impl AsRef<[u8]> for Signature {
    fn as_ref(&self) -> &[u8] {
        &self.bytes[..]
    }
}

impl Debug for Signature {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "RecoverableSignature {{ bytes: {:?}) }}", self.as_ref())
    }
}

// TODO(tarcieri): derive `Eq` after const generics are available
impl Eq for Signature {}

// TODO(tarcieri): derive `PartialEq` after const generics are available
impl PartialEq for Signature {
    fn eq(&self, other: &Self) -> bool {
        self.as_ref().eq(other.as_ref())
    }
}

impl TryFrom<&[u8]> for Signature {
    type Error = Error;

    fn try_from(bytes: &[u8]) -> Result<Self, Error> {
        if bytes.len() != SIZE {
            return Err(Error::new());
        }

        let signature = super::Signature::try_from(&bytes[..64])?;
        let recovery_id = Id::try_from(bytes[64])?;
        Self::new(&signature, recovery_id)
    }
}

impl From<Signature> for super::Signature {
    fn from(sig: Signature) -> Self {
        Self::from_bytes(&sig.bytes[..64]).unwrap()
    }
}

#[cfg(feature = "keccak256")]
impl ecdsa_core::signature::PrehashSignature for Signature {
    type Digest = Keccak256;
}

/// Identifier used to compute a [`EncodedPoint`] from a [`Signature`].
///
/// In practice these values are always either `0` or `1`, and indicate
/// whether or not the y-coordinate of the original [`EncodedPoint`] is odd.
///
/// While values `2` and `3` are also defined to capture whether `r`
/// overflowed the curve's order, this crate does *not* support them.
///
/// There is a vanishingly small chance of these values occurring outside
/// of contrived examples, so for simplicity's sake handling these values
/// is unsupported and will return an `Error` when parsing the `Id`.
#[derive(Copy, Clone, Debug)]
pub struct Id(pub(super) u8);

impl Id {
    /// Create a new [`Id`] from the given byte value
    pub fn new(byte: u8) -> Result<Self, Error> {
        match byte {
            0 | 1 => Ok(Self(byte)),
            _ => Err(Error::new()),
        }
    }

    /// Is `y` odd?
    #[cfg(feature = "ecdsa")]
    fn is_y_odd(self) -> Choice {
        self.0.into()
    }
}

impl TryFrom<u8> for Id {
    type Error = Error;

    fn try_from(byte: u8) -> Result<Self, Error> {
        Self::new(byte)
    }
}

impl From<Id> for u8 {
    fn from(recovery_id: Id) -> u8 {
        recovery_id.0
    }
}

#[cfg(all(test, feature = "ecdsa", feature = "sha256"))]
mod tests {
    use super::Signature;
    use crate::EncodedPoint;
    use core::convert::TryFrom;
    use hex_literal::hex;
    use sha2::{Digest, Sha256};

    /// Signature recovery test vectors
    struct TestVector {
        pk: [u8; 33],
        sig: [u8; 65],
        msg: &'static [u8],
    }

    const VECTORS: &[TestVector] = &[
        // Recovery ID 0
        TestVector {
            pk: hex!("021a7a569e91dbf60581509c7fc946d1003b60c7dee85299538db6353538d59574"),
            sig: hex!(
                "ce53abb3721bafc561408ce8ff99c909f7f0b18a2f788649d6470162ab1aa03239
                 71edc523a6d6453f3fb6128d318d9db1a5ff3386feb1047d9816e780039d5200"
            ),
            msg: b"example message",
        },
        // Recovery ID 1
        TestVector {
            pk: hex!("036d6caac248af96f6afa7f904f550253a0f3ef3f5aa2fe6838a95b216691468e2"),
            sig: hex!(
                "46c05b6368a44b8810d79859441d819b8e7cdc8bfd371e35c53196f4bcacdb5135
                 c7facce2a97b95eacba8a586d87b7958aaf8368ab29cee481f76e871dbd9cb01"
            ),
            msg: b"example message",
        },
    ];

    #[test]
    fn public_key_recovery() {
        for vector in VECTORS {
            let sig = Signature::try_from(&vector.sig[..]).unwrap();
            let prehash = Sha256::new().chain(vector.msg);
            let pk = sig.recover_verify_key_from_digest(prehash).unwrap();
            assert_eq!(&vector.pk[..], EncodedPoint::from(&pk).as_bytes());
        }
    }
}
