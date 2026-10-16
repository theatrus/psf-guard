//! Each rig's rotator angle epochs, from what its lights recorded and what
//! PSF Guard solved (see [`super::lights`]).
//!
//! N.I.N.A. writes a light's synced sky angle (`RotatorPosition`) and the
//! rotator's mechanical angle (`RotatorMechanicalPosition`) into
//! `acquiredimage.metadata`. Its sky angle comes from its own plate solves
//! while centring. Where PSF Guard has solved a light's pixels (the
//! astrometry cache beside the flat responses) and the file it solved is
//! unchanged, that solve gives the sky angle instead, once put in N.I.N.A.'s
//! convention by the rig's lights that have both.
//!
//! The measuring job works the offsets out ([`gather`]) and stores them
//! (`psf_guard_flat_light_angle`); the book only reads the stored rows
//! ([`epochs`]), so loading it touches neither the metadata of every light
//! nor the astrometry cache.

use super::family_store::LightAngle;
use super::lights::{half, AngleEpochs};
use rusqlite::Connection;
use std::collections::HashMap;
use std::path::Path;

/// Lights with both a PSF Guard solve and N.I.N.A.'s sky angle needed to
/// put the solves in N.I.N.A.'s convention.
const MIN_CONVENTION_LIGHTS: usize = 5;
/// How far those lights may scatter, degrees, for the convention to hold.
const MAX_CONVENTION_SCATTER_DEG: f64 = 1.0;

/// One light's recorded angles.
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

/// A solve file's stamp: its modification time, so an unchanged solve is
/// not read again.
fn stamp(path: &Path) -> Option<String> {
    let modified = std::fs::metadata(path).ok()?.modified().ok()?;
    let since = modified.duration_since(std::time::UNIX_EPOCH).ok()?;
    Some(format!("{}.{:09}", since.as_secs(), since.subsec_nanos()))
}

