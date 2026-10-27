//! Catalog-to-core adapters. No matching, grading, or file access lives here.
use super::{CalibrationFrame, CalibrationKind, ValidDirection};
use crate::commands::import::headers::FrameMeta;
use seiza_calibration::library as core;

pub(super) fn frame(frame: &CalibrationFrame) -> core::CalibrationFrame {
    core::CalibrationFrame {
        id: frame.id,
        frame_uuid: frame.frame_uuid.clone(),
        rig_uuid: frame.rig_uuid.clone(),
        kind: match frame.kind {
            CalibrationKind::Bias => core::CalibrationKind::Bias,
            CalibrationKind::Dark => core::CalibrationKind::Dark,
            CalibrationKind::DarkFlat => core::CalibrationKind::DarkFlat,
            CalibrationKind::Flat => core::CalibrationKind::Flat,
        },
        source_path: frame.source_path.clone(),
        source_fingerprint: frame.source_fingerprint.clone(),
        captured_at: frame.captured_at,
        telescope: frame.telescope.clone(),
        camera: frame.camera.clone(),
        width: frame.width,
        height: frame.height,
        channels: frame.channels,
        binning_x: frame.binning_x,
        binning_y: frame.binning_y,
        gain: frame.gain,
        offset: frame.offset,
        readout_mode: frame.readout_mode,
        readout_mode_name: frame.readout_mode_name.clone(),
        is_master: frame.is_master,
        bayer_pattern: frame.bayer_pattern.clone(),
        exposure_s: frame.exposure_s,
        camera_temp: frame.camera_temp,
        filter: frame.filter.clone(),
        focal_length_mm: frame.focal_length_mm,
        rotation: frame.rotation,
        valid_direction: frame.valid_direction.map(|v| match v {
            ValidDirection::Forward => core::ValidDirection::Forward,
            ValidDirection::Backward => core::ValidDirection::Backward,
        }),
        kept_out: frame.kept_out.clone(),
        source_verified: frame.source_verified,
    }
}
pub(super) fn light(light: &FrameMeta) -> core::Light {
    core::Light {
        path: light.path.clone(),
        rig_uuid: super::rules::light_rig_uuid(light),
        camera: light.camera.clone(),
        telescope: light.telescope.clone(),
        bayer_pattern: light.bayer_pattern.clone(),
        filter: light.filter.clone(),
        width: light.width,
        height: light.height,
        channels: light.channels,
        binning_x: light.binning_x,
        binning_y: light.binning_y,
        gain: light.gain,
        offset: light.offset,
        readout_mode: light.readout_mode,
        readout_mode_name: light.readout_mode_name.clone(),
        focal_length_mm: light.focal_length_mm,
        rotator_position: light.rotator_position,
        exposure_s: light.exposure_s,
        camera_temp: light.camera_temp,
        timestamp: light.timestamp,
    }
}
