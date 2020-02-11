//! A mechanism to specify policy.
//!
//! A major goal of the Sequoia OpenPGP crate is to be policy free.
//! However, many mid-level operations build on low-level primitives.
//! For instance, finding a certificate's primary User ID means
//! examining each of its User IDs and their current self-signature.
//! Some algorithms are considered broken (e.g., MD5) and some are
//! considered weak (e.g. SHA-1).  When dealing with data from an
//! untrusted source, for instance, callers will often prefer to
//! ignore signatures that rely on these algorithms even though [RFC
//! 4880] says that "[i]mplementations MUST implement SHA-1."  When
//! trying to decrypt old archives, however, users probably don't want
//! to ignore keys using MD5, even though [RFC 4880] deprecates MD5.
//!
//! Rather than not provide this mid-level functionality, the `Policy`
//! trait allows callers to specify their prefer policy.  This can be
//! highly customized by providing a custom implementation of the
//! `Policy` trait, or it can be slightly refined by tweaking the
//! `StandardPolicy`'s parameters.
//!
//! When implementing the `Policy` trait, it is *essential* that the
//! functions are [idempotent].  That is, if the same `Policy` is used
//! to determine whether a given `Signature` is valid, it must always
//! return the same value.
//!
//! [RFC 4880]: https://tools.ietf.org/html/rfc4880#section-9.4
//! [pure]: https://en.wikipedia.org/wiki/Pure_function
use std::fmt;
use std::time::{SystemTime, Duration};
use std::u32;

use failure::ResultExt;

use crate::{
    packet::Signature,
    Result,
    types::HashAlgorithm,
    types::SignatureType,
    types::Timestamp,
};

#[macro_use] mod cutofflist;
use cutofflist::{
    CutoffList,
    REJECT,
    ACCEPT,
};

/// A policy for cryptographic operations.
pub trait Policy : fmt::Debug {
    /// Returns an error if the signature violates the policy.
    ///
    /// This function performs the last check before the library
    /// decides that a signature is valid.  That is, after the library
    /// has determined that the signature is well-formed, alive, not
    /// revoked, etc., it calls this function to allow you to
    /// implement any additional policy.  For instance, you may reject
    /// signatures that make use of cryptographically insecure
    /// algorithms like SHA-1.
    ///
    /// Note: Whereas it is generally better to reject suspicious
    /// signatures, one should be more liberal when considering
    /// revocations: if you reject a revocation certificate, it may
    /// inadvertently make something else valid!
    fn signature(&self, _sig: &Signature) -> Result<()> {
        Ok(())
    }
}

/// The standard policy.
///
/// The standard policy stores when each algorithm in a family of
/// algorithms is no longer considered safe.  Attempts to use an
/// algorithm after its cutoff time should fail.
///
/// When validating a signature, we normally want to know whether the
/// algorithms used are safe *now*.  That is, we don't use the
/// signature's alleged creation time when considering whether an
/// algorithm is safe, because if an algorithm is discovered to be
/// compromised at time X, then an attacker could forge a message
/// after time X with a signature creation time that is prior to X,
/// which would be incorrectly accepted.
///
/// Occasionally, we know that a signature has not been tampered with
/// since some time in the past.  We might know this if the signature
/// was stored on some tamper-proof medium.  In those cases, it is
/// reasonable to use the time that the signature was saved, since an
/// attacker could not have taken advantage of any weaknesses found
/// after that time.
#[derive(Debug, Clone)]
pub struct StandardPolicy {
    // The time.  If None, the current time is used.
    time: Option<Timestamp>,

    // Hash algorithms.
    hash_algos_normal: NormalHashCutoffList,
    hash_algos_revocation: RevocationHashCutoffList,

}

impl Default for StandardPolicy {
    fn default() -> Self {
        Self::new()
    }
}

impl<'a> From<&'a StandardPolicy> for Option<&'a dyn Policy> {
    fn from(p: &'a StandardPolicy) -> Self {
        Some(p as &dyn Policy)
    }
}

