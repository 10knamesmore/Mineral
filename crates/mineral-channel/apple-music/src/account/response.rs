//! Classifies Apple HTTP failures without retaining response bodies or request URLs.

use reqwest::Response;

use crate::{Error, Result};

/// iTunes can reject authentication inside an HTTP 200 response.
pub(crate) const SESSION_ENDED_FAILURE: &str = "2002";

pub(super) fn checked_response(response: Response) -> Result<Response> {
    let status = response.status();
    if matches!(status.as_u16(), 401 | 403) {
        return Err(Error::AuthRequired);
    }
    if !status.is_success() {
        return Err(Error::Http {
            status: status.as_u16(),
        });
    }
    Ok(response)
}
