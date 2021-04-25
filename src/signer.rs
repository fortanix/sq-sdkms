use sequoia_openpgp::{
    crypto::{mpi, Signer},
    packet::{
        key::{PublicParts, UnspecifiedRole},
        Key,
    },
    types::HashAlgorithm,
    Result as SequoiaResult,
};

use sdkms::{
    api_model::{DigestAlgorithm, SignRequest, SobjectDescriptor},
    SdkmsClient,
};

use super::SequoiaKey;

pub(crate) struct RawSigner {
    pub(crate) api_endpoint: String,
    pub(crate) api_key: String,
    pub(crate) sequoia_key: SequoiaKey,
}

impl Signer for RawSigner {
    fn public(&self) -> &Key<PublicParts, UnspecifiedRole> {
        &self.sequoia_key.public_key
    }

    fn sign(&mut self, hash_algo: HashAlgorithm, digest: &[u8]) -> SequoiaResult<mpi::Signature> {
        let http_client = SdkmsClient::builder()
            .with_api_endpoint(&self.api_endpoint)
            .with_api_key(&self.api_key)
            .build()?;

        let signature = {
            let hash_alg = match hash_algo {
                HashAlgorithm::SHA1 => DigestAlgorithm::Sha1,
                HashAlgorithm::SHA512 => DigestAlgorithm::Sha512,
                HashAlgorithm::SHA256 => DigestAlgorithm::Sha256,
                _ => {
                    panic!("unimplemented hash algorithm");
                }
            };

            let sign_req = SignRequest {
                key: Some(SobjectDescriptor::Kid(self.sequoia_key.kid)),
                hash_alg,
                hash: Some(digest.to_vec().into()),
                data: None,
                mode: None,
                deterministic_signature: None,
            };

            let sign_resp = http_client.sign(&sign_req)?;
            let plain: Vec<u8> = sign_resp.signature.into();
            mpi::Signature::RSA { s: plain.into() }
        };

        Ok(signature)
    }
}