a_cutoff_list!(NormalHashCutoffList, HashAlgorithm, 12,
               [
                   REJECT,                 // 0. Not assigned.
                   Some(Timestamp::Y1997), // 1. MD5
                   Some(Timestamp::Y2013), // 2. SHA-1
                   Some(Timestamp::Y2013), // 3. RIPE-MD/160
                   REJECT,                 // 4. Reserved.
                   REJECT,                 // 5. Reserved.
                   REJECT,                 // 6. Reserved.
                   REJECT,                 // 7. Reserved.
                   ACCEPT,                 // 8. SHA256
                   ACCEPT,                 // 9. SHA384
                   ACCEPT,                 // 10. SHA512
                   ACCEPT,                 // 11. SHA224
               ]);
a_cutoff_list!(RevocationHashCutoffList, HashAlgorithm, 12,
               [
                   REJECT,                 // 0. Not assigned.
                   Some(Timestamp::Y2004), // 1. MD5
                   Some(Timestamp::Y2020), // 2. SHA-1
                   Some(Timestamp::Y2020), // 3. RIPE-MD/160
                   REJECT,                 // 4. Reserved.
                   REJECT,                 // 5. Reserved.
                   REJECT,                 // 6. Reserved.
                   REJECT,                 // 7. Reserved.
                   ACCEPT,                 // 8. SHA256
                   ACCEPT,                 // 9. SHA384
                   ACCEPT,                 // 10. SHA512
                   ACCEPT,                 // 11. SHA224
               ]);

// We need to convert a `SystemTime` to a `Timestamp` in
// `StandardPolicy::reject_hash_at`.  Unfortunately, a `SystemTime`
// can represent a larger range of time than a `Timestamp` can.  Since
// the times passed to this function are cutoff points, and we only
// compare them to OpenPGP timestamps, any `SystemTime` that is prior
// to the Unix Epoch is equivalent to the Unix Epoch: it will reject
// all timestamps.  Similarly, any `SystemTime` that is later than the
// latest time representable by a `Timestamp` is equivalent to
// accepting all time stamps, which is equivalent to passing None.
fn system_time_cutoff_to_timestamp(t: SystemTime) -> Option<Timestamp> {
    let t = t
        .duration_since(SystemTime::UNIX_EPOCH)
        // An error can only occur if the SystemTime is less than the
        // reference time (SystemTime::UNIX_EPOCH).  Map that to
        // SystemTime::UNIX_EPOCH, as above.
        .unwrap_or(Duration::new(0, 0));
    let t = t.as_secs();
    if t > u32::MAX as u64 {
        // Map to None, as above.
        None
    } else {
        Some((t as u32).into())
    }
}

impl StandardPolicy {
    /// Instantiates a new `StandardPolicy` with the default parameters.
    pub const fn new() -> Self {
        Self {
            time: None,
            hash_algos_normal: NormalHashCutoffList::Default(),
            hash_algos_revocation: RevocationHashCutoffList::Default(),
        }
    }

    /// Instantiates a new `StandardPolicy` with parameters
    /// appropriate for `time`.
    ///
    /// `time` is a meta-parameter that selects a security profile
    /// that is appropriate for the given point in time.  When
    /// evaluating an object, the reference time should be set to the
    /// time that the object was stored to non-tamperable storage.
    /// Since most applications don't record when they received an
    /// object, they should conservatively use the current time.
    ///
    /// Note that the reference time is a security parameter and is
    /// different from the time that the object was allegedly created.
    /// Consider evaluating a signature whose `Signature Creation
    /// Time` subpacket indicates that it was created in 2007.  Since
    /// the subpacket is under the control of the sender, setting the
    /// reference time according to the subpacket means that the
    /// sender chooses the security profile.  If the sender were an
    /// attacker, she could have forged this to take advantage of
    /// security weaknesses found since 2007.  This is why the
    /// reference time must be set---at the earliest---to the time
    /// that the message was stored to non-tamperable storage.  When
    /// that is not available, the current time should be used.
    pub fn at(time: SystemTime) -> Self {
        let mut p = Self::new();
        p.time = Some(system_time_cutoff_to_timestamp(time)
                          // Map "ACCEPT" to the end of time (None
                          // here means the current time).
                          .unwrap_or(Timestamp::MAX));
        p
    }