/// PSF Guard's pixel solves of these lights, by id, from the astrometry
/// cache in the catalog's cache folder `cache`: their position angles and
/// stamps. A solve whose stamp is the one `known` holds is not read again;
/// one whose source file changed since it was solved counts for nothing.
pub(crate) fn solved_angles(
    cache: Option<&Path>,
    lights: &[LightAngles],
    known: &HashMap<i64, LightAngle>,
) -> HashMap<i64, (f64, String)> {
    let mut out = HashMap::new();
    let Some(cache) = cache.filter(|cache| cache.join("astrometry").is_dir()) else {
        return out;
    };
    for light in lights {
        let Ok(id) = i32::try_from(light.id) else {
            continue;
        };
        let Some(stamp) = stamp(&crate::astrometry::persisted_analysis_path(cache, id)) else {
            continue;
        };
        if let Some(row) = known.get(&light.id)
            && row.solve_stamp.as_deref() == Some(stamp.as_str())
        {
            if let Some(pa) = row.solved_pa {
                out.insert(light.id, (pa, stamp));
            }
            continue;
        }
        if let Some(solution) = crate::astrometry::persisted_current_pixel_analysis(cache, id)
            .and_then(|analysis| analysis.solution)
        {
            out.insert(light.id, (position_angle(&solution.wcs.cd), stamp));
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
pub(crate) fn offsets(
    lights: &[LightAngles],
    solved: &HashMap<i64, (f64, String)>,
) -> Vec<LightAngle> {
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
            .filter_map(|light| Some((solved.get(&light.id)?.0, light.sky?)))
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
    lights
        .iter()
        .filter_map(|light| {
            let solve = solved.get(&light.id);
            let (sky, used) = match (conventions.get(light.rig.as_str()), solve) {
                (Some((sense, turn)), Some((pa, _))) => (sense * pa - turn, true),
                _ => (light.sky?, false),
            };
            // Modulo a half turn: the sky angle turns at a meridian flip,
            // and which pier side turns it is the mount's convention.
            Some(LightAngle {
                light_id: light.id,
                rig: light.rig.clone(),
                at: light.at,
                offset: (sky - light.mechanical).rem_euclid(180.0),
                solved: used,
                solved_pa: solve.map(|(pa, _)| *pa),
                solve_stamp: solve.map(|(_, stamp)| stamp.clone()),
            })
        })
        .collect()
}

/// Work every light's offset out and store it: the measuring job's step.
/// Returns how many rows changed.
pub(crate) fn gather(
    conn: &Connection,
    cache: Option<&Path>,
    rig_of: impl Fn(Option<&str>) -> Vec<String>,
) -> anyhow::Result<usize> {
    super::family_store::add_table(conn)?;
    let lights = light_angles(conn, rig_of);
    let known: HashMap<i64, LightAngle> = super::family_store::load_angles(conn)?
        .into_iter()
        .map(|angle| (angle.light_id, angle))
        .collect();
    let solved = solved_angles(cache, &lights, &known);
    super::family_store::store_angles(conn, &offsets(&lights, &solved))
}

/// The epochs the stored offsets make.
pub(crate) fn epochs(angles: &[LightAngle]) -> AngleEpochs {
    AngleEpochs::from_samples(
        angles
            .iter()
            .map(|angle| (angle.rig.clone(), angle.at, angle.offset))
            .collect(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn light(id: i64, at: i64, sky: f64, mechanical: f64) -> LightAngles {
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
            light(1, 0, 180.0, 102.0),
            light(2, 100, 0.3, 102.1),
            // Re-homed: the same sky angle now reads 20° less.
            light(3, 200, 180.0, 82.0),
        ];
        let epochs = epochs(&offsets(&lights, &HashMap::new()));
        assert_eq!(epochs.by_rig["rig"].len(), 2, "{epochs:?}");
        assert!((epochs.shift("rig", 200, 0) - 20.0).abs() < 0.5);
    }

    #[test]
    fn solves_count_once_they_agree_with_the_rig_on_a_convention() {
        // Six lights with both: the solve reads the sky angle mirrored and
        // turned by 90°. The seventh has only a solve, and it says the
        // rotator re-homed though N.I.N.A. did not notice.
        let mut lights: Vec<LightAngles> = (0..6)
            .map(|i| light(i, i * 10, 100.0 + i as f64, 20.0 + i as f64))
            .collect();
        let mut solved: HashMap<i64, (f64, String)> = (0..6)
            .map(|i| {
                (
                    i,
                    ((90.0 - (100.0 + i as f64)).rem_euclid(360.0), "s".into()),
                )
            })
            .collect();
        lights.push(light(6, 1000, 100.0, 20.0));
        solved.insert(6, ((90.0 - 115.0f64).rem_euclid(360.0), "s".into()));
        let offsets = offsets(&lights, &solved);
        assert!(offsets.iter().all(|angle| angle.solved));
        let epochs = epochs(&offsets);
        let rig = &epochs.by_rig["rig"];
        assert_eq!(rig.len(), 2, "{rig:?}");
        assert!((epochs.shift("rig", 1000, 0) - 15.0).abs() < 0.5);
    }

    #[test]
    fn a_solve_is_read_from_the_cache_only_when_it_changed() {
        let temp = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(temp.path().join("astrometry")).unwrap();
        // A file that is not a solve: read, it counts for nothing.
        let path = crate::astrometry::persisted_analysis_path(temp.path(), 7);
        std::fs::write(&path, "{}").unwrap();
        let lights = vec![light(7, 0, 100.0, 20.0)];
        assert!(solved_angles(Some(temp.path()), &lights, &HashMap::new()).is_empty());
        // Known under its stamp: the stored angle stands, nothing is read.
        let known: HashMap<i64, LightAngle> = [(
            7,
            LightAngle {
                light_id: 7,
                rig: "rig".into(),
                at: 0,
                offset: 80.0,
                solved: true,
                solved_pa: Some(12.5),
                solve_stamp: stamp(&path),
            },
        )]
        .into();
        let solved = solved_angles(Some(temp.path()), &lights, &known);
        assert_eq!(solved[&7].0, 12.5);
        // No cache folder: nothing at all.
        assert!(solved_angles(None, &lights, &known).is_empty());
    }
}
