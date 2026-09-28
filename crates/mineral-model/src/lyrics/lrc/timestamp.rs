//! Parses LRC timestamps and applies the document's signed millisecond offset.

/// Accepts minutes, 0–59 seconds and optional 1–3 digit fractions separated by a dot or colon.
pub(super) fn parse_timestamp(text: &str) -> Option<u64> {
    let (minutes, remainder) = text.split_once(':')?;
    let minutes = decimal(minutes)?;
    let (seconds, fraction) = match remainder.split_once(['.', ':']) {
        Some((seconds, fraction)) => (seconds, Some(fraction)),
        None => (remainder, None),
    };
    if seconds.len() > 2 {
        return None;
    }
    let seconds = decimal(seconds)?;
    if seconds >= 60 {
        return None;
    }
    let milliseconds = match fraction {
        None => 0,
        Some(digits) => {
            let scale = match digits.len() {
                1 => 100,
                2 => 10,
                3 => 1,
                _ => return None,
            };
            decimal(digits)? * scale
        }
    };
    minutes
        .checked_mul(60_000)?
        .checked_add(seconds * 1_000)?
        .checked_add(milliseconds)
}

/// Rejects signs, whitespace and non-ASCII digits before converting a timestamp component.
fn decimal(digits: &str) -> Option<u64> {
    if digits.is_empty() || !digits.bytes().all(|byte| byte.is_ascii_digit()) {
        return None;
    }
    digits.parse().ok()
}

/// Moves positive offsets earlier, clamps before zero and reports overflow as an absent time.
pub(super) fn apply_offset(time_ms: u64, offset_ms: Option<i64>) -> Option<u64> {
    match offset_ms {
        Some(offset) if offset < 0 => time_ms.checked_add(offset.unsigned_abs()),
        Some(offset) => Some(time_ms.saturating_sub(offset.unsigned_abs())),
        None => Some(time_ms),
    }
}

/// Recognizes NetEase's negative-credit timestamp spelling, such as `00:00.00-1`.
pub(super) fn is_negative_credit_timestamp(text: &str) -> bool {
    text.rsplit_once('-').is_some_and(|(timestamp, suffix)| {
        !suffix.is_empty()
            && suffix.bytes().all(|byte| byte.is_ascii_digit())
            && parse_timestamp(timestamp).is_some()
    })
}
