//! Each rig's rotator angle epochs, from what its lights recorded and what
//! PSF Guard solved (see [`super::lights`]).
//!
//! N.I.N.A. writes a light's synced sky angle (`RotatorPosition`), the
//! rotator's mechanical angle (`RotatorMechanicalPosition`) and the pier
//! side into `acquiredimage.metadata`. Its sky angle comes from its own
//! plate solves while centring. Where PSF Guard has solved a light's pixels
//! (the astrometry cache beside the flat responses), that solve gives the
//! sky angle instead, once put in N.I.N.A.'s convention by the rig's lights
//! that have both.

use super::lights::{half, AngleEpochs};
use rusqlite::Connection;
use std::collections::HashMap;
use std::path::Path;

/// Lights with both a PSF Guard solve and N.I.N.A.'s sky angle needed to
/// put the solves in N.I.N.A.'s convention.
const MIN_CONVENTION_LIGHTS: usize = 5;
/// How far those lights may scatter, degrees, for the convention to hold.
const MAX_CONVENTION_SCATTER_DEG: f64 = 1.0;

/// One light's angles.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct LightAngles {
    pub id: i64,
    pub rig: String,
    pub at: i64,
    pub sky: Option<f64>,
    pub mechanical: f64,
}

fn number(value: &serde_json::Value, key: &str) -> Option<f64> {
    value.get(key)?.as_f64().filter(|v| v.is_finite())
}

/// Every light's recorded angles, by the rig its profile shot with (a
/// profile bound to several rigs says nothing of which).
pub(crate) fn light_angles(
    conn: &Connection,
    rig_of: impl Fn(Option<&str>) -> Vec<String>,
) -> Vec<LightAngles> {
    let has_profile = conn
        .prepare("SELECT profileId FROM acquiredimage LIMIT 0")
        .is_ok();
    let sql = if has_profile {
        "SELECT Id, profileId, acquireddate, metadata FROM acquiredimage
         WHERE acquireddate IS NOT NULL AND metadata LIKE '%RotatorMechanicalPosition%'"
    } else {
        "SELECT Id, NULL, acquireddate, metadata FROM acquiredimage
         WHERE acquireddate IS NOT NULL AND metadata LIKE '%RotatorMechanicalPosition%'"
    };
    let Ok(mut statement) = conn.prepare(sql) else {
        return Vec::new();
    };
    let Ok(rows) = statement.query_map([], |row| {
        Ok((
            row.get::<_, i64>(0)?,
            row.get::<_, Option<String>>(1)?,
            row.get::<_, i64>(2)?,
            row.get::<_, String>(3)?,
        ))
    }) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for (id, profile, at, metadata) in rows.flatten() {
        let rigs = rig_of(profile.as_deref());
        let [rig] = rigs.as_slice() else {
            continue;
        };
        let Ok(value) = serde_json::from_str::<serde_json::Value>(&metadata) else {
            continue;
        };
        let Some(mechanical) = number(&value, "RotatorMechanicalPosition") else {
            continue;
        };
        out.push(LightAngles {
            id,
            rig: rig.clone(),
            at,
            sky: number(&value, "RotatorPosition"),
            mechanical,
        });
    }
    out
}

/// A plate solution's position angle, degrees: the turn of the image's
/// +y axis from north, from its CD matrix.
fn position_angle(cd: &[[f64; 2]; 2]) -> f64 {
    cd[0][1].atan2(cd[1][1]).to_degrees().rem_euclid(360.0)
}

/// PSF Guard's pixel solves of these lights, by id, from the astrometry
/// cache in the catalog's cache folder `cache`: their position angles.
pub(crate) fn solved_angles(cache: &Path, lights: &[LightAngles]) -> HashMap<i64, f64> {
    let mut out = HashMap::new();
    if !cache.join("astrometry").is_dir() {
        return out;
    }
    for light in lights {
        let Ok(id) = i32::try_from(light.id) else {
            continue;
        };
        if let Some(solution) = crate::astrometry::persisted_pixel_analysis(cache, id)
            .and_then(|analysis| analysis.solution)
        {
            out.insert(light.id, position_angle(&solution.wcs.cd));
        }
    }
    out
}

fn median(mut values: Vec<f64>) -> Option<f64> {
    if values.is_empty() {
        return None;
    }
    values.sort_by(f64::total_cmp);
    Some(values[values.len() / 2])
}

