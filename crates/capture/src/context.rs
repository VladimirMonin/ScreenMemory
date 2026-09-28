//! Capture context assembly: device, confirmed application, observed title and geometry.
//!
//! Missing information becomes an explicit `unknown` value plus a warning; it is never
//! substituted with the last known application (S02 §1.3, §6.3).
use crate::{geometry::DisplayTopology, window::WindowObservation, UNKNOWN_TEXT};
use sm_application::storage::{CaptureContext, CaptureTarget, PhysicalBounds, MAX_TITLE_LENGTH};

/// One stored field plus whether it had to be shortened.
fn clamp(raw: &str) -> (String, bool) {
    if raw.chars().count() <= MAX_TITLE_LENGTH {
        return (raw.to_string(), false);
    }
    (raw.chars().take(MAX_TITLE_LENGTH).collect(), true)
}

/// Stored context of one accepted capture.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BuiltContext {
    pub context: CaptureContext,
    pub warnings: Vec<String>,
}

/// Assemble the stored context of an accepted frame.
pub fn build(
    topology: &DisplayTopology,
    target: CaptureTarget,
    bounds: PhysicalBounds,
    observation: Option<&WindowObservation>,
) -> BuiltContext {
    let mut warnings = Vec::new();
    let (device, _dpi) = topology.device_for_rect(bounds);
    if device == UNKNOWN_TEXT {
        warnings.push("display_unknown".to_string());
    }
    let app_source = observation
        .map(|value| value.executable.as_str())
        .unwrap_or(UNKNOWN_TEXT);
    let title_source = observation
        .map(|value| value.title.as_str())
        .unwrap_or(UNKNOWN_TEXT);
    if app_source == UNKNOWN_TEXT {
        warnings.push("application_unknown".to_string());
    }
    if title_source == UNKNOWN_TEXT {
        warnings.push("title_unknown".to_string());
    }
    let (device, device_clamped) = clamp(&device);
    let (app, app_clamped) = clamp(app_source);
    let (title, title_clamped) = clamp(title_source);
    if device_clamped || app_clamped || title_clamped {
        warnings.push("context_truncated".to_string());
    }
    if let Some(observation) = observation {
        warnings.extend(observation.warnings.iter().cloned());
        if !observation.dwm_bounds {
            warnings.push("geometry_fallback_getwindowrect".to_string());
        }
    }
    warnings.sort();
    warnings.dedup();
    BuiltContext {
        context: CaptureContext {
            device,
            app,
            title,
            target,
            sources: vec![target],
            bounds,
        },
        warnings,
    }
}
