use crate::astrometry_headers::FitsAstrometryHeaders;
use crate::image_io::FrameHeader;
use psf_guard_director_core::{
    ephemeris,
    visibility::{IcrsPosition, Site},
};
use seiza_fits::HeaderValue;
use std::sync::LazyLock;

pub(super) fn number(header: &FrameHeader, key: &str, low: f64, high: f64) -> Option<f64> {
    let value = &header
        .cards
        .iter()
        .find(|(name, _)| name.eq_ignore_ascii_case(key))?
        .1;
    let n = match value {
        HeaderValue::Float(n) => *n,
        HeaderValue::Integer(n) => *n as f64,
        HeaderValue::String(n) | HeaderValue::Raw(n) => n.trim().parse().ok()?,
        _ => return None,
    };
    (n.is_finite() && (low..=high).contains(&n)).then_some(n)
}

pub(super) fn bandpass(header: &FrameHeader) -> Option<f64> {
    if let Some(n) = number(header, "PGBAND", f64::MIN_POSITIVE, 1e6) {
        return Some(n);
    }
    // Only an explicit nm suffix on the saved filter label is evidence. A
    // current rig profile or the remote requirement is not a capture record.
    static NM: LazyLock<regex::Regex> = LazyLock::new(|| {
        regex::Regex::new(
            r"(?i)^\s*(?:Ha|H-alpha|Halpha|OIII|O3|SII|S2)\s+(\d+(?:\.\d+)?)\s*nm\s*$",
        )
        .unwrap()
    });
    let value = header
        .cards
        .iter()
        .find(|(name, _)| name.eq_ignore_ascii_case("FILTER"))?;
    let HeaderValue::String(label) = &value.1 else {
        return None;
    };
    let n: f64 = NM.captures(label)?.get(1)?.as_str().parse().ok()?;
    (n.is_finite() && n > 0.0 && n <= 1e6).then_some(n)
}

pub(super) fn moon(
    headers: &FitsAstrometryHeaders,
    captured: i64,
    exposure_seconds: f64,
    center: IcrsPosition,
) -> (Option<f64>, Option<f64>) {
    let start = match &headers.capture_time {
        Some(time) => chrono::DateTime::parse_from_rfc3339(&time.value)
            .map(|t| t.timestamp_millis())
            .ok()
            .or_else(|| {
                chrono::NaiveDateTime::parse_from_str(&time.value, "%Y-%m-%dT%H:%M:%S%.f")
                    .ok()
                    .map(|t| t.and_utc().timestamp_millis())
            }),
        None => captured.checked_mul(1000),
    };
    let Some(midpoint) = start
        .and_then(|t| t.checked_add((exposure_seconds * 500.0).round() as i64))
        .and_then(|t| u64::try_from(t).ok())
        .filter(|t| *t <= 4_102_444_800_000)
    else {
        return (None, None);
    };
    let illumination = ephemeris::moon_illumination(midpoint);
    let separation = headers.observer.as_ref().and_then(|observer| {
        let site = observer.value;
        ephemeris::moon_at_site(
            midpoint,
            Site {
                latitude_degrees: site.latitude_deg,
                longitude_degrees: site.longitude_deg,
                elevation_meters: site.altitude_m,
            },
        )
        .map(|moon| ephemeris::separation_degrees(center, moon))
    });
    (illumination, separation)
}

pub(super) fn director_capture(header: &FrameHeader) -> Result<Option<uuid::Uuid>, ()> {
    let mut values = header
        .cards
        .iter()
        .filter(|(name, _)| name.eq_ignore_ascii_case("PGCAPID"));
    let Some((_, value)) = values.next() else {
        return Ok(None);
    };
    if values.next().is_some() {
        return Err(());
    }
    let HeaderValue::String(value) = value else {
        return Err(());
    };
    uuid::Uuid::parse_str(value)
        .ok()
        .filter(|id| !id.is_nil())
        .map(Some)
        .ok_or(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn header(cards: &[(&str, HeaderValue)]) -> FrameHeader {
        FrameHeader {
            cards: cards
                .iter()
                .map(|(k, v)| ((*k).into(), v.clone()))
                .collect(),
            ..Default::default()
        }
    }

    #[test]
    fn measurements_need_recorded_units_and_do_not_fill_unknowns_from_requirements() {
        let empty = FrameHeader::default();
        assert_eq!(number(&empty, "PGGRMS", 0.0, 1e6), None);
        assert_eq!(
            number(
                &header(&[("PGGRMS", HeaderValue::Float(0.5))]),
                "PGGRMS",
                0.0,
                1e6
            ),
            Some(0.5)
        );
        assert_eq!(
            number(
                &header(&[("PGGRMS", HeaderValue::Float(-1.0))]),
                "PGGRMS",
                0.0,
                1e6
            ),
            None
        );
        for filter in ["Ha", "OIII", "Dual 3nm/5nm", "Ha 0nm", "Ha 3 angstrom"] {
            assert_eq!(
                bandpass(&header(&[("FILTER", HeaderValue::String(filter.into()))])),
                None
            );
        }
        assert_eq!(
            bandpass(&header(&[("FILTER", HeaderValue::String("Ha 3nm".into()))])),
            Some(3.0)
        );
        assert_eq!(
            bandpass(&header(&[("PGBAND", HeaderValue::Float(5.0))])),
            Some(5.0)
        );
        assert_eq!(
            bandpass(&header(&[("PGBAND", HeaderValue::Float(f64::NAN))])),
            None
        );
    }

    #[test]
    fn lunar_context_uses_saved_midpoint_and_site_not_the_current_night() {
        let cards = header(&[
            (
                "DATE-OBS",
                HeaderValue::String("2026-09-26T16:46:30".into()),
            ),
            ("SITELAT", HeaderValue::Float(35.0)),
            ("SITELONG", HeaderValue::Float(-120.0)),
            ("SITEELEV", HeaderValue::Float(1000.0)),
        ]);
        let headers = FitsAstrometryHeaders::from_headers(&cards.cards);
        let center = IcrsPosition {
            ra_degrees: 10.0,
            dec_degrees: 20.0,
        };
        let actual = moon(&headers, 1_789_097_220, 300.0, center);
        assert!(actual.0.unwrap() > 0.99);
        assert!((0.0..=180.0).contains(&actual.1.unwrap()));
        let mut no_site = headers.clone();
        no_site.observer = None;
        assert_eq!(moon(&no_site, 1_789_097_220, 300.0, center).1, None);
        let mut invalid_time = headers;
        invalid_time.capture_time.as_mut().unwrap().value = "invalid".into();
        assert_eq!(
            moon(&invalid_time, 1_789_097_220, 300.0, center),
            (None, None)
        );
    }

    #[test]
    fn director_identity_is_explicit_and_bad_or_duplicate_headers_never_become_ts_captures() {
        assert_eq!(director_capture(&FrameHeader::default()), Ok(None));
        let id = uuid::Uuid::new_v4();
        let valid = ("PGCAPID", HeaderValue::String(id.to_string()));
        assert_eq!(
            director_capture(&header(std::slice::from_ref(&valid))),
            Ok(Some(id))
        );
        assert_eq!(director_capture(&header(&[valid.clone(), valid])), Err(()));
        for bad in ["broken", "00000000-0000-0000-0000-000000000000"] {
            assert_eq!(
                director_capture(&header(&[("PGCAPID", HeaderValue::String(bad.into()))])),
                Err(())
            );
        }
    }
}