    /// Returns the policy's reference time.
    ///
    /// The current time is None.
    ///
    /// See `StandardPolicy::at` for details.
    pub fn time(&self) -> Option<SystemTime> {
        self.time.map(Into::into)
    }

    /// Always considers `h` to be secure.
    pub fn accept_hash(&mut self, h: HashAlgorithm) {
        self.hash_algos_normal.set(h, ACCEPT);
        self.hash_algos_revocation.set(h, ACCEPT);
    }

    /// Always considers `h` to be insecure.
    pub fn reject_hash(&mut self, h: HashAlgorithm) {
        self.hash_algos_normal.set(h, REJECT);
        self.hash_algos_revocation.set(h, REJECT);
    }

    /// Considers `h` to be insecure starting at `normal` for normal
    /// signatures and at `revocation` for revocation certificates.
    ///
    /// For each algorithm, there are two different cutoffs: when the
    /// algorithm is no longer safe for normal use (e.g., binding
    /// signatures, document signatures), and when the algorithm is no
    /// longer safe for revocations.  Normally, an algorithm should be
    /// allowed for use in a revocation longer than it should be
    /// allowed for normal use, because once we consider a revocation
    /// certificate to be invalid, it may cause something else to be
    /// considered valid!
    ///
    /// A cutoff of `None` means that there is no cutoff and the
    /// algorithm has no known vulnerabilities.
    ///
    /// As a rule of thumb, we want to stop accepting a Hash algorithm
    /// for normal signature when there is evidence that it is broken,
    /// and we want to stop accepting it for revocations shortly
    /// before collisions become practical.
    ///
    /// As such, we start rejecting [MD5] in 1997 and completely
    /// reject it starting in 2004:
    ///
    /// >  In 1996, Dobbertin announced a collision of the
    /// >  compression function of MD5 (Dobbertin, 1996). While this
    /// >  was not an attack on the full MD5 hash function, it was
    /// >  close enough for cryptographers to recommend switching to
    /// >  a replacement, such as SHA-1 or RIPEMD-160.
    /// >
    /// >  MD5CRK ended shortly after 17 August 2004, when collisions
    /// >  for the full MD5 were announced by Xiaoyun Wang, Dengguo
    /// >  Feng, Xuejia Lai, and Hongbo Yu. Their analytical attack
    /// >  was reported to take only one hour on an IBM p690 cluster.
    /// >
    /// > (Accessed Feb. 2020.)
    ///
    /// [MD5]: https://en.wikipedia.org/wiki/MD5
    ///
    /// And we start rejecting [SHA-1] in 2013 and completely reject
    /// it in 2020:
    ///
    /// > Since 2005 SHA-1 has not been considered secure against
    /// > well-funded opponents, as of 2010 many organizations have
    /// > recommended its replacement. NIST formally deprecated use
    /// > of SHA-1 in 2011 and disallowed its use for digital
    /// > signatures in 2013. As of 2020, attacks against SHA-1 are
    /// > as practical as against MD5; as such, it is recommended to
    /// > remove SHA-1 from products as soon as possible and use
    /// > instead SHA-256 or SHA-3. Replacing SHA-1 is urgent where
    /// > it's used for signatures.
    /// >
    /// > (Accessed Feb. 2020.)
    ///
    /// [SHA-1]: https://en.wikipedia.org/wiki/SHA-1
    ///
    /// Since RIPE-MD is structured similarly to SHA-1, we
    /// conservatively consider it to be broken as well.
    pub fn reject_hash_at<N, R>(&mut self, h: HashAlgorithm,
                                normal: N, revocation: R)
        where N: Into<Option<SystemTime>>,
              R: Into<Option<SystemTime>>,
    {
        self.hash_algos_normal.set(
            h,
            normal.into().and_then(system_time_cutoff_to_timestamp));
        self.hash_algos_revocation.set(
            h,
            revocation.into().and_then(system_time_cutoff_to_timestamp));
    }

