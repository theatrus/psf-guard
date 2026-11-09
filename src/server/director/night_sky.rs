//! The skies night plans and feasibility sum their targets' hours under:
//! for each night at each site, the Sun, the Moon and the Earth at every
//! step, and the Moon track Moon rules are judged on. A sky is most of a
//! night's work, and it is the same for every target, every rig at the
//! site and every request, so it is computed once, across the shared pool,
//! and kept.
//!
//! A sky depends only on the site's location, the night's noon and the
//! step, which together are its key. Nothing it is computed from changes,
//! so a kept sky never goes stale; the least recently used go first once
//! [`MAX_KEPT`] are kept.

use psf_guard_director_core::night::{NightError, NightSky, SkyMoonCell, SkyStep};
use psf_guard_director_core::visibility::Site;
use rayon::prelude::*;
use std::collections::HashMap;
use std::sync::{Arc, Mutex};

/// The most skies kept at once, about 170 kB each with its Moon track: a
/// month of nights at four sites.
const MAX_KEPT: usize = 128;

/// A sky's inputs, to the bit.
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
struct Key {
    latitude: u64,
    longitude: u64,
    elevation: u64,
    noon_ms: u64,
    step_ms: u64,
}

impl Key {
    fn of(site: Site, noon_ms: u64, step_ms: u64) -> Self {
        Self {
            latitude: site.latitude_degrees.to_bits(),
            longitude: site.longitude_degrees.to_bits(),
            elevation: site.elevation_meters.to_bits(),
            noon_ms,
            step_ms,
        }
    }
}

#[derive(Default)]
pub(super) struct Skies {
    kept: Mutex<Kept>,
    /// The most skies kept: [`MAX_KEPT`], or fewer in a test.
    #[cfg(test)]
    limit: std::sync::atomic::AtomicUsize,
    /// How many skies, and how many Moon tracks, were computed.
    #[cfg(test)]
    computed: (
        std::sync::atomic::AtomicUsize,
        std::sync::atomic::AtomicUsize,
    ),
}

/// The skies one request plans under, held for as long as it runs. A
/// request may need more skies than [`MAX_KEPT`], so it does not count on
/// the kept skies still holding the ones it computed at its start.
#[derive(Default)]
pub(super) struct Held(HashMap<Key, Result<Arc<NightSky>, NightError>>);

impl Held {
    /// The held skies of the nights opening at each of `noons` at `site`,
    /// in order; `None` unless every one is held.
    pub(super) fn nights(
        &self,
        site: Site,
        noons: &[u64],
        step_ms: u64,
    ) -> Option<Vec<Result<Arc<NightSky>, NightError>>> {
        noons
            .iter()
            .map(|noon| self.0.get(&Key::of(site, *noon, step_ms)).cloned())
            .collect()
    }
}

#[derive(Default)]
struct Kept {
    skies: HashMap<Key, (Arc<NightSky>, u64)>,
    /// Counts reads, to find the least recently used.
    reads: u64,
}

