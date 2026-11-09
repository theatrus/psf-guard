//! What each of a rig's nights is available for: the marks set by hand for
//! the rig and for its planning site, and, where the site has no mark, the
//! site's cloud forecast. Night plans and feasibility both plan inside it.
//! Planning input only: it never allows or blocks acquisition.

use super::*;
use psf_guard_director_core::{
    night::{NightDate, NightSky},
    visibility::Site,
    windows::Interval,
};
use psf_guard_director_meta::{
    night_availability::{Availability, MarkScope, NightMark},
    site_forecast::{
        ForecastHour, ForecastNight, ForecastProvider, ForecastSettings, SiteForecast,
    },
    site_profile::RigSite,
};

/// A night's effective availability and the marks that set it.
#[derive(Serialize)]
pub(super) struct NightAvailability {
    pub(super) state: Availability,
    /// The clear hours of a limited night: the site's and the rig's windows
    /// met. Empty otherwise.
    pub(super) windows: Vec<Interval>,
    pub(super) marks: Vec<AppliedMark>,
}

impl NightAvailability {
    /// A night with no marks and no forecast: available in full.
    pub(super) fn whole() -> Self {
        Self {
            state: Availability::Available,
            windows: vec![],
            marks: vec![],
        }
    }
}

#[derive(Serialize)]
pub(super) struct AppliedMark {
    scope: MarkScope,
    state: Availability,
    windows: Vec<Interval>,
    note: String,
    source: MarkSource,
}

/// Where a mark came from.
#[derive(Clone, Copy, Debug, PartialEq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub(super) enum MarkSource {
    /// Marked by hand.
    Manual,
    /// The site's cloud forecast, where no mark says otherwise.
    Forecast {
        provider: ForecastProvider,
        fetched_at_ms: u64,
    },
}

/// A site's forecast choice and its stored forecast.
struct SiteClouds {
    settings: ForecastSettings,
    /// From the provider the site has now.
    forecast: Option<SiteForecast>,
}

/// The cloud forecast of a rig's planning site.
#[derive(Serialize)]
pub(super) struct RigForecast {
    provider: ForecastProvider,
    /// The credit the provider's licence requires.
    attribution: &'static str,
    cloud_threshold_percent: u8,
    /// Hours this close to the dew point, in °C, are not clear.
    dew_limit_celsius: Option<u8>,
    /// The forecast planning uses, when one is kept and has not expired.
    fetched_at_ms: Option<u64>,
    expires_at_ms: Option<u64>,
}

impl RigForecast {
    /// When the forecast stops counting, and the nights planned from it
    /// count as clear.
    pub(super) fn expires_at_ms(&self) -> Option<u64> {
        self.expires_at_ms
    }
}

/// What a rig's nights are planned against, read from the store.
pub(super) struct RigConditions {
    rig_marks: Vec<NightMark>,
    site_marks: Vec<NightMark>,
    clouds: Option<SiteClouds>,
    /// The revision of the rig's marks, to send back with a change.
    pub(super) rig_revision: u64,
    /// The revision of the planning site's marks, when the rig has a site.
    pub(super) site_revision: Option<u64>,
}

/// Each of a rig's nights, as it is available.
pub(super) struct Nights {
    /// Per night: the clear windows, `None` for the whole night, and what
    /// the response shows.
    pub(super) nights: Vec<(Option<Vec<Interval>>, NightAvailability)>,
    /// Per night: the forecast's hours, empty without a forecast.
    pub(super) clouds: Vec<Vec<ForecastHour>>,
    /// The windows for the core's `NightRequest::available` across all the
    /// nights; `None` when every night is available in full.
    pub(super) available: Option<Vec<Interval>>,
    /// The site's forecast, when it takes one.
    pub(super) forecast: Option<RigForecast>,
}

impl RigConditions {
    /// The rig's and its planning site's marks from `first` to `last`, and
    /// the site's forecast.
    pub(super) fn read(
        store: &MetaStore,
        rig: Uuid,
        placed: &RigSite,
        first: NightDate,
        last: NightDate,
    ) -> Result<Self, StoreError> {
        let rig_marks = store.night_marks(MarkScope::Rig, rig, first, last)?;
        let site_marks = placed
            .site
            .as_ref()
            .map(|site| store.night_marks(MarkScope::Site, site.id, first, last))
            .transpose()?;
        let mut clouds = None;
        if let Some(site) = &placed.site
            && let Some(settings) = store.site_profile(site.id)?.and_then(|p| p.forecast)
        {
            let forecast = store
                .site_forecast(site.id)?
                .filter(|f| f.provider == settings.provider);
            clouds = Some(SiteClouds { settings, forecast });
        }
        Ok(Self {
            rig_revision: rig_marks.revision,
            site_revision: site_marks.as_ref().map(|marks| marks.revision),
            rig_marks: rig_marks.marks,
            site_marks: site_marks.map(|marks| marks.marks).unwrap_or_default(),
            clouds,
        })
    }