    /// Returns the cutoff times for the specified hash algorithm.
    pub fn hash_cutoffs(&self, h: HashAlgorithm)
        -> (Option<SystemTime>, Option<SystemTime>)
    {
        (self.hash_algos_normal.cutoff(h).map(|t| t.into()),
         self.hash_algos_revocation.cutoff(h).map(|t| t.into()))
    }
}

impl Policy for StandardPolicy {
    fn signature(&self, sig: &Signature) -> Result<()> {
        let time = self.time.unwrap_or_else(Timestamp::now);

        match sig.typ() {
            t @ SignatureType::KeyRevocation
                | t @ SignatureType::SubkeyRevocation
                | t @ SignatureType::CertificationRevocation =>
            {
                self.hash_algos_revocation.check(sig.hash_algo(), time)
                    .context(format!("revocation signature ({})", t))?
            }
            t =>
            {
                self.hash_algos_normal.check(sig.hash_algo(), time)
                    .context(format!("non-revocation signature ({})", t))?
            }
        }

        Ok(())
    }
}

#[cfg(test)]
mod test {
    use std::io::Read;
    use std::time::Duration;

    use super::*;
    use crate::Fingerprint;
    use crate::cert::{Cert, CertBuilder};
    use crate::parse::Parse;
    use crate::policy::StandardPolicy as P;

    #[test]
    fn binding_signature() {
        let p = &P::new();

        // A primary and two subkeys.
        let (cert, _) = CertBuilder::new()
            .add_signing_subkey()
            .add_transport_encryption_subkey()
            .generate().unwrap();

        assert_eq!(cert.keys().with_policy(p, None).count(), 3);

        // Reject all direct key signatures.
        #[derive(Debug)]
        struct NoDirectKeySigs;
        impl Policy for NoDirectKeySigs {
            fn signature(&self, sig: &Signature) -> Result<()> {
                use crate::types::SignatureType::*;

                match sig.typ() {
                    DirectKey => Err(format_err!("direct key!")),
                    _ => Ok(()),
                }
            }
        }

        let p = &NoDirectKeySigs {};
        assert_eq!(cert.keys().with_policy(p, None).count(), 0);

        // Reject all subkey signatures.
        #[derive(Debug)]
        struct NoSubkeySigs;
        impl Policy for NoSubkeySigs {
            fn signature(&self, sig: &Signature) -> Result<()> {
                use crate::types::SignatureType::*;

                match sig.typ() {
                    SubkeyBinding => Err(format_err!("subkey signature!")),
                    _ => Ok(()),
                }
            }
        }

        let p = &NoSubkeySigs {};
        assert_eq!(cert.keys().with_policy(p, None).count(), 1);
    }