impl Skies {
    /// The skies of the nights opening at each of `noons` at `site`,
    /// sampled every `step_ms`, in order: kept, or computed now across the
    /// shared pool. With `moon_track`, each carries its Moon track too.
    pub(super) fn nights(
        &self,
        site: Site,
        noons: &[u64],
        step_ms: u64,
        moon_track: bool,
    ) -> Vec<Result<Arc<NightSky>, NightError>> {
        let keys: Vec<Key> = noons
            .iter()
            .map(|noon| Key::of(site, *noon, step_ms))
            .collect();
        let mut found: Vec<Option<Arc<NightSky>>> = {
            let mut kept = self.kept.lock().unwrap_or_else(|e| e.into_inner());
            keys.iter()
                .map(|key| {
                    kept.reads += 1;
                    let reads = kept.reads;
                    kept.skies.get_mut(key).map(|(sky, read)| {
                        *read = reads;
                        Arc::clone(sky)
                    })
                })
                .collect()
        };
        // Every missing night's steps at once, so one night spreads over
        // the pool as well as many.
        let missing: Vec<usize> = (0..noons.len()).filter(|i| found[*i].is_none()).collect();
        let instants: Vec<(usize, u64)> = missing
            .iter()
            .flat_map(|i| {
                NightSky::instants(noons[*i], step_ms)
                    .unwrap_or_default()
                    .into_iter()
                    .map(move |t| (*i, t))
            })
            .collect();
        let steps: Vec<Result<SkyStep, NightError>> = instants
            .par_iter()
            .map(|(i, t)| SkyStep::at(site, noons[*i], *t))
            .collect();
        let mut steps = steps.into_iter();
        let mut computed: Vec<Option<Result<Arc<NightSky>, NightError>>> =
            (0..noons.len()).map(|_| None).collect();
        for i in &missing {
            let count = NightSky::instants(noons[*i], step_ms).map_or(0, |all| all.len());
            let sky = steps
                .by_ref()
                .take(count)
                .collect::<Result<Vec<_>, _>>()
                .and_then(|night| NightSky::from_steps(site, noons[*i], step_ms, night))
                .map(Arc::new);
            if let Ok(sky) = &sky {
                found[*i] = Some(Arc::clone(sky));
                #[cfg(test)]
                self.computed
                    .0
                    .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            }
            computed[*i] = Some(sky);
        }
        if moon_track {
            self.moon_tracks(site, &found);
        }
        // Keep what was computed, with its track when it has one.
        {
            let mut kept = self.kept.lock().unwrap_or_else(|e| e.into_inner());
            for (key, sky) in keys.iter().zip(&computed) {
                if let Some(Ok(sky)) = sky {
                    kept.reads += 1;
                    let reads = kept.reads;
                    kept.skies
                        .entry(*key)
                        .or_insert_with(|| (Arc::clone(sky), reads));
                }
            }
            while kept.skies.len() > self.max_kept() {
                let Some(oldest) = kept
                    .skies
                    .iter()
                    .min_by_key(|(_, (_, read))| *read)
                    .map(|(key, _)| *key)
                else {
                    break;
                };
                kept.skies.remove(&oldest);
            }
        }
        found
            .into_iter()
            .zip(computed)
            .map(|(found, computed)| match (found, computed) {
                (Some(sky), _) => Ok(sky),
                (None, Some(Err(error))) => Err(error),
                (None, _) => Err(NightError::InvalidRequest),
            })
            .collect()
    }

    /// [`Skies::nights`], held in `held` for the rest of the request.
    pub(super) fn hold(
        &self,
        held: &mut Held,
        site: Site,
        noons: &[u64],
        step_ms: u64,
        moon_track: bool,
    ) {
        let skies = self.nights(site, noons, step_ms, moon_track);
        for (noon, sky) in noons.iter().zip(skies) {
            held.0.insert(Key::of(site, *noon, step_ms), sky);
        }
    }

    /// Give each of these skies at `site` its Moon track, when it has none
    /// yet, every missing minute at once across the pool. The skies kept
    /// are the same skies, so they gain the tracks too.
    pub(super) fn add_moon_tracks(&self, site: Site, skies: &[Result<Arc<NightSky>, NightError>]) {
        let skies: Vec<Option<Arc<NightSky>>> =
            skies.iter().map(|sky| sky.as_ref().ok().cloned()).collect();
        self.moon_tracks(site, &skies);
    }

    fn max_kept(&self) -> usize {
        #[cfg(test)]
        {
            let limit = self.limit.load(std::sync::atomic::Ordering::Relaxed);
            if limit > 0 {
                return limit;
            }
        }
        MAX_KEPT
    }

    /// Keep at most `limit` skies, so a test can plan more nights than
    /// are kept without computing [`MAX_KEPT`] of them.
    #[cfg(test)]
    pub(super) fn limit(&self, limit: usize) {
        self.limit
            .store(limit, std::sync::atomic::Ordering::Relaxed);
    }

    /// How many skies and Moon tracks this has computed.
    #[cfg(test)]
    pub(super) fn computed(&self) -> (usize, usize) {
        use std::sync::atomic::Ordering;
        (
            self.computed.0.load(Ordering::Relaxed),
            self.computed.1.load(Ordering::Relaxed),
        )
    }

    /// Let go of every kept sky, so the next request computes its own.
    #[cfg(test)]
    pub(super) fn clear(&self) {
        let mut kept = self.kept.lock().unwrap_or_else(|e| e.into_inner());
        kept.skies.clear();
    }

