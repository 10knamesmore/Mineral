//! Owns the cryptographic state between a streaming challenge and its response.

use std::io::{Cursor, Error as IoError, Read};
use std::num::TryFromIntError;
use std::sync::Arc;
use std::time::{SystemTime, SystemTimeError, UNIX_EPOCH};

use aes::Aes128;
use cbc::Decryptor;
use cbc::cipher::{BlockDecryptMut, KeyIvInit, block_padding::Pkcs7};
use cmac::{Cmac, Mac};
use hmac::Hmac;
use prost::{DecodeError, Message};
use rand::RngCore;
use rand::rngs::OsRng;
use rsa::pkcs1::{DecodeRsaPrivateKey, Error as RsaKeyError};
use rsa::pss::BlindedSigningKey;
use rsa::signature::{RandomizedSigner, SignatureEncoding};
use rsa::{Error as RsaError, Oaep, RsaPrivateKey};
use sha1::Sha1;
use sha2::Sha256;
use thiserror::Error;
use zeroize::Zeroizing;

use super::wire::{CencRequest, Content, Envelope, KeyInitialization, License, StreamingRequest};

#[derive(Debug, Error)]
pub(crate) enum Error {
    #[error("invalid device identity container")]
    DeviceFormat,

    #[error("unsupported device container version {0}")]
    DeviceVersion(u8),

    #[error("unsupported device platform {0}")]
    DevicePlatform(u8),