    #[test]
    fn revocation() -> Result<()> {
        use crate::cert::UserIDRevocationBuilder;
        use crate::cert::SubkeyRevocationBuilder;
        use crate::types::SignatureType;
        use crate::types::ReasonForRevocation;

        let p = &P::new();

        // A primary and two subkeys.
        let (cert, _) = CertBuilder::new()
            .add_userid("Alice")
            .add_signing_subkey()
            .add_transport_encryption_subkey()
            .generate()?;

        // Make sure we have all keys and all user ids.
        assert_eq!(cert.keys().with_policy(p, None).count(), 3);
        assert_eq!(cert.userids().with_policy(p, None).count(), 1);

        // Reject all user id signatures.
        #[derive(Debug)]
        struct NoPositiveCertifications;
        impl Policy for NoPositiveCertifications {
            fn signature(&self, sig: &Signature) -> Result<()> {
                use crate::types::SignatureType::*;
                match sig.typ() {
                    PositiveCertification =>
                        Err(format_err!("positive certification!")),
                    _ => Ok(()),
                }
            }
        }
        let p = &NoPositiveCertifications {};
        assert_eq!(cert.userids().with_policy(p, None).count(), 0);


        // Revoke it.
        let mut keypair = cert.primary_key().key().clone()
            .mark_parts_secret()?.into_keypair()?;
        let ca = cert.userids().nth(0).unwrap();

        // Generate the revocation for the first and only UserID.
        let revocation =
            UserIDRevocationBuilder::new()
            .set_reason_for_revocation(
                ReasonForRevocation::KeyRetired,
                b"Left example.org.")?
            .build(&mut keypair, &cert, ca.userid(), None)?;
        assert_eq!(revocation.typ(), SignatureType::CertificationRevocation);

        // Now merge the revocation signature into the Cert.
        let cert = cert.merge_packets(vec![revocation.clone().into()])?;

        // Check that it is revoked.
        assert_eq!(cert.userids().with_policy(p, None).revoked(false).count(), 0);

        // Reject all user id signatures.
        #[derive(Debug)]
        struct NoCertificationRevocation;
        impl Policy for NoCertificationRevocation {
            fn signature(&self, sig: &Signature) -> Result<()> {
                use crate::types::SignatureType::*;
                match sig.typ() {
                    CertificationRevocation =>
                        Err(format_err!("certification certification!")),
                    _ => Ok(()),
                }
            }
        }
        let p = &NoCertificationRevocation {};

        // Check that the user id is no longer revoked.
        assert_eq!(cert.userids().with_policy(p, None).revoked(false).count(), 1);


        // Generate the revocation for the first subkey.
        let subkey = cert.keys().subkeys().nth(0).unwrap();
        let revocation =
            SubkeyRevocationBuilder::new()
                .set_reason_for_revocation(
                    ReasonForRevocation::KeyRetired,
                    b"Smells funny.").unwrap()
                .build(&mut keypair, &cert, subkey.key(), None)?;
        assert_eq!(revocation.typ(), SignatureType::SubkeyRevocation);

        // Now merge the revocation signature into the Cert.
        assert_eq!(cert.keys().with_policy(p, None).revoked(false).count(), 3);
        let cert = cert.merge_packets(vec![revocation.clone().into()])?;
        assert_eq!(cert.keys().with_policy(p, None).revoked(false).count(), 2);

        // Reject all subkey revocations.
        #[derive(Debug)]
        struct NoSubkeyRevocation;
        impl Policy for NoSubkeyRevocation {
            fn signature(&self, sig: &Signature) -> Result<()> {
                use crate::types::SignatureType::*;
                match sig.typ() {
                    SubkeyRevocation =>
                        Err(format_err!("subkey revocation!")),
                    _ => Ok(()),
                }
            }
        }
        let p = &NoSubkeyRevocation {};

        // Check that the key is no longer revoked.
        assert_eq!(cert.keys().with_policy(p, None).revoked(false).count(), 3);

        Ok(())
    }