    /// Give each sky without a Moon track its track, every missing
    /// minute at once across the pool. A night whose track cannot be
    /// computed is left to fail as its summary asks for it.
    fn moon_tracks(&self, site: Site, skies: &[Option<Arc<NightSky>>]) {
        let wanting: Vec<&Arc<NightSky>> = skies
            .iter()
            .flatten()
            .filter(|sky| !sky.has_moon_track())
            .collect();
        let spans: Vec<(u64, _)> = wanting
            .iter()
            .flat_map(|sky| {
                sky.moon_spans()
                    .into_iter()
                    .map(move |span| (sky.noon_ms(), span))
            })
            .collect();
        let cells: Vec<Result<SkyMoonCell, NightError>> = spans
            .par_iter()
            .map(|(noon, span)| SkyMoonCell::at(site, *noon, *span))
            .collect();
        let mut cells = cells.into_iter();
        for sky in wanting {
            let count = sky.moon_spans().len();
            if let Ok(track) = cells.by_ref().take(count).collect::<Result<Vec<_>, _>>()
                && sky.set_moon_track(track).is_ok()
            {
                #[cfg(test)]
                self.computed
                    .1
                    .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use psf_guard_director_core::night::{
        night_summary, night_summary_under, NightRequest, NightTarget,
    };
    use psf_guard_director_core::visibility::{AltitudeLimits, Horizon, IcrsPosition};
    use psf_guard_director_core::windows::MeridianExclusion;

    fn request(moon: bool) -> NightRequest {
        let policy = psf_guard_director_core::moon::MoonPolicy {
            enabled: true,
            ..Default::default()
        };
        NightRequest {
            site: Site {
                latitude_degrees: 34.2,
                longitude_degrees: -118.3,
                elevation_meters: 400.0,
            },
            horizon: Horizon::FixedMinimum {},
            limits: AltitudeLimits {
                rig_minimum_degrees: 20.0,
                project_minimum_degrees: 25.0,
                horizon_offset_degrees: 0.0,
                rig_maximum_degrees: 90.0,
                project_maximum_degrees: 90.0,
            },
            targets: (0..4)
                .map(|i| NightTarget {
                    id: i.to_string(),
                    position: IcrsPosition {
                        ra_degrees: 40.0 + 80.0 * f64::from(i),
                        dec_degrees: 10.0 + 12.0 * f64::from(i),
                    },
                    moon: if moon { vec![policy.clone()] } else { vec![] },
                })
                .collect(),
            start_ms: 1_790_294_400_000,
            nights: 3,
            step_ms: 300_000,
            dark_below_degrees: -12.0,
            meridian_exclusion: MeridianExclusion {
                before_ms: 600_000,
                after_ms: 600_000,
            },
            meridian_window_ms: 0,
            available: None,
        }
    }

    #[test]
    fn kept_skies_give_every_summary_to_the_bit() {
        let skies = Skies::default();
        for moon in [false, true] {
            let request = request(moon);
            let noons: Vec<u64> = (0..3).map(|i| request.noon_ms(i).unwrap()).collect();
            // Computed now, then kept: the same either way.
            for _ in 0..2 {
                let nights = skies.nights(request.site, &noons, request.step_ms, moon);
                for (index, sky) in nights.iter().enumerate() {
                    let sky = sky.as_ref().unwrap();
                    assert_eq!(sky.has_moon_track(), moon);
                    assert_eq!(
                        night_summary_under(&request, index as u32, sky).unwrap(),
                        night_summary(&request, index as u32).unwrap()
                    );
                }
            }
        }
        assert_eq!(skies.kept.lock().unwrap().skies.len(), 3);
    }

    #[test]
    fn the_least_recently_read_skies_go_first() {
        let skies = Skies::default();
        let site = request(false).site;
        let noon = request(false).noon_ms(0).unwrap();
        let noons: Vec<u64> = (0..MAX_KEPT as u64 + 1)
            .map(|i| noon + i * 86_400_000)
            .collect();
        // An hourly step keeps this quick. The first night, read again
        // before the last is added, stays; the second goes.
        skies.nights(site, &noons[..MAX_KEPT], 3_600_000, false);
        skies.nights(site, &noons[..1], 3_600_000, false);
        skies.nights(site, &noons[MAX_KEPT..MAX_KEPT + 1], 3_600_000, false);
        let kept = skies.kept.lock().unwrap();
        assert_eq!(kept.skies.len(), MAX_KEPT);
        assert!(kept.skies.contains_key(&Key::of(site, noons[0], 3_600_000)));
        assert!(!kept.skies.contains_key(&Key::of(site, noons[1], 3_600_000)));
    }
}
