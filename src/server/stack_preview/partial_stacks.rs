//! A channel stacked as partial stacks, one per calibration session, with
//! Seiza's distributed stacking (`seiza-pipeline`).
//!
//! Each session is stacked on its own, against the channel's reference
//! frame calibrated by its own session, so every session's frames land on
//! one grid, scale and weight. Its partial stack is kept under a key Seiza
//! computes from the contents of its frames and masters and the options, so
//! a rebuild after another night stacks only that night. The channel is the
//! sessions' stacks combined by their weights, which is what one stack of
//! every frame gives, apart from rejection: it runs within each session.
//!
//! The SNR curve and the orientation vote follow the frames of one
//! accumulator, so a channel stacked this way has neither: it publishes in
//! the reference frame's rotation, or north up.

use std::path::{Path, PathBuf};

use seiza_pipeline::{
    ChannelReference, PartialStack, Reintegration, Session, SessionOptions, SessionStep, Store,
};
use serde::{Deserialize, Serialize};

use super::integrate::{
    ChannelEvents, ChannelOutcome, ChannelOutputs, Integrated, Orientation, RestoredFrames,
};
use super::*;

/// Everything a channel stacked as partial stacks needs.
pub(super) struct SessionWork<'a> {
    /// For the log.
    pub job_id: &'a str,
    pub group: &'a PreparedGroup,
    pub plan: &'a crate::calibration::CalibrationPlan,
    pub method: StackMethod,
    pub gpu: StackGpu,
    pub order: snr::StackFrameOrder,
    pub orientation: Orientation,
    pub outputs: ChannelOutputs,
    /// Where partial stacks are kept, shared by every channel.
    pub store: PathBuf,
    pub pool: &'a rayon::ThreadPool,
    pub cancel: &'a Stop,
}

/// Where every channel's partial stacks are kept, below the stack root.
pub(super) fn store_root(stack_root: &Path) -> PathBuf {
    crate::server::storage::stack_folder(
        stack_root,
        crate::server::storage::stack_kind::PARTIAL_STACKS,
    )
}

/// A light a session's stack turned away, kept beside its partial stack so
/// a rebuild that reuses the stack can list it again.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
struct TurnedAway {
    path: PathBuf,
    reason: String,
}

/// Where the lights a session's stack with `key` turned away are listed.
fn turned_away_path(store: &Path, key: &seiza_pipeline::Digest) -> PathBuf {
    store.join("turned-away").join(format!("{key}.json"))
}

fn write_turned_away(path: &Path, lights: &[TurnedAway]) -> Result<(), String> {
    let parent = path
        .parent()
        .ok_or_else(|| "turned-away list has no folder".to_string())?;
    std::fs::create_dir_all(parent).map_err(|error| error.to_string())?;
    let temporary = path.with_extension(format!("{}.tmp", temporary_tag()));
    let json = serde_json::to_vec(lights).map_err(|error| error.to_string())?;
    std::fs::write(&temporary, json).map_err(|error| error.to_string())?;
    std::fs::rename(&temporary, path).map_err(|error| error.to_string())
}

fn read_turned_away(path: &Path) -> Option<Vec<TurnedAway>> {
    serde_json::from_slice(&std::fs::read(path).ok()?).ok()
}

/// A session's recipe as Seiza's file calibration session takes it: the
/// same masters, gate and pedestal, and the 0..1 declared range rescaled to
/// 65535 ADU, as PSF Guard's frame reader does.
pub(super) fn file_session(
    session: &crate::calibration::CalibrationSession,
) -> seiza_stacking::FileCalibrationSession {
    let mut file = seiza_stacking::FileCalibrationSession {
        normalized_full_scale: Some(65535.0),
        tolerances: session.match_tolerances,
        ..Default::default()
    };
    match &session.recipe {
        crate::calibration::MasterRecipe::None => {}
        crate::calibration::MasterRecipe::Files {
            bias,
            dark,
            flat,
            light_exposure_s,
        } => {
            file.bias = bias.clone();
            file.flat = flat.clone();
            // The dark scales to the light's exposure; without a dark there
            // is nothing to scale.
            file.dark_exposure_seconds = dark.as_ref().and(*light_exposure_s);
            file.dark = dark.clone();
        }
        crate::calibration::MasterRecipe::PedestalFlat { flat, pedestal } => {
            file.flat = Some(flat.clone());
            file.pedestal = Some(*pedestal);
        }
    }
    file
}

