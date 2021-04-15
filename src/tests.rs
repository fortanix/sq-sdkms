use sequoia_openpgp::{
    crypto::{Decryptor, Signer},
    types::HashAlgorithm,
};

use crate::decryptor::PgpDecryptor;

use crate::signer::PgpSigner;

const API_ENDPOINT: &'static str = "https://sdkms.test.fortanix.com";
const MY_API_KEY: &'static str = "YjI1Y2M4NzUtZTNhOC00MmE5LTk1OWYtOGI0N2IyMDE2OWFmOnl4TThQWWdhclBWVzhQajRBZkVZcUNYM292TUVRVkRYbWh2d1V2OUxLeTB0UDY4eTFJZld2TlJFbmZTckxGdHIwZ25NVk9NMlhWTmZEalNSX3VzRVZB";

#[test]
fn get_signer_public_key() {
    let signer = PgpSigner::new(
        Some(API_ENDPOINT.to_string()),
        MY_API_KEY.to_string(),
        "Sobject Rsa").unwrap();
    signer.public();
}

#[test]
fn get_decryptor_public_key() {
    let decryptor = PgpDecryptor::new(
        Some(API_ENDPOINT.to_string()),
        MY_API_KEY.to_string(),
        "Sobject Rsa").unwrap();
    decryptor.public();
}

#[test]
fn get_armored_public_key() {
    let mut signer = PgpSigner::new(
        Some(API_ENDPOINT.to_string()),
        MY_API_KEY.to_string(),
        "Sobject Rsa").unwrap();
    let armored = signer.get_armored_key().unwrap();

    use std::io::{self, Write};

    let stdout = io::stdout();
    let mut handle = stdout.lock();

    handle.write_all(&armored).unwrap();
}

#[test]
fn sign() {
    let mut signer = PgpSigner::new(
        Some(API_ENDPOINT.to_string()),
        MY_API_KEY.to_string(),
        "Sobject Rsa").unwrap();
    signer.sign(HashAlgorithm::SHA1, &vec![0; 20]).unwrap();
}
