// Copyright (c) 2026 OptionLab LLC. All rights reserved.
// Synthetic test issuer only; included exclusively by test modules.
use aws_lc_rs::{
    rsa::{KeyPair as RsaKeyPair, KeySize},
    signature::KeyPair,
};
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
use pkcs1::der::Decode;

pub struct SyntheticRsa(RsaKeyPair);
impl SyntheticRsa {
    pub fn generate() -> Self {
        Self::generate_with_size(KeySize::Rsa2048)
    }
    pub fn generate_with_size(size: KeySize) -> Self {
        Self(RsaKeyPair::generate(size).expect("synthetic RSA fixture generation"))
    }
    pub fn public_components(&self) -> (String, String) {
        let public = pkcs1::RsaPublicKey::from_der(self.0.public_key().as_ref()).unwrap();
        (
            URL_SAFE_NO_PAD.encode(public.modulus.as_bytes()),
            URL_SAFE_NO_PAD.encode(public.public_exponent.as_bytes()),
        )
    }
    pub fn sign_rs256(&self, message: &str) -> Vec<u8> {
        let mut signature = vec![0; self.0.public_modulus_len()];
        // Backend hashes the original message; never pass a precomputed digest.
        self.0
            .sign(
                &aws_lc_rs::signature::RSA_PKCS1_SHA256,
                &aws_lc_rs::rand::SystemRandom::new(),
                message.as_bytes(),
                &mut signature,
            )
            .unwrap();
        signature
    }
}
#[test]
fn synthetic_fixture_signatures_verify_independently_and_reject_substitution() {
    let key = SyntheticRsa::generate();
    let (n, e) = key.public_components();
    let n = URL_SAFE_NO_PAD.decode(n).unwrap();
    let e = URL_SAFE_NO_PAD.decode(e).unwrap();
    assert_eq!(n.len(), 256);
    assert_eq!(n[0].leading_zeros(), 0);
    let public = ring::signature::RsaPublicKeyComponents { n: &n, e: &e };
    let message = "synthetic.header.synthetic.claims";
    let mut signed = key.sign_rs256(message);
    let algorithm = &ring::signature::RSA_PKCS1_2048_8192_SHA256;
    assert!(public
        .verify(algorithm, message.as_bytes(), &signed)
        .is_ok());
    assert!(public
        .verify(algorithm, b"changed claims", &signed)
        .is_err());
    signed[0] ^= 1;
    assert!(public
        .verify(algorithm, message.as_bytes(), &signed)
        .is_err());
    assert!(public
        .verify(algorithm, message.as_bytes(), &signed[..signed.len() - 1])
        .is_err());
}
