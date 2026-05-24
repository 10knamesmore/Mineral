use aes::Aes128;
use cipher::block_padding::Pkcs7;
use cipher::{BlockEncryptMut, KeyInit, KeyIvInit};

/// AES-128 CBC 加密器别名。
type Aes128CbcEnc = cbc::Encryptor<Aes128>;

/// AES-128 ECB 加密器别名。
type Aes128EcbEnc = ecb::Encryptor<Aes128>;

pub fn aes_cbc_pkcs7_encrypt(plaintext: &[u8], key: &[u8; 16], iv: &[u8; 16]) -> Vec<u8> {
    Aes128CbcEnc::new(key.into(), iv.into()).encrypt_padded_vec_mut::<Pkcs7>(plaintext)
}

pub fn aes_ecb_pkcs7_encrypt(plaintext: &[u8], key: &[u8; 16]) -> Vec<u8> {
    Aes128EcbEnc::new(key.into()).encrypt_padded_vec_mut::<Pkcs7>(plaintext)
}
