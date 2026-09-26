//! Runtime audio output discovery and switching.

use mineral_audio::{OutputDevice, OutputTarget};
use mineral_protocol::{Request, Response};

use super::Client;
use crate::operation::{Pending, SubmitError, decode_applied, decode_query};

impl Client {
    /// Queries devices on the daemon host without blocking the caller.
    pub fn audio_outputs(&self) -> Result<Pending<Vec<OutputDevice>>, SubmitError> {
        self.submit(Request::AudioOutputs, |result, name| {
            decode_query(result, name, |response| match response {
                Response::AudioOutputs(devices) => Some(devices),
                _ => None,
            })
        })
    }

    /// Selects an output route; playback subscriptions carry the confirmed device.
    ///
    /// # Params:
    ///   - `target`: System default or a CPAL device identifier returned by the daemon.
    pub fn set_audio_output(&self, target: OutputTarget) -> Result<Pending<()>, SubmitError> {
        self.submit(Request::SetAudioOutput(target), decode_applied)
    }
}
