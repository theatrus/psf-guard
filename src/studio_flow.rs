//! A PSF-Studio document from PSF Guard's color processing settings.
//!
//! [`crate::studio::document_from_color_setup`] works on the settings as
//! JSON. This module is the typed side: it takes and returns
//! [`StackColorProcessing`], so a document made here gives the color pipeline
//! exactly the settings a saved setup would.

use crate::processing_setups::{ProcessingSetupKind, ProcessingSetupRecord};
use crate::server::stack_preview::color::{StackColorProcessing, StackColorRole};
use crate::studio::{color_setup_from_document, document_from_color_setup, Document, StudioError};

fn refuse(reason: impl std::fmt::Display) -> StudioError {
    StudioError::Document(reason.to_string())
}

/// The name a role has in saved settings, such as `ha`.
fn role_name(role: StackColorRole) -> Result<String, StudioError> {
    match serde_json::to_value(role).map_err(refuse)? {
        serde_json::Value::String(name) => Ok(name),
        other => Err(refuse(format!(
            "a channel role is saved as text, not {other}"
        ))),
    }
}

/// A document that applies `processing` to the channels of one color card.
/// `channels` stack bottom first.
pub fn document_from_processing(
    name: &str,
    processing: &StackColorProcessing,
    channels: &[StackColorRole],
) -> Result<Document, StudioError> {
    let settings = serde_json::to_value(processing).map_err(refuse)?;
    let names = channels
        .iter()
        .map(|role| role_name(*role))
        .collect::<Result<Vec<_>, _>>()?;
    let names: Vec<&str> = names.iter().map(String::as_str).collect();
    document_from_color_setup(name, &settings, &names)
}

/// A document from a saved Color processing setup.
pub fn document_from_setup(
    setup: &ProcessingSetupRecord,
    channels: &[StackColorRole],
) -> Result<Document, StudioError> {
    if setup.kind != ProcessingSetupKind::Color {
        return Err(refuse(format!(
            "{} is a {} setup; a document starts from a Color setup",
            setup.name,
            setup.kind.label()
        )));
    }
    // Reading the settings as PSF Guard's type first refuses settings the
    // color pipeline would refuse.
    let processing: StackColorProcessing =
        serde_json::from_value(setup.settings.clone()).map_err(refuse)?;
    document_from_processing(&setup.name, &processing, channels)
}

/// The color processing settings a document's steps hold.
pub fn processing_from_document(document: &Document) -> Result<StackColorProcessing, StudioError> {
    serde_json::from_value(color_setup_from_document(document)?).map_err(refuse)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::processing_setups::DEFAULT_COLOR_SETTINGS;
    use crate::studio::{flow_operations as op, StageKind};
    use serde_json::{json, Value};

    const SHO: [StackColorRole; 3] = [
        StackColorRole::Sii,
        StackColorRole::Ha,
        StackColorRole::Oiii,
    ];

    fn default_processing() -> StackColorProcessing {
        serde_json::from_str(DEFAULT_COLOR_SETTINGS).unwrap()
    }

    fn as_json(processing: &StackColorProcessing) -> Value {
        serde_json::to_value(processing).unwrap()
    }

    #[test]
    fn the_default_setup_gives_the_pipeline_the_same_settings() {
        let processing = default_processing();
        let document = document_from_processing("Default", &processing, &SHO).unwrap();
        let back = processing_from_document(&document).unwrap();
        assert_eq!(as_json(&back), as_json(&processing));
    }

    /// Every field of the typed settings must be one the mapping places in a
    /// stage. A field added to `StackColorProcessing` later fails here until
    /// the mapping gives it a step.
    #[test]
    fn no_typed_setting_lands_in_other_settings() {
        let mut processing = default_processing();
        processing.emission = Some(Default::default());
        processing.narrowband = Some(Default::default());
        // A chain with no steps is enough to carry the setting through.
        let chain = || serde_json::from_value(json!({ "steps": [] })).unwrap();
        processing.color_rc_astro = Some(chain());
        processing
            .input_rc_astro
            .insert(StackColorRole::Ha, chain());
        processing.input_deconvolutions.insert(
            StackColorRole::Oiii,
            seiza_deconvolution::DeconvolutionConfig::conservative(3.0),
        );
        processing.output_stretches = processing.input_stretches[&StackColorRole::Ha].clone();

        let document = document_from_processing("All", &processing, &SHO).unwrap();
        let other = document
            .stages()
            .iter()
            .flat_map(|stage| &stage.steps)
            .filter(|step| step.kind.name() == op::OTHER)
            .count();
        assert_eq!(other, 0);
        assert_eq!(document.stage(StageKind::Finish).steps.len(), 1);
        let back = processing_from_document(&document).unwrap();
        assert_eq!(as_json(&back), as_json(&processing));
    }

    #[test]
    fn a_view_setup_is_refused() {
        let setup = ProcessingSetupRecord {
            name: "Sharp".into(),
            kind: ProcessingSetupKind::View,
            settings: json!({}),
            created_unix_seconds: 0,
            updated_unix_seconds: 0,
            other_fields: Default::default(),
        };
        assert!(matches!(
            document_from_setup(&setup, &SHO),
            Err(StudioError::Document(_))
        ));
    }

    #[test]
    fn settings_the_pipeline_refuses_are_refused() {
        let setup = ProcessingSetupRecord {
            name: "Broken".into(),
            kind: ProcessingSetupKind::Color,
            settings: json!({ "input_stretches": { "not_a_channel": [] } }),
            created_unix_seconds: 0,
            updated_unix_seconds: 0,
            other_fields: Default::default(),
        };
        assert!(document_from_setup(&setup, &SHO).is_err());
    }
}