    /// Each night's availability at the rig's site. An expired forecast
    /// says nothing, so freshness is judged at `now_ms`, not when planning
    /// starts.
    ///
    /// `skies` holds each night's sky at the site, sampled every five
    /// minutes as the summaries are, as
    /// [`Skies::nights`](super::night_sky::Skies::nights) gives them for
    /// the noons of `dates`.
    pub(super) fn nights(
        &self,
        dates: &[NightDate],
        site: Site,
        now_ms: u64,
        skies: &[Option<Arc<NightSky>>],
    ) -> Nights {
        let longitude_degrees = site.longitude_degrees;
        let dark = darkness(skies, DARK_BELOW_DEGREES);
        let twilight = darkness(skies, TWILIGHT_BELOW_DEGREES);
        let forecast = self.clouds.as_ref().and_then(|clouds| {
            let kept = clouds.forecast.as_ref().filter(|f| f.fresh(now_ms))?;
            Some((clouds.settings, kept))
        });
        // A forecast that covers none of a night's dark hours says nothing
        // about that night.
        let forecast_nights: Vec<Option<ForecastNight>> = dates
            .iter()
            .zip(&dark)
            .map(|(date, dark)| {
                let (settings, kept) = forecast?;
                let night = kept.night(date.span(longitude_degrees), &settings, now_ms)?;
                let covers_dark = dark.is_none_or(|(dusk, dawn)| {
                    night
                        .covered
                        .iter()
                        .any(|covered| covered.end_ms > dusk && covered.start_ms < dawn)
                });
                covers_dark.then_some(night)
            })
            .collect();
        let source = forecast.map(|(_, kept)| MarkSource::Forecast {
            provider: kept.provider,
            fetched_at_ms: kept.fetched_at_ms,
        });
        let nights: Vec<(Option<Vec<Interval>>, NightAvailability)> = dates
            .iter()
            .zip(&forecast_nights)
            .zip(dark.iter().zip(&twilight))
            .map(|((date, night), (dark, twilight))| {
                let (windows, mut shown) = effective(
                    *date,
                    longitude_degrees,
                    &self.site_marks,
                    &self.rig_marks,
                    night.as_ref().zip(source),
                );
                judge_by_darkness(&mut shown, *dark, *twilight);
                (windows, shown)
            })
            .collect();
        let available = (!nights.iter().all(|(windows, _)| windows.is_none())).then(|| {
            nights
                .iter()
                .zip(dates)
                .flat_map(|((windows, _), date)| {
                    windows
                        .clone()
                        .unwrap_or_else(|| vec![date.span(longitude_degrees)])
                })
                .collect()
        });
        Nights {
            nights,
            clouds: forecast_nights
                .into_iter()
                .map(|night| night.map(|night| night.hours).unwrap_or_default())
                .collect(),
            available,
            forecast: self.clouds.as_ref().map(|clouds| RigForecast {
                provider: clouds.settings.provider,
                attribution: clouds.settings.provider.attribution(),
                cloud_threshold_percent: clouds.settings.cloud_threshold_percent,
                dew_limit_celsius: clouds.settings.dew_limit_celsius,
                fetched_at_ms: forecast.map(|(_, kept)| kept.fetched_at_ms),
                expires_at_ms: forecast.map(|(_, kept)| kept.expires_at_ms),
            }),
        }
    }
}