    #[error("read device identity")]
    DeviceRead(#[from] IoError),

    #[error("parse RSA device key")]
    DeviceKey(#[from] RsaKeyError),

    #[error("protobuf response is invalid")]
    Protocol(#[from] DecodeError),

    #[error("Widevine initialization must identify one 16-byte content key")]
    InitializationKeyId,

    #[error("missing license field: {0}")]
    Missing(&'static str),

    #[error("unsupported license message type")]
    MessageType,

    #[error("unsupported session key type")]
    SessionKeyType,

    #[error("unwrap license session key")]
    SessionKey(#[from] RsaError),

    #[error("license session key has the wrong length")]
    SessionKeyLength,

    #[error("license authentication failed")]
    Authentication,

    #[error("content key IV has the wrong length")]
    ContentIv,

    #[error("content key decryption failed")]
    ContentKey,

    #[error("license contains no content keys")]
    NoContentKeys,

    #[error("system clock precedes Unix epoch")]
    Clock(#[from] SystemTimeError),

    #[error("timestamp exceeds protocol range")]
    Timestamp(#[from] TryFromIntError),
}

type Result<T> = std::result::Result<T, Error>;

/// Device credentials decoded from a WVD container and used to sign a license challenge.
pub(crate) struct DeviceIdentity {
    private_key: RsaPrivateKey,

    client_identity: Vec<u8>,
}

impl DeviceIdentity {
    pub fn from_wvd(bytes: &[u8]) -> Result<Self> {
        let mut reader = Cursor::new(bytes);
        let mut header = [0; 7];
        reader.read_exact(&mut header)?;
        let [
            first,
            second,
            third,
            version,
            platform,
            _security_level,
            _flags,
        ] = header;
        if [first, second, third] != *b"WVD" {
            return Err(Error::DeviceFormat);
        }
        if version != 2 {
            return Err(Error::DeviceVersion(version));
        }
        if platform != 2 {
            return Err(Error::DevicePlatform(platform));
        }
        let private_key = Zeroizing::new(read_sized(&mut reader)?);
        let client_identity = read_sized(&mut reader)?;
        if reader.position() != u64::try_from(bytes.len()).map_err(|_length| Error::DeviceFormat)? {
            return Err(Error::DeviceFormat);
        }
        let private_key = RsaPrivateKey::from_pkcs1_der(&private_key)?;
        Ok(Self {
            private_key,
            client_identity,
        })
    }
}

/// A license's content key and its KID; secret bytes are zeroed when released.
/// The playback opener selects the matching KID and validates the AES-128 key length.
pub(crate) struct ContentKey {
    pub id: Vec<u8>,

    pub bytes: Zeroizing<Vec<u8>>,
}

/// Retains the signed request bytes and device key until its license response arrives.
/// Response authentication and key derivation use these exact request bytes;
/// finishing consumes the exchange so they cannot be paired with another response.
pub(crate) struct LicenseExchange {
    device: Arc<DeviceIdentity>,

    request_payload: Vec<u8>,

    challenge: Vec<u8>,
}

impl LicenseExchange {
    pub fn streaming(device: Arc<DeviceIdentity>, initialization: Vec<u8>) -> Result<Self> {
        let requested_at = SystemTime::now().duration_since(UNIX_EPOCH)?.as_secs();
        let mut random = OsRng;
        let mut request_id = [0; 16];
        random.fill_bytes(&mut request_id);
        let request_id = request_id
            .iter()
            .map(|byte| format!("{byte:02X}"))
            .collect::<String>()
            .into_bytes();
        let request = StreamingRequest {
            client_identity: Some(device.client_identity.clone()),
            content: Some(Content {
                cenc: Some(CencRequest {
                    initialization: vec![initialization],
                    license_type: Some(1),
                    request_id: Some(request_id),
                }),
            }),
            request_type: Some(1),
            requested_at: Some(i64::try_from(requested_at)?),
            protocol_version: Some(21),
            nonce: Some(random.next_u32()),
        };
        let request_payload = request.encode_to_vec();
        let signing_key = BlindedSigningKey::<Sha1>::new(device.private_key.clone());
        let signature = signing_key
            .sign_with_rng(&mut random, &request_payload)
            .to_vec();
        let challenge = Envelope {
            message_type: Some(1),
            message: Some(request_payload.clone()),
            signature: Some(signature),
            wrapped_session_key: None,
            session_key_type: None,
            authenticated_prefix: None,
        }
        .encode_to_vec();
        Ok(Self {
            device,
            request_payload,
            challenge,
        })
    }

    pub fn challenge(&self) -> &[u8] {
        &self.challenge
    }

    pub fn finish(self, response: &[u8]) -> Result<Vec<ContentKey>> {
        let envelope = Envelope::decode(response)?;
        if envelope.message_type != Some(2) {
            return Err(Error::MessageType);
        }
        if envelope.session_key_type.is_some_and(|kind| kind != 1) {
            return Err(Error::SessionKeyType);
        }
        let wrapped = envelope
            .wrapped_session_key
            .ok_or(Error::Missing("session key"))?;
        let session_key = Zeroizing::new(
            self.device
                .private_key
                .decrypt(Oaep::new::<Sha1>(), &wrapped)?,
        );
        let session_key: &[u8; 16] = session_key
            .as_slice()
            .try_into()
            .map_err(|_length| Error::SessionKeyLength)?;
        let encryption_key = Zeroizing::new(derive_key(
            session_key,
            b"ENCRYPTION",
            &self.request_payload,
            128,
            1,
        ));
        let mut authentication_key = Zeroizing::new(Vec::with_capacity(32));
        authentication_key.extend_from_slice(&derive_key(
            session_key,
            b"AUTHENTICATION",
            &self.request_payload,
            512,
            1,
        ));
        authentication_key.extend_from_slice(&derive_key(
            session_key,
            b"AUTHENTICATION",
            &self.request_payload,
            512,
            2,
        ));
        let message = envelope.message.ok_or(Error::Missing("license payload"))?;
        let signature = envelope
            .signature
            .ok_or(Error::Missing("license authentication"))?;
        let mut mac = <Hmac<Sha256> as Mac>::new_from_slice(&authentication_key)
            .map_err(|_length| Error::Authentication)?;
        if let Some(prefix) = envelope.authenticated_prefix {
            mac.update(&prefix);
        }
        mac.update(&message);
        mac.verify_slice(&signature)
            .map_err(|_signature| Error::Authentication)?;
        let license = License::decode(message.as_slice())?;
        let mut keys = Vec::new();
        for key in license.keys {
            if key.purpose != Some(2) {
                continue;
            }
            let id = key.id.ok_or(Error::Missing("content key identity"))?;
            let iv = key.iv.ok_or(Error::Missing("content key IV"))?;
            let iv: &[u8; 16] = iv
                .as_slice()
                .try_into()
                .map_err(|_length| Error::ContentIv)?;
            let encrypted = key
                .encrypted_key
                .ok_or(Error::Missing("encrypted content key"))?;
            let bytes = Decryptor::<Aes128>::new((&*encryption_key).into(), iv.into())
                .decrypt_padded_vec_mut::<Pkcs7>(&encrypted)
                .map_err(|_padding| Error::ContentKey)?;
            keys.push(ContentKey {
                id,
                bytes: Zeroizing::new(bytes),
            });
        }
        if keys.is_empty() {
            return Err(Error::NoContentKeys);
        }
        Ok(keys)
    }
}

pub(crate) fn key_initialization(key_id: &[u8]) -> Vec<u8> {
    KeyInitialization {
        algorithm: Some(1),
        key_ids: vec![key_id.to_vec()],
    }
    .encode_to_vec()
}

pub(crate) fn initialization_key_id(initialization: &[u8]) -> Result<[u8; 16]> {
    let initialization = KeyInitialization::decode(initialization)?;
    let [key_id] = initialization.key_ids.as_slice() else {
        return Err(Error::InitializationKeyId);
    };
    key_id
        .as_slice()
        .try_into()
        .map_err(|_wrong_length| Error::InitializationKeyId)
}

fn read_sized(reader: &mut Cursor<&[u8]>) -> Result<Vec<u8>> {
    let mut length = [0; 2];
    reader.read_exact(&mut length)?;
    let mut bytes = vec![0; usize::from(u16::from_be_bytes(length))];
    reader.read_exact(&mut bytes)?;
    Ok(bytes)
}

// NIST SP 800-108 CMAC counter-mode derivation; the protocol fixes a one-byte counter.
fn derive_key(key: &[u8; 16], label: &[u8], context: &[u8], bits: u32, counter: u8) -> [u8; 16] {
    let mut mac = <Cmac<Aes128> as Mac>::new(key.into());
    mac.update(&[counter]);
    mac.update(label);
    mac.update(&[0]);
    mac.update(context);
    mac.update(&bits.to_be_bytes());
    mac.finalize().into_bytes().into()
}
