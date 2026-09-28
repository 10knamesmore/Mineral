//! Read MP4 audio-track duration without decoding audio or replacing Lofty's tag reader.

use std::{
    fs::File,
    io::{ErrorKind, Seek},
    path::Path,
    time::Duration,
};

use color_eyre::eyre::{WrapErr, eyre};
use symphonia::{
    core::{
        errors::Error, formats::FormatOptions, io::MediaSourceStream, meta::MetadataOptions,
        probe::Hint,
    },
    default::get_probe,
};

/// Rewind the file already read by Lofty; fragmented MP4 needs packet timestamps.
pub(super) fn read(mut file: File, path: &Path) -> color_eyre::Result<Option<Duration>> {
    file.rewind()
        .wrap_err_with(|| format!("rewind MP4 {}", path.display()))?;
    let source = MediaSourceStream::new(Box::new(file), Default::default());
    let mut format = get_probe()
        .format(
            &Hint::new(),
            source,
            &FormatOptions::default(),
            &MetadataOptions::default(),
        )
        .wrap_err_with(|| format!("probe MP4 duration {}", path.display()))?
        .format;
    let track = format
        .tracks()
        .iter()
        .find(|track| track.codec_params.sample_rate.is_some());
    let Some(track) = track else {
        return Ok(None);
    };
    let track_id = track.id;
    let Some(time_base) = track.codec_params.time_base else {
        return Ok(None);
    };
    if time_base.numer == 0 || time_base.denom == 0 {
        return Ok(None);
    }
    if let Some(ticks) = track.codec_params.n_frames.filter(|ticks| *ticks > 0) {
        return Ok(Some(time_base.calc_time(ticks).into()));
    }

    let mut last_end = None;
    let mut audio_packets = 0_u64;
    loop {
        let packet = match format.next_packet() {
            Ok(packet) => packet,
            Err(Error::IoError(error)) if error.kind() == ErrorKind::UnexpectedEof => break,
            Err(error) => return Err(error).wrap_err("read MP4 audio packets"),
        };
        if packet.track_id() != track_id {
            continue;
        }
        let end = packet
            .ts()
            .checked_add(packet.dur())
            .ok_or_else(|| eyre!("MP4 audio timestamp overflow"))?;
        audio_packets += 1;
        last_end = Some(last_end.map_or(end, |previous: u64| previous.max(end)));
    }
    let duration: Option<Duration> = last_end
        .filter(|ticks| *ticks > 0)
        .map(|ticks| time_base.calc_time(ticks).into());
    mineral_log::debug!(target: "local_library", path = %path.display(), audio_packets, duration_ms = ?duration.map(|value| value.as_millis()), "MP4 duration calculated from audio packets");
    Ok(duration)
}
