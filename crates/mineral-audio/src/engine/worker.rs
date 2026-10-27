//! Each playback instance owns a cancellable decoder worker. Only prepared PCM crosses
//! into the shared render buffer; network reads and resampling never run in CPAL.

use std::sync::Arc;
use std::thread;
use std::time::Duration;

use mineral_common::revision::Revision;
use mineral_playback::OpenedMedia;

use super::convert::PcmConverter;
use super::render::{Decoding, SharedPlayback};
use crate::decoder::{DecodedAudio, Decoder, PcmFormat};
use crate::{Error, Result};

pub(super) fn spawn(
    media: OpenedMedia,
    playback: Arc<SharedPlayback>,
    id: u64,
    tick: Duration,
) -> Result<()> {
    let cancellation = media.cancellation().clone();
    thread::Builder::new()
        .name(format!("mineral-decode-{id}"))
        .spawn(move || {
            if let Err(error) = decode(media, &playback, id, tick)
                && !cancellation.is_cancelled()
            {
                mineral_log::error!(target: "audio", error = mineral_log::chain(&error), "audio decoding failed");
                if let Some(track) = playback
                    .state
                    .lock()
                    .tracks
                    .iter_mut()
                    .flatten()
                    .find(|track| track.id == id)
                {
                    track.decoding = Decoding::Failed(Arc::new(error));
                }
            }
        })
        .map_err(|source| Error::SpawnThread { source })?;
    Ok(())
}

struct Work {
    generation: Revision,

    output: PcmFormat,

    seek_position: Option<Duration>,

    chunk_frames: usize,
}

fn wait_for_work(
    playback: &SharedPlayback,
    id: u64,
    generation: Option<Revision>,
    ended: bool,
    tick: Duration,
) -> Option<Work> {
    loop {
        let state = playback.state.lock();
        let track = state.tracks.iter().flatten().find(|track| track.id == id)?;
        if track.cancellation.is_cancelled() || track.finished {
            return None;
        }
        if let Some(output) = &state.output
            && (generation != Some(track.generation)
                || (!ended && track.samples.len() < output.capacity_samples))
        {
            if let Some(control) = &track.read_control {
                control.begin_read();
            }
            return Some(Work {
                generation: track.generation,
                output: output.pcm,
                seek_position: track.seek_position,
                chunk_frames: output.capacity_samples / usize::from(output.pcm.channels.get()),
            });
        }
        drop(state);
        // The callback never wakes or parks threads. Workers refill on the engine cadence.
        thread::sleep(tick);
    }
}

fn decode(media: OpenedMedia, playback: &SharedPlayback, id: u64, tick: Duration) -> Result<()> {
    let read_control = media.read_control().cloned();
    let mut decoder =
        Decoder::from_input(media.into_audio_input()).map_err(|source| Error::Decode {
            source: Arc::new(source),
        })?;
    {
        let mut state = playback.state.lock();
        let Some(track) = state
            .tracks
            .iter_mut()
            .flatten()
            .find(|track| track.id == id)
        else {
            return Ok(());
        };
        track.read_control = read_control;
        track.duration = decoder.duration();
        track.source_format = Some(decoder.format());
        mineral_log::info!(
            target: "audio",
            id,
            sample_rate = decoder.format().sample_rate.get(),
            duration = ?decoder.duration(),
            "decoder ready"
        );
    }

    let mut generation = None;
    let mut converter: Option<PcmConverter> = None;
    let mut pending = Vec::new().into_iter();
    let mut following_format: Option<DecodedAudio> = None;
    let mut input_ended = false;
    let mut ended = false;
    let mut origin = None;
    while let Some(work) = wait_for_work(playback, id, generation, ended, tick) {
        let step = (|| -> Result<()> {
            if generation != Some(work.generation) {
                if let Some(position) = work.seek_position {
                    let actual = decoder.seek(position).map_err(|source| Error::Decode {
                        source: Arc::new(source),
                    })?;
                    mineral_log::debug!(target: "audio", id, generation = ?work.generation, position = ?actual, "decoder seek completed");
                    if let Some(track) = playback
                        .state
                        .lock()
                        .tracks
                        .iter_mut()
                        .flatten()
                        .find(|track| track.id == id && track.generation == work.generation)
                    {
                        track.origin = actual;
                        track.seek_position = None;
                    }
                }
                generation = Some(work.generation);
                converter = None;
                pending = Vec::new().into_iter();
                following_format = None;
                input_ended = false;
                ended = false;
                origin = None;
            }

            while pending.len() == 0 && !ended {
                if let Some(active) = converter.as_mut() {
                    if let Some(samples) = active.next(input_ended)? {
                        pending = samples.into_iter();
                        break;
                    }
                    if input_ended {
                        if following_format.is_none() {
                            ended = true;
                            break;
                        }
                        converter = None;
                        input_ended = false;
                    }
                }
                let audio = match following_format.take() {
                    Some(audio) => Some(audio),
                    None => decoder.read().map_err(|source| Error::Decode {
                        source: Arc::new(source),
                    })?,
                };
                let Some(audio) = audio else {
                    input_ended = true;
                    if converter.is_none() {
                        ended = true;
                    }
                    continue;
                };
                origin.get_or_insert(audio.position);
                if let Some(active) = &converter
                    && active.input_format != audio.format
                {
                    following_format = Some(audio);
                    input_ended = true;
                    continue;
                }
                if converter.is_none() {
                    converter = Some(PcmConverter::new(
                        audio.format,
                        work.output,
                        work.chunk_frames,
                    )?);
                }
                if let Some(active) = converter.as_mut() {
                    active.push(&audio.samples);
                }
            }

            let mut state = playback.state.lock();
            let Some(capacity) = state.output.as_ref().map(|output| output.capacity_samples) else {
                return Ok(());
            };
            let Some(track) = state
                .tracks
                .iter_mut()
                .flatten()
                .find(|track| track.id == id)
            else {
                return Ok(());
            };
            if track.generation != work.generation {
                return Ok(());
            }
            track.duration = decoder.duration();
            if track.rendered_frames == 0
                && track.samples.is_empty()
                && let Some(position) = origin
            {
                track.origin = position;
            }
            let channels = usize::from(work.output.channels.get());
            let writable =
                (capacity - track.samples.len()).min(pending.len()) / channels * channels;
            track.samples.extend(pending.by_ref().take(writable));
            if ended && pending.len() == 0 {
                track.decoding = Decoding::Ended;
            }
            Ok(())
        })();
        if let Err(error) = step {
            let mut state = playback.state.lock();
            if let Some(track) = state
                .tracks
                .iter_mut()
                .flatten()
                .find(|track| track.id == id && track.generation == work.generation)
            {
                mineral_log::error!(target: "audio", id, error = mineral_log::chain(&error), "audio decoding failed");
                track.decoding = Decoding::Failed(Arc::new(error));
                return Ok(());
            }
            // A newer seek owns this instance, including errors from obsolete work.
        }
    }
    Ok(())
}