    #[test]
    fn binary_signature() {
        use crate::crypto::SessionKey;
        use crate::types::SymmetricAlgorithm;
        use crate::packet::{PKESK, SKESK};
        use crate::parse::stream::MessageLayer;
        use crate::parse::stream::MessageStructure;
        use crate::parse::stream::Verifier;
        use crate::parse::stream::Decryptor;
        use crate::parse::stream::VerificationHelper;
        use crate::parse::stream::DecryptionHelper;

        #[derive(PartialEq, Debug)]
        struct VHelper {
            good: usize,
            errors: usize,
            keys: Vec<Cert>,
        }

        impl VHelper {
            fn new(keys: Vec<Cert>) -> Self {
                VHelper {
                    good: 0,
                    errors: 0,
                    keys: keys,
                }
            }
        }

        impl VerificationHelper for VHelper {
            fn get_public_keys(&mut self, _ids: &[crate::KeyHandle])
                -> Result<Vec<Cert>>
            {
                Ok(self.keys.clone())
            }

            fn check(&mut self, structure: MessageStructure) -> Result<()>
            {
                use crate::parse::stream::VerificationResult::*;
                for layer in structure.iter() {
                    match layer {
                        MessageLayer::SignatureGroup { ref results } =>
                            for result in results {
                                eprintln!("result: {:?}", result);
                                match result {
                                    GoodChecksum { .. } => self.good += 1,
                                    Error { .. } => self.errors += 1,
                                    _ => (),
                                }
                            }
                        MessageLayer::Compression { .. } => (),
                        _ => unreachable!(),
                    }
                }

                Ok(())
            }
        }

        impl DecryptionHelper for VHelper {
            fn decrypt<D>(&mut self, _: &[PKESK], _: &[SKESK], _: D)
                          -> Result<Option<Fingerprint>>
                where D: FnMut(SymmetricAlgorithm, &SessionKey) -> Result<()>
            {
                unreachable!();
            }
        }

        // Reject all data (binary) signatures.
        #[derive(Debug)]
        struct NoBinarySigantures;
        impl Policy for NoBinarySigantures {
            fn signature(&self, sig: &Signature) -> Result<()> {
                use crate::types::SignatureType::*;
                eprintln!("{:?}", sig.typ());
                match sig.typ() {
                    Binary =>
                        Err(format_err!("binary!")),
                    _ => Ok(()),
                }
            }
        }
        let no_binary_signatures = &NoBinarySigantures {};

        // Reject all subkey signatures.
        #[derive(Debug)]
        struct NoSubkeySigs;
        impl Policy for NoSubkeySigs {
            fn signature(&self, sig: &Signature) -> Result<()> {
                use crate::types::SignatureType::*;

                match sig.typ() {
                    SubkeyBinding => Err(format_err!("subkey signature!")),
                    _ => Ok(()),
                }
            }
        }
        let no_subkey_signatures = &NoSubkeySigs {};

        let standard = &P::new();

        let keys = [
            "neal.pgp",
        ].iter()
            .map(|f| Cert::from_bytes(crate::tests::key(f)).unwrap())
            .collect::<Vec<_>>();
        let data = "messages/signed-1.gpg";

        let reference = crate::tests::manifesto();



        // Test Verifier.

        // Standard policy => ok.
        let h = VHelper::new(keys.clone());
        let mut v =
            match Verifier::from_bytes(standard, crate::tests::file(data), h,
                                       crate::frozen_time()) {
                Ok(v) => v,
                Err(e) => panic!("{}", e),
            };
        assert!(v.message_processed());
        assert_eq!(v.helper_ref().good, 1);
        assert_eq!(v.helper_ref().errors, 0);

        let mut content = Vec::new();
        v.read_to_end(&mut content).unwrap();
        assert_eq!(reference.len(), content.len());
        assert_eq!(reference, &content[..]);


        // Kill the subkey.
        let h = VHelper::new(keys.clone());
        let mut v = match Verifier::from_bytes(no_subkey_signatures,
                                   crate::tests::file(data), h,
                                   crate::frozen_time()) {
            Ok(v) => v,
            Err(e) => panic!("{}", e),
        };
        assert!(v.message_processed());
        assert_eq!(v.helper_ref().good, 0);
        assert_eq!(v.helper_ref().errors, 1);

        let mut content = Vec::new();
        v.read_to_end(&mut content).unwrap();
        assert_eq!(reference.len(), content.len());
        assert_eq!(reference, &content[..]);


        // Kill the data signature.
        let h = VHelper::new(keys.clone());
        let mut v =
            match Verifier::from_bytes(no_binary_signatures,
                                       crate::tests::file(data), h,
                                       crate::frozen_time()) {
                Ok(v) => v,
                Err(e) => panic!("{}", e),
            };
        assert!(v.message_processed());
        assert_eq!(v.helper_ref().good, 0);
        assert_eq!(v.helper_ref().errors, 1);

        let mut content = Vec::new();
        v.read_to_end(&mut content).unwrap();
        assert_eq!(reference.len(), content.len());
        assert_eq!(reference, &content[..]);



        // Test Decryptor.

        // Standard policy.
        let h = VHelper::new(keys.clone());
        let mut v =
            match Decryptor::from_bytes(standard, crate::tests::file(data), h,
                                        crate::frozen_time()) {
                Ok(v) => v,
                Err(e) => panic!("{}", e),
            };
        assert!(v.message_processed());
        assert_eq!(v.helper_ref().good, 1);
        assert_eq!(v.helper_ref().errors, 0);

        let mut content = Vec::new();
        v.read_to_end(&mut content).unwrap();
        assert_eq!(reference.len(), content.len());
        assert_eq!(reference, &content[..]);


        // Kill the subkey.
        let h = VHelper::new(keys.clone());
        let mut v = match Decryptor::from_bytes(no_subkey_signatures,
                                                crate::tests::file(data), h,
                                                crate::frozen_time()) {
            Ok(v) => v,
            Err(e) => panic!("{}", e),
        };
        assert!(v.message_processed());
        assert_eq!(v.helper_ref().good, 0);
        assert_eq!(v.helper_ref().errors, 1);

        let mut content = Vec::new();
        v.read_to_end(&mut content).unwrap();
        assert_eq!(reference.len(), content.len());
        assert_eq!(reference, &content[..]);


        // Kill the data signature.
        let h = VHelper::new(keys.clone());
        let mut v =
            match Decryptor::from_bytes(no_binary_signatures,
                                        crate::tests::file(data), h,
                                        crate::frozen_time()) {
                Ok(v) => v,
                Err(e) => panic!("{}", e),
            };
        assert!(v.message_processed());
        assert_eq!(v.helper_ref().good, 0);
        assert_eq!(v.helper_ref().errors, 1);

        let mut content = Vec::new();
        v.read_to_end(&mut content).unwrap();
        assert_eq!(reference.len(), content.len());
        assert_eq!(reference, &content[..]);
    }