/// Each light's sky − mechanical offset, modulo a half turn, by rig: from
/// PSF Guard's solve where the rig's solves agree with N.I.N.A. on a
/// convention, else from N.I.N.A.'s sky angle.
pub(crate) fn epochs(lights: &[LightAngles], solved: &HashMap<i64, f64>) -> AngleEpochs {
    // Per rig, how a solve's angle reads in N.I.N.A.'s terms: a constant
    // turn, with the solve's sense of rotation either way.
    let mut conventions: HashMap<&str, (f64, f64)> = HashMap::new();
    let mut by_rig: HashMap<&str, Vec<&LightAngles>> = HashMap::new();
    for light in lights {
        by_rig.entry(light.rig.as_str()).or_default().push(light);
    }
    for (rig, rig_lights) in &by_rig {
        let both: Vec<(f64, f64)> = rig_lights
            .iter()
            .filter_map(|light| Some((*solved.get(&light.id)?, light.sky?)))
            .collect();
        if both.len() < MIN_CONVENTION_LIGHTS {
            continue;
        }
        let best = [1.0f64, -1.0]
            .into_iter()
            .filter_map(|sense| {
                let first = half(sense * both[0].0 - both[0].1);
                let turns: Vec<f64> = both
                    .iter()
                    .map(|(pa, sky)| first + half(sense * pa - sky - first))
                    .collect();
                let middle = median(turns.clone())?;
                let scatter = median(turns.iter().map(|t| (t - middle).abs()).collect())?;
                Some((sense, middle, scatter))
            })
            .min_by(|a, b| a.2.total_cmp(&b.2));
        if let Some((sense, turn, scatter)) = best
            && scatter <= MAX_CONVENTION_SCATTER_DEG
        {
            conventions.insert(rig, (sense, turn));
        }
    }
    let samples = lights
        .iter()
        .filter_map(|light| {
            let sky = match (conventions.get(light.rig.as_str()), solved.get(&light.id)) {
                (Some((sense, turn)), Some(pa)) => sense * pa - turn,
                _ => light.sky?,
            };
            // Modulo a half turn: the sky angle turns at a meridian flip,
            // and which pier side turns it is the mount's convention.
            Some((
                light.rig.clone(),
                light.at,
                (sky - light.mechanical).rem_euclid(180.0),
            ))
        })
        .collect();
    AngleEpochs::from_samples(samples)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn light(id: i64, at: i64, sky: f64, mechanical: f64, _west: bool) -> LightAngles {
        LightAngles {
            id,
            rig: "rig".into(),
            at,
            sky: Some(sky),
            mechanical,
        }
    }

    #[test]
    fn a_meridian_flip_is_not_a_re_home_and_a_re_home_is() {
        let lights = vec![
            light(1, 0, 180.0, 102.0, false),
            light(2, 100, 0.3, 102.1, true),
            // Re-homed: the same sky angle now reads 20° less.
            light(3, 200, 180.0, 82.0, false),
        ];
        let epochs = epochs(&lights, &HashMap::new());
        assert_eq!(epochs.by_rig["rig"].len(), 2, "{epochs:?}");
        assert!((epochs.shift("rig", 200, 0) - 20.0).abs() < 0.5);
    }

    #[test]
    fn solves_count_once_they_agree_with_the_rig_on_a_convention() {
        // Six lights with both: the solve reads the sky angle mirrored and
        // turned by 90°. The seventh has only a solve, and it says the
        // rotator re-homed though N.I.N.A. did not notice.
        let mut lights: Vec<LightAngles> = (0..6)
            .map(|i| light(i, i * 10, 100.0 + i as f64, 20.0 + i as f64, false))
            .collect();
        let mut solved: HashMap<i64, f64> = (0..6)
            .map(|i| (i, (90.0 - (100.0 + i as f64)).rem_euclid(360.0)))
            .collect();
        lights.push(light(6, 1000, 100.0, 20.0, false));
        solved.insert(6, (90.0 - 115.0f64).rem_euclid(360.0));
        let epochs = epochs(&lights, &solved);
        let rig = &epochs.by_rig["rig"];
        assert_eq!(rig.len(), 2, "{rig:?}");
        assert!((epochs.shift("rig", 1000, 0) - 15.0).abs() < 0.5);
    }
}