/// A night's availability from the site's and the rig's marks, and where
/// the site has no mark, from its forecast: `None` for the whole night,
/// else the clear windows, empty when none.
fn effective(
    date: NightDate,
    longitude_degrees: f64,
    site: &[NightMark],
    rig: &[NightMark],
    forecast: Option<(&ForecastNight, MarkSource)>,
) -> (Option<Vec<Interval>>, NightAvailability) {
    let span = date.span(longitude_degrees);
    let mut marks = Vec::new();
    let mut windows: Option<Vec<Interval>> = None;
    for (scope, list) in [(MarkScope::Site, site), (MarkScope::Rig, rig)] {
        let mark = match (list.iter().find(|mark| mark.date == date), scope, forecast) {
            (Some(mark), _, _) => AppliedMark {
                scope,
                state: mark.state,
                windows: mark.windows.clone(),
                note: mark.note.clone(),
                source: MarkSource::Manual,
            },
            (None, MarkScope::Site, Some((night, source))) => {
                let clear = forecast_windows(span, night);
                let state = if clear == [span] {
                    Availability::Available
                } else if clear.is_empty() {
                    Availability::Unavailable
                } else {
                    Availability::Limited
                };
                AppliedMark {
                    scope,
                    state,
                    windows: if state == Availability::Limited {
                        clear
                    } else {
                        vec![]
                    },
                    note: String::new(),
                    source,
                }
            }
            _ => continue,
        };
        let clear = match mark.state {
            Availability::Available => {
                marks.push(mark);
                continue;
            }
            Availability::Unavailable => vec![],
            Availability::Limited => mark
                .windows
                .iter()
                .filter_map(|w| {
                    let start_ms = w.start_ms.max(span.start_ms);
                    let end_ms = w.end_ms.min(span.end_ms);
                    (start_ms < end_ms).then_some(Interval { start_ms, end_ms })
                })
                .collect(),
        };
        marks.push(mark);
        windows = Some(match windows {
            None => clear,
            Some(previous) => intersect(&previous, &clear),
        });
    }
    let state = match &windows {
        None => Availability::Available,
        Some(clear) if clear.is_empty() => Availability::Unavailable,
        Some(_) => Availability::Limited,
    };
    let shown = match (&windows, state) {
        (Some(clear), Availability::Limited) => clear.clone(),
        _ => vec![],
    };
    (
        windows,
        NightAvailability {
            state,
            windows: shown,
            marks,
        },
    )
}

/// A forecast's clear hours in a night, with the hours it does not cover:
/// no information is no reason to plan less.
fn forecast_windows(span: Interval, night: &ForecastNight) -> Vec<Interval> {
    let mut windows = night.clear.clone();
    // Before the forecast's first hour lies the past, as it was when the
    // forecast arrived: that is not clear.
    let mut from = night
        .covered
        .first()
        .map_or(span.start_ms, |first| first.start_ms);
    for covered in &night.covered {
        if covered.start_ms > from {
            windows.push(Interval {
                start_ms: from,
                end_ms: covered.start_ms,
            });
        }
        from = from.max(covered.end_ms);
    }
    if from < span.end_ms {
        windows.push(Interval {
            start_ms: from,
            end_ms: span.end_ms,
        });
    }
    windows.sort_by_key(|w| w.start_ms);
    let mut merged: Vec<Interval> = Vec::with_capacity(windows.len());
    for window in windows {
        match merged.last_mut() {
            Some(last) if last.end_ms >= window.start_ms => {
                last.end_ms = last.end_ms.max(window.end_ms)
            }
            _ => merged.push(window),
        }
    }
    merged
}

/// The Sun's altitude below which the night summaries count the sky dark.
const DARK_BELOW_DEGREES: f64 = -12.0;
/// The Sun's altitude below which any schedule may image: no Target
/// Scheduler twilight level allows imaging above it.
const TWILIGHT_BELOW_DEGREES: f64 = -6.0;

/// A forecast's windows run noon to noon, but a night is its dark hours.
/// Where the forecast decides the night, it shows as available when it is
/// clear through all the dark hours, and as not available when it is clear
/// at no time a schedule could image. A mark set by hand keeps the night as
/// it was marked. Only the display changes: the schedules still keep out of
/// the cloudy twilight.
fn judge_by_darkness(
    availability: &mut NightAvailability,
    dark: Option<(u64, u64)>,
    twilight: Option<(u64, u64)>,
) {
    let clear_in = |windows: &[Interval], (from, to): (u64, u64)| -> u64 {
        windows
            .iter()
            .map(|w| w.end_ms.min(to).saturating_sub(w.start_ms.max(from)))
            .sum()
    };
    let judge = |state: &mut Availability, windows: &mut Vec<Interval>| {
        if *state != Availability::Limited {
            return;
        }
        if let Some((dusk, dawn)) = dark
            && clear_in(windows, (dusk, dawn)) + 60_000 >= dawn.saturating_sub(dusk)
        {
            *state = Availability::Available;
            windows.clear();
        } else if twilight.is_none_or(|bounds| clear_in(windows, bounds) < 60_000) {
            *state = Availability::Unavailable;
            windows.clear();
        }
    };
    let mut forecast = false;
    for mark in &mut availability.marks {
        if matches!(mark.source, MarkSource::Forecast { .. }) {
            forecast = true;
            judge(&mut mark.state, &mut mark.windows);
        }
    }
    let by_hand = availability
        .marks
        .iter()
        .any(|mark| mark.source == MarkSource::Manual && mark.state != Availability::Available);
    if forecast && !by_hand {
        judge(&mut availability.state, &mut availability.windows);
    }
}

