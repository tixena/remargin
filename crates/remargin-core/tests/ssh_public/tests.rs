//! Generate, load, sign and verify using only `remargin_core`'s public surface.

use remargin_core::crypto::generate_keypair;
use remargin_core::crypto::ssh::{PrivateKey, PublicKey, SshSig};

#[test]
fn ssh_keys_are_usable_from_outside_the_crate() {
    let (private, public) = generate_keypair("test").unwrap();
    let key = PrivateKey::from_openssh(&private).unwrap();
    let signature = SshSig::from_pem(&key.sign("remargin", b"hello")).unwrap();
    let verifier = PublicKey::from_openssh(&public).unwrap();

    assert!(verifier.verify("remargin", b"hello", &signature).unwrap());
    assert!(!verifier.verify("remargin", b"other", &signature).unwrap());
}
