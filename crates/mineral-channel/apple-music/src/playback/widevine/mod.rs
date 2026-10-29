//! Widevine device identity, challenge construction and license key extraction.
//! The caller transports challenge and response bytes; no account or HTTP state lives here.

mod exchange;
mod wire;

pub(crate) use exchange::{
    ContentKey, DeviceIdentity, Error, LicenseExchange, LicenseInitialization,
};