/// Each night's dusk and dawn at the site, with the Sun below
/// `below_degrees`, from its sky: `None` for a night when it never gets
/// that low, or whose sky could not be computed.
fn darkness(skies: &[Option<Arc<NightSky>>], below_degrees: f64) -> Vec<Option<(u64, u64)>> {
    skies
        .iter()
        .map(|sky| sky.as_ref()?.darkness(below_degrees))
        .collect()
}

/// The instants in both sorted lists of apart windows.
fn intersect(a: &[Interval], b: &[Interval]) -> Vec<Interval> {
    let mut result = Vec::new();
    for x in a {
        for y in b {
            let start_ms = x.start_ms.max(y.start_ms);
            let end_ms = x.end_ms.min(y.end_ms);
            if start_ms < end_ms {
                result.push(Interval { start_ms, end_ms });
            }
        }
    }
    result.sort_by_key(|w| w.start_ms);
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mark(date: NightDate, state: Availability, windows: Vec<Interval>) -> NightMark {
        NightMark {
            date,
            state,
            windows,
            note: String::new(),
            revision: 1,
            updated_at_ms: 0,
        }
    }

    #[test]
    fn site_and_rig_marks_meet_and_a_mark_elsewhere_changes_nothing() {
        let date: NightDate = "2026-11-03".parse().unwrap();
        let span = date.span(-118.3);
        let hour = 3_600_000;
        let at = |from: u64, to: u64| Interval {
            start_ms: span.start_ms + from * hour,
            end_ms: span.start_ms + to * hour,
        };
        let (none, shown) = effective(date, -118.3, &[], &[], None);
        assert_eq!((none, shown.state), (None, Availability::Available));
        // Clear sky, rig down: out.
        let (out, shown) = effective(
            date,
            -118.3,
            &[mark(date, Availability::Available, vec![])],
            &[mark(date, Availability::Unavailable, vec![])],
            None,
        );
        assert_eq!(out, Some(vec![]));
        assert_eq!(shown.state, Availability::Unavailable);
        assert_eq!(shown.marks.len(), 2);
        // The sky clears from 8 to 12 hours after noon and the rig can run
        // from 10 to 14: two hours.
        let (both, shown) = effective(
            date,
            -118.3,
            &[mark(date, Availability::Limited, vec![at(8, 12)])],
            &[mark(date, Availability::Limited, vec![at(10, 14)])],
            None,
        );
        assert_eq!(both, Some(vec![at(10, 12)]));
        assert_eq!(
            (shown.state, shown.windows),
            (Availability::Limited, vec![at(10, 12)])
        );
        // Windows that do not meet leave nothing.
        let (apart, shown) = effective(
            date,
            -118.3,
            &[mark(date, Availability::Limited, vec![at(8, 9)])],
            &[mark(date, Availability::Limited, vec![at(10, 14)])],
            None,
        );
        assert_eq!(
            (apart, shown.state),
            (Some(vec![]), Availability::Unavailable)
        );
        let other = date.next().unwrap();
        let (elsewhere, _) = effective(
            date,
            -118.3,
            &[],
            &[mark(other, Availability::Unavailable, vec![])],
            None,
        );
        assert_eq!(elsewhere, None);
    }

    #[test]
    fn a_forecast_counts_later_uncovered_hours_clear_and_is_judged_by_the_dark() {
        let date: NightDate = "2026-11-03".parse().unwrap();
        let span = date.span(-118.3);
        let hour = 3_600_000;
        let at = |from: u64, to: u64| Interval {
            start_ms: span.start_ms + from * hour,
            end_ms: span.start_ms + to * hour,
        };
        // Covered from 2 h to 20 h, clear from 4 to 6 and 10 to 20. Before
        // the first hour is the past, not clear; after the last is clear.
        let night = ForecastNight {
            covered: vec![at(2, 20)],
            clear: vec![at(4, 6), at(10, 20)],
            hours: vec![],
        };
        assert_eq!(forecast_windows(span, &night), vec![at(4, 6), at(10, 24)]);
        let source = MarkSource::Forecast {
            provider: ForecastProvider::OpenMeteo,
            fetched_at_ms: 0,
        };
        let (windows, mut shown) = effective(date, -118.3, &[], &[], Some((&night, source)));
        assert_eq!(windows, Some(vec![at(4, 6), at(10, 24)]));
        assert_eq!(shown.state, Availability::Limited);
        // Dark from 10 h to 21 h: clear all through it, so available.
        let bounds = |from: u64, to: u64| Some((at(from, from).start_ms, at(to, to).start_ms));
        judge_by_darkness(&mut shown, bounds(10, 21), bounds(9, 22));
        assert_eq!(
            (shown.state, shown.marks[0].state),
            (Availability::Available, Availability::Available)
        );
        // Dark from 7 h to 9 h: clear in none of it, but clear from 6 h in
        // the twilight a schedule may still use, so it stays limited.
        let (_, mut shown) = effective(date, -118.3, &[], &[], Some((&night, source)));
        judge_by_darkness(&mut shown, bounds(7, 9), bounds(5, 10));
        assert_eq!(shown.state, Availability::Limited);
        // Clear at no time a schedule could image: not available.
        let (_, mut shown) = effective(date, -118.3, &[], &[], Some((&night, source)));
        judge_by_darkness(&mut shown, bounds(7, 9), bounds(6, 10));
        assert_eq!(shown.state, Availability::Unavailable);
        // A rig's mark set by hand keeps the night limited.
        let (_, mut marked) = effective(
            date,
            -118.3,
            &[],
            &[mark(date, Availability::Limited, vec![at(8, 22)])],
            Some((&night, source)),
        );
        judge_by_darkness(&mut marked, bounds(10, 21), bounds(9, 22));
        assert_eq!(marked.state, Availability::Limited);
        assert_eq!(
            (marked.marks[0].state, marked.marks[1].source),
            (Availability::Available, MarkSource::Manual)
        );
    }

    /// A night's darkness from its sky is the dusk and dawn a summary with
    /// no targets gives, as it was found before skies were kept; a night
    /// that never gets dark enough has none.
    #[test]
    fn darkness_from_a_sky_is_a_summarys_dusk_and_dawn() {
        use psf_guard_director_core::{
            night::{night_summary, NightRequest},
            visibility::{AltitudeLimits, Horizon},
            windows::MeridianExclusion,
        };
        const STEP_MS: u64 = 300_000;
        let sites = [(34.2, -118.3), (-33.9, 151.2), (64.8, -147.7), (0.0, 0.0)];
        let dates: Vec<NightDate> = ["2026-06-20", "2026-11-03"]
            .iter()
            .map(|d| d.parse().unwrap())
            .collect();
        let mut nights_without_darkness = 0;
        for (latitude, longitude) in sites {
            let site = Site {
                latitude_degrees: latitude,
                longitude_degrees: longitude,
                elevation_meters: 100.0,
            };
            for date in &dates {
                let noon = date.span(longitude).start_ms;
                let sky = Arc::new(NightSky::new(site, noon, STEP_MS).unwrap());
                for below in [DARK_BELOW_DEGREES, TWILIGHT_BELOW_DEGREES] {
                    let request = NightRequest {
                        site,
                        horizon: Horizon::FixedMinimum {},
                        limits: AltitudeLimits {
                            rig_minimum_degrees: 0.0,
                            project_minimum_degrees: 0.0,
                            horizon_offset_degrees: 0.0,
                            rig_maximum_degrees: 90.0,
                            project_maximum_degrees: 90.0,
                        },
                        targets: vec![],
                        start_ms: noon + 3_600_000,
                        nights: 1,
                        step_ms: STEP_MS,
                        dark_below_degrees: below,
                        meridian_exclusion: MeridianExclusion {
                            before_ms: 0,
                            after_ms: 0,
                        },
                        meridian_window_ms: 0,
                        available: None,
                    };
                    let night = night_summary(&request, 0).unwrap();
                    let summary = night.dusk_ms.zip(night.dawn_ms);
                    nights_without_darkness += usize::from(summary.is_none());
                    assert_eq!(darkness(&[Some(Arc::clone(&sky))], below), vec![summary]);
                }
            }
        }
        assert!(nights_without_darkness > 0);
        assert_eq!(darkness(&[None], DARK_BELOW_DEGREES), vec![None]);
    }
}