/// Stack a channel as partial stacks, one per calibration session, and
/// publish them combined as the channel's stack. Blocking; runs Seiza on
/// `work.pool`.
pub(super) fn integrate_by_sessions(
    work: SessionWork<'_>,
    events: &mut (dyn ChannelEvents + Send),
) -> Result<ChannelOutcome, String> {
    let SessionWork {
        job_id,
        group,
        plan,
        method,
        gpu,
        order,
        orientation,
        outputs,
        store,
        pool,
        cancel,
    } = work;
    let reference_path = &group
        .frames
        .first()
        .ok_or_else(|| "Stack group has no input frames".to_string())?
        .path;
    let reference_frame =
        crate::image_io::open_linear_frame(reference_path).map_err(|error| error.to_string())?;
    let output_channels = if reference_frame.bayer.is_some() {
        3
    } else {
        reference_frame.image.channels
    };
    drop(reference_frame);
    events.frames_restored(RestoredFrames {
        frames: Vec::new(),
        accepted: 0,
        rejected: 0,
        reused: 0,
        output_channels,
        total_exposure_seconds: 0.0,
        snr: None,
    });
    let recipes = plan.sessions.iter().map(file_session).collect::<Vec<_>>();
    let reference = ChannelReference {
        image: reference_path.clone(),
        light: reference_path.clone(),
        calibration: recipes[plan.assignments[0]].clone(),
    };
    // Each session's frames, in the group's order.
    let mut members = vec![Vec::new(); plan.sessions.len()];
    for (frame, &session) in group.frames.iter().zip(&plan.assignments) {
        members[session].push(frame);
    }
    let cosmetics = SessionCosmetics::for_plan(plan, group.calibration);
    let store_handle = Store::new(&store);
    // Seiza needs a scratch folder it is given to exist.
    if method.final_pass == StackFinalPass::Reintegrate {
        std::fs::create_dir_all(&outputs.scratch).map_err(|error| error.to_string())?;
    }
    let sessions = members
        .iter()
        .enumerate()
        .filter(|(_, frames)| !frames.is_empty())
        .collect::<Vec<_>>();
    let mut stored = Vec::new();
    for (position, &(session, frames)) in sessions.iter().enumerate() {
        if cancel.requested() {
            return Ok(ChannelOutcome::Cancelled);
        }
        events.phase(
            format!("Stacking session {}/{}", position + 1, sessions.len()),
            None,
        );
        let options = SessionOptions {
            stack: method.stack_options(cosmetics.for_frame(session, false), gpu),
            reintegration: (method.final_pass == StackFinalPass::Reintegrate).then(|| {
                Reintegration {
                    scratch_directory: Some(outputs.scratch.clone()),
                    ..Reintegration::default()
                }
            }),
            ..SessionOptions::default()
        };
        let lights = Session {
            lights: frames.iter().map(|frame| frame.path.clone()).collect(),
            calibration: recipes[session].clone(),
        };
        let outcome = pool
            .install(|| {
                SessionStep::plan(&reference, &lights, &options)
                    .and_then(|step| step.run_into(&store_handle))
            })
            .map_err(|error| format!("Stacking session {}: {error}", position + 1))?;
        let listed = turned_away_path(&store, &outcome.key);
        let turned_away = if outcome.reused {
            read_turned_away(&listed)
        } else {
            let lights = outcome
                .rejected
                .iter()
                .map(|(path, reason)| TurnedAway {
                    path: path.clone(),
                    reason: reason.to_string(),
                })
                .collect::<Vec<_>>();
            write_turned_away(&listed, &lights)?;
            Some(lights)
        };
        if outcome.reused {
            events.resume_note(&format!(
                "Session {} reused from an earlier build",
                position + 1
            ));
        }
        for frame in frames {
            let reason = turned_away
                .iter()
                .flatten()
                .find(|light| light.path == frame.path)
                .map(|light| light.reason.clone());
            let decision = match reason {
                Some(reason) => rejected_decision(frame, reason),
                None if outcome.path.is_none() => {
                    rejected_decision(frame, "no light of its session was accepted".into())
                }
                None => StackFrameDecision {
                    disposition: "accepted".into(),
                    reason: None,
                    ..rejected_decision(frame, String::new())
                },
            };
            events.frame(decision, frame.exposure_seconds);
        }
        if let Some(path) = outcome.path {
            stored.push(path);
        }
    }
    if stored.is_empty() {
        return Err("No session of the group accepted a light".into());
    }
    if cancel.requested() {
        return Ok(ChannelOutcome::Cancelled);
    }
    events.phase(format!("Combining {} partial stacks", stored.len()), None);
    let tile = pool
        .install(|| PartialStack::combine_files(&stored))
        .map_err(|error| format!("Combining partial stacks: {error}"))?;
    tracing::info!(
        job_id,
        group_index = group.index,
        sessions = stored.len(),
        frames = tile.identity.combined,
        "Combined partial stacks"
    );
    let accepted_frames = tile.identity.combined;
    let rejected_frames = (group.frames.len() as u32).saturating_sub(accepted_frames);
    pool.install(|| {
        super::integrate::publish(
            job_id,
            group.index,
            Integrated {
                image: tile.mean,
                clipped: None,
                reference_headers: tile.headers,
                accepted_frames,
                rejected_frames,
                progressive: snr::ProgressiveSnr::for_group(order, Vec::new(), &[], false),
                orientation_vote: OrientationVote::default(),
            },
            orientation,
            gpu,
            &outputs,
            events,
        )
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_recipe_becomes_the_same_file_session() {
        let tolerances = seiza_calibration::MatchTolerances::default();
        let session = |recipe| crate::calibration::CalibrationSession {
            masters: seiza_stacking::CalibrationMasters::default(),
            recipe,
            applied: Default::default(),
            match_tolerances: tolerances,
        };
        let files = file_session(&session(crate::calibration::MasterRecipe::Files {
            bias: Some("bias.fits".into()),
            dark: Some("dark.fits".into()),
            flat: Some("flat.fits".into()),
            light_exposure_s: Some(300.0),
        }));
        assert_eq!(files.bias.as_deref(), Some(Path::new("bias.fits")));
        assert_eq!(files.dark.as_deref(), Some(Path::new("dark.fits")));
        assert_eq!(files.flat.as_deref(), Some(Path::new("flat.fits")));
        assert_eq!(files.dark_exposure_seconds, Some(300.0));
        assert_eq!(files.normalized_full_scale, Some(65535.0));
        // Without a dark, there is no exposure to scale one to.
        let darkless = file_session(&session(crate::calibration::MasterRecipe::Files {
            bias: Some("bias.fits".into()),
            dark: None,
            flat: None,
            light_exposure_s: Some(300.0),
        }));
        assert_eq!(darkless.dark_exposure_seconds, None);
        let pedestal = file_session(&session(crate::calibration::MasterRecipe::PedestalFlat {
            flat: "flat.fits".into(),
            pedestal: 512.0,
        }));
        assert_eq!(pedestal.flat.as_deref(), Some(Path::new("flat.fits")));
        assert_eq!(pedestal.pedestal, Some(512.0));
        assert!(pedestal.bias.is_none() && pedestal.dark.is_none());
        let raw = file_session(&session(crate::calibration::MasterRecipe::None));
        assert!(raw.bias.is_none() && raw.dark.is_none() && raw.flat.is_none());
    }

    /// A channel stacked as partial stacks takes every frame, a rebuild
    /// reuses every session from the store to the same pixels, and the
    /// result sits where the single pass's does.
    #[test]
    fn a_channel_stacks_by_session_and_a_rebuild_reuses_them() {
        use super::super::remote_channel::testing;
        let directory = tempfile::tempdir().unwrap();
        let channel = testing::synthetic_group(directory.path());
        let store = directory.path().join("store");
        let build = |name: &str| {
            let output = directory.path().join(name);
            std::fs::create_dir_all(&output).unwrap();
            output
        };
        let first = testing::stack_in_sessions(&channel, &build("first"), &store, 2);
        assert_eq!(first.dispositions, vec!["accepted"; 5]);
        assert!(!first.notes.iter().any(|note| note.contains("reused")));

        let again = testing::stack_in_sessions(&channel, &build("again"), &store, 2);
        let reused = again
            .notes
            .iter()
            .filter(|note| note.contains("reused"))
            .count();
        assert_eq!(reused, 2);
        assert_eq!(again.dispositions, first.dispositions);
        assert_eq!(again.pixels, first.pixels);

        let whole = testing::stack_here(&channel, &build("whole"));
        let median = |bits: &[u32]| {
            let mut values = bits
                .iter()
                .map(|&bits| f32::from_bits(bits))
                .filter(|value| value.is_finite())
                .collect::<Vec<_>>();
            values.sort_by(f32::total_cmp);
            values[values.len() / 2]
        };
        let (sessions, single) = (median(&first.pixels), median(&whole.pixels));
        assert!(
            (sessions - single).abs() < 0.01 * single,
            "{sessions} vs {single}"
        );
    }

    #[test]
    fn turned_away_lights_round_trip() {
        let directory = tempfile::tempdir().unwrap();
        let key = seiza_pipeline::Digest::of_bytes(b"session");
        let path = turned_away_path(directory.path(), &key);
        assert_eq!(read_turned_away(&path), None);
        let lights = vec![TurnedAway {
            path: "light-3.fits".into(),
            reason: "too few stars".into(),
        }];
        write_turned_away(&path, &lights).unwrap();
        assert_eq!(read_turned_away(&path), Some(lights));
    }
}
