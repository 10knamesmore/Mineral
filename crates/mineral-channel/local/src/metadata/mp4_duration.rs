//! Read MP4 audio-track duration without decoding audio or replacing Lofty's tag reader.

use std::{fs::File, io::Seek, path::Path, time::Duration};

use symphonia::{
    core::{
        formats::{FormatOptions, TrackType, probe::Hint},
        io::MediaSourceStream,
        meta::MetadataOptions,
        units::{Time, Timestamp},
    },
    default::get_probe,
};

use crate::error::{Error, Result};

/// Rewind the file already read by Lofty; fragmented MP4 needs packet timestamps.
pub(super) fn read(mut file: File, path: &Path) -> Result<Option<Duration>> {
    file.rewind().map_err(|source| Error::File {
        operation: "rewind MP4 audio",
        path: path.to_owned(),
        source,
    })?;
    let source = MediaSourceStream::new(Box::new(file), Default::default());
    let mut format = get_probe()
        .probe(
            &Hint::new(),
            source,
            FormatOptions::default(),
            MetadataOptions::default(),
        )
        .map_err(|source| Error::Mp4 {
            operation: "probe MP4 duration",
            path: path.to_owned(),
            source,
        })?;
    let track = format.default_track(TrackType::Audio);
    let Some(track) = track else {
        return Ok(None);
    };
    let track_id = track.id;
    let start_ts = track.start_ts;
    let Some(time_base) = track.time_base else {
        return Ok(None);
    };
    // A fragmented MP4 initialization header may not declare the fragments' duration.
    if let Some(ticks) = track.duration.filter(|duration| !duration.is_zero()) {
        return Ok(time_base.calc_duration(ticks).and_then(to_duration));
    }

    let mut last_end = None;
    let mut audio_packets = 0_u64;
    loop {
        let Some(packet) = format.next_packet().map_err(|source| Error::Mp4 {
            operation: "read MP4 audio packet",
            path: path.to_owned(),
            source,
        })?
        else {
            break;
        };
        if packet.track_id != track_id {
            continue;
        }
        let end =
            packet
                .pts
                .checked_add(packet.dur)
                .ok_or_else(|| Error::Mp4TimestampOverflow {
                    path: path.to_owned(),
                    timestamp: packet.pts.get(),
                    duration: packet.dur.get(),
                })?;
        audio_packets += 1;
        last_end = Some(last_end.map_or(end, |previous: Timestamp| previous.max(end)));
    }
    let duration: Option<Duration> = last_end
        .and_then(|end| end.duration_from(start_ts))
        .and_then(|ticks| time_base.calc_duration(ticks))
        .and_then(to_duration);
    mineral_log::debug!(target: "local_library", path = %path.display(), audio_packets, duration_ms = ?duration.map(|value| value.as_millis()), "MP4 duration calculated from audio packets");
    Ok(duration)
}

/// Converts a nonnegative media time to a filesystem metadata duration.
fn to_duration(time: Time) -> Option<Duration> {
    let (seconds, nanos) = time.parts();
    u64::try_from(seconds)
        .ok()
        .map(|seconds| Duration::new(seconds, nanos))
}