    #[test]
    fn hash_algo() -> Result<()> {
        use crate::RevocationStatus;
        use crate::types::ReasonForRevocation;

        const SECS_IN_YEAR : u64 = 365 * 24 * 60 * 60;

        // A `const fn` is only guaranteed to be evaluated at compile
        // time if the result is assigned to a `const` variable.  Make
        // sure that works.
        const DEFAULT : StandardPolicy = StandardPolicy::new();

        let (cert, _) = CertBuilder::new()
            .add_userid("Alice")
            .generate()?;

        let algo = cert.primary_key().bundle()
            .binding_signature(&DEFAULT, None).unwrap().hash_algo();

        eprintln!("{:?}", algo);

        // Create a revoked version.
        let mut keypair = cert.primary_key().key().clone()
            .mark_parts_secret()?.into_keypair()?;
        let cert_revoked = cert.clone().revoke_in_place(
            &mut keypair,
            ReasonForRevocation::KeyCompromised,
            b"It was the maid :/")?;

        match cert_revoked.revoked(&DEFAULT, None) {
            RevocationStatus::Revoked(sigs) => {
                assert_eq!(sigs.len(), 1);
                assert_eq!(sigs[0].hash_algo(), algo);
            }
            _ => panic!("not revoked"),
        }


        // Reject the hash algorithm unconditionally.
        let mut reject : StandardPolicy = StandardPolicy::new();
        reject.reject_hash(algo);
        assert!(cert.primary_key().bundle()
                    .binding_signature(&reject, None).is_none());
        assert_match!(RevocationStatus::NotAsFarAsWeKnow
                      = cert_revoked.revoked(&reject, None));

        // Reject the hash algorith next year.
        let mut reject : StandardPolicy = StandardPolicy::new();
        reject.reject_hash_at(
            algo,
            SystemTime::now() + Duration::from_secs(SECS_IN_YEAR),
            SystemTime::now() + Duration::from_secs(SECS_IN_YEAR));
        assert!(cert.primary_key().bundle()
                    .binding_signature(&reject, None).is_some());
        assert_match!(RevocationStatus::Revoked(_)
                      = cert_revoked.revoked(&reject, None));

        // Reject the hash algorith last year.
        let mut reject : StandardPolicy = StandardPolicy::new();
        reject.reject_hash_at(
            algo,
            SystemTime::now() - Duration::from_secs(SECS_IN_YEAR),
            SystemTime::now() - Duration::from_secs(SECS_IN_YEAR));
        assert!(cert.primary_key().bundle()
                    .binding_signature(&reject, None).is_none());
        assert_match!(RevocationStatus::NotAsFarAsWeKnow
                      = cert_revoked.revoked(&reject, None));

        // Reject the hash algorithm for normal signatures last year,
        // and revocations next year.
        let mut reject : StandardPolicy = StandardPolicy::new();
        reject.reject_hash_at(
            algo,
            SystemTime::now() - Duration::from_secs(SECS_IN_YEAR),
            SystemTime::now() + Duration::from_secs(SECS_IN_YEAR));
        assert!(cert.primary_key().bundle()
                    .binding_signature(&reject, None).is_none());
        assert_match!(RevocationStatus::Revoked(_)
                      = cert_revoked.revoked(&reject, None));

        // Accept algo, but reject the algos with id - 1 and id + 1.
        let mut reject : StandardPolicy = StandardPolicy::new();
        let algo_u8 : u8 = algo.into();
        assert!(algo_u8 != 0u8);
        reject.reject_hash_at(
            (algo_u8 - 1).into(),
            SystemTime::now() - Duration::from_secs(SECS_IN_YEAR),
            SystemTime::now() - Duration::from_secs(SECS_IN_YEAR));
        reject.reject_hash_at(
            (algo_u8 + 1).into(),
            SystemTime::now() - Duration::from_secs(SECS_IN_YEAR),
            SystemTime::now() - Duration::from_secs(SECS_IN_YEAR));
        assert!(cert.primary_key().bundle()
                    .binding_signature(&reject, None).is_some());
        assert_match!(RevocationStatus::Revoked(_)
                      = cert_revoked.revoked(&reject, None));

        // Reject the hash algorithm since before the Unix epoch.
        // Since the earliest representable time using a Timestamp is
        // the Unix epoch, this is equivalent to rejecting everything.
        let mut reject : StandardPolicy = StandardPolicy::new();
        reject.reject_hash_at(
            algo,
            SystemTime::UNIX_EPOCH - Duration::from_secs(SECS_IN_YEAR),
            SystemTime::UNIX_EPOCH - Duration::from_secs(SECS_IN_YEAR));
        assert!(cert.primary_key().bundle()
                    .binding_signature(&reject, None).is_none());
        assert_match!(RevocationStatus::NotAsFarAsWeKnow
                      = cert_revoked.revoked(&reject, None));

        // Reject the hash algorithm after the end of time that is
        // representable by a Timestamp (2106).  This should accept
        // everything.
        let mut reject : StandardPolicy = StandardPolicy::new();
        reject.reject_hash_at(
            algo,
            SystemTime::UNIX_EPOCH + Duration::from_secs(500 * SECS_IN_YEAR),
            SystemTime::UNIX_EPOCH + Duration::from_secs(500 * SECS_IN_YEAR));
        assert!(cert.primary_key().bundle()
                    .binding_signature(&reject, None).is_some());
        assert_match!(RevocationStatus::Revoked(_)
                      = cert_revoked.revoked(&reject, None));

        Ok(())
    }
}