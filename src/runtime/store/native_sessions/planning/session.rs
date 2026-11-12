//! Per-session observations; the aggregate owns bounded report accumulation.
use super::{Binding, Provider, api, files};

pub(super) enum Disposition {
    Retained,
    Absent,
    Queued,
}
pub(super) enum Preview {
    NotRequested,
    Full,
    Entry(api::NativeSessionPreview),
}
pub(super) struct Outcome {
    pub disposition: Disposition,
    pub warning: Option<api::NativeWarningReason>,
    pub preview: Preview,
}
impl Outcome {
    pub fn retained(warning: Option<api::NativeWarningReason>) -> Self {
        Self {
            disposition: Disposition::Retained,
            warning,
            preview: Preview::NotRequested,
        }
    }
}
impl api::NativeCleanup {
    pub(super) fn record_session(&mut self, agent: &str, outcome: Outcome) {
        match outcome.preview {
            Preview::NotRequested => (),
            Preview::Full => self.native_preview_truncated = true,
            Preview::Entry(preview) => self.native_preview.push(preview),
        }
        match outcome.disposition {
            Disposition::Retained => self.native_sessions_retained += 1,
            Disposition::Absent => self.native_sessions_absent += 1,
            Disposition::Queued => (),
        }
        if let Some(warning) = outcome.warning {
            self.warn(agent, warning);
        }
    }
}
pub(super) fn preview(
    binding: &Binding,
    provider: Provider,
    generation: uuid::Uuid,
    files: &[files::File],
    full: bool,
) -> Preview {
    if full {
        return Preview::Full;
    }
    Preview::Entry(api::NativeSessionPreview {
        agent_id: binding.agent.clone(),
        provider,
        generation,
        thread_id: binding.thread.clone(),
        files: files
            .iter()
            .take(api::NATIVE_PREVIEW_LIMIT)
            .map(|file| api::NativeFilePreview {
                path: file.path.clone(),
                bytes: file.bytes(),
            })
            .collect(),
        bytes: files.iter().map(files::File::bytes).sum(),
        files_truncated: files.len() > api::NATIVE_PREVIEW_LIMIT,
    })
}

#[cfg(test)]
mod tests;
