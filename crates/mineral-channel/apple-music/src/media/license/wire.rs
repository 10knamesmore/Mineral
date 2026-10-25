use prost::Message;

// Only fields needed by the streaming exchange are modeled. Nested client identity
// is preserved as opaque protobuf bytes supplied by the device owner.
#[derive(Clone, PartialEq, Message)]
pub(super) struct Envelope {
    #[prost(int32, optional, tag = "1")]
    pub message_type: Option<i32>,

    #[prost(bytes = "vec", optional, tag = "2")]
    pub message: Option<Vec<u8>>,

    #[prost(bytes = "vec", optional, tag = "3")]
    pub signature: Option<Vec<u8>>,

    #[prost(bytes = "vec", optional, tag = "4")]
    pub wrapped_session_key: Option<Vec<u8>>,

    #[prost(int32, optional, tag = "8")]
    pub session_key_type: Option<i32>,

    #[prost(bytes = "vec", optional, tag = "9")]
    pub authenticated_prefix: Option<Vec<u8>>,
}

#[derive(Clone, PartialEq, Message)]
pub(super) struct StreamingRequest {
    #[prost(bytes = "vec", optional, tag = "1")]
    pub client_identity: Option<Vec<u8>>,

    #[prost(message, optional, tag = "2")]
    pub content: Option<Content>,

    #[prost(int32, optional, tag = "3")]
    pub request_type: Option<i32>,

    #[prost(int64, optional, tag = "4")]
    pub requested_at: Option<i64>,

    #[prost(int32, optional, tag = "6")]
    pub protocol_version: Option<i32>,

    #[prost(uint32, optional, tag = "7")]
    pub nonce: Option<u32>,
}

#[derive(Clone, PartialEq, Message)]
pub(super) struct Content {
    #[prost(message, optional, tag = "1")]
    pub cenc: Option<CencRequest>,
}

#[derive(Clone, PartialEq, Message)]
pub(super) struct CencRequest {
    #[prost(bytes = "vec", repeated, tag = "1")]
    pub initialization: Vec<Vec<u8>>,

    #[prost(int32, optional, tag = "2")]
    pub license_type: Option<i32>,

    #[prost(bytes = "vec", optional, tag = "3")]
    pub request_id: Option<Vec<u8>>,
}

#[derive(Clone, PartialEq, Message)]
pub(super) struct KeyInitialization {
    #[prost(int32, optional, tag = "1")]
    pub algorithm: Option<i32>,

    #[prost(bytes = "vec", repeated, tag = "2")]
    pub key_ids: Vec<Vec<u8>>,
}

#[derive(Clone, PartialEq, Message)]
pub(super) struct License {
    #[prost(message, repeated, tag = "3")]
    pub keys: Vec<LicenseKey>,
}

#[derive(Clone, PartialEq, Message)]
pub(super) struct LicenseKey {
    #[prost(bytes = "vec", optional, tag = "1")]
    pub id: Option<Vec<u8>>,

    #[prost(bytes = "vec", optional, tag = "2")]
    pub iv: Option<Vec<u8>>,

    #[prost(bytes = "vec", optional, tag = "3")]
    pub encrypted_key: Option<Vec<u8>>,

    #[prost(int32, optional, tag = "4")]
    pub purpose: Option<i32>,
}
