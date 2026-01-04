//! Audio output control (MPD only)
//!
//! Enable, disable, and manage audio outputs.

use anyhow::Result;

use crate::{backends::mpd::specific::Outputs, mpd::commands::Output};

/// Controls audio outputs (MPD only)
///
/// This controller is only available when the backend supports `MpdOutputs`
/// capability. Use `dispatcher.outputs_control()` to get an instance - it
/// returns `None` for backends that don't support output control (like
/// YouTube).
///
/// # Example
///
/// ```ignore
/// if let Some(mut outputs) = dispatcher.outputs_control() {
///     // List all outputs
///     for output in outputs.list()? {
///         println!("{}: {}", output.id, output.name);
///     }
///     
///     // Toggle an output
///     outputs.toggle(0)?;
/// }
/// ```
pub struct OutputController<'a> {
    pub(crate) backend: &'a mut dyn Outputs,
}

impl OutputController<'_> {
    /// List all audio outputs
    pub fn list(&mut self) -> Result<Vec<Output>> {
        self.backend.list()
    }

    /// Enable an output by ID
    pub fn enable(&mut self, id: u32) -> Result<()> {
        self.backend.enable(id)
    }

    /// Disable an output by ID
    pub fn disable(&mut self, id: u32) -> Result<()> {
        self.backend.disable(id)
    }

    /// Toggle an output by ID
    pub fn toggle(&mut self, id: u32) -> Result<()> {
        self.backend.toggle(id)
    }
}
