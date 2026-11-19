use crate::{CoreError, CoreResult, JobState, Store};

#[derive(Clone, Copy, Default)]
pub(crate) struct JobExecution<'a>(Option<(&'a Store, &'a str)>);

impl<'a> JobExecution<'a> {
    pub(crate) fn for_job(store: &'a Store, id: &'a str) -> Self {
        Self(Some((store, id)))
    }

    pub(crate) fn is_tracked(self) -> bool {
        self.0.is_some()
    }

    pub(crate) fn checkpoint(self) -> CoreResult<()> {
        if let Some((store, id)) = self.0 {
            if store.job_cancellation_requested(id)? {
                return Err(CoreError::Cancelled);
            }
        }
        Ok(())
    }

    pub(crate) fn stage(self, stage: &str) -> CoreResult<()> {
        self.checkpoint()?;
        if let Some((store, id)) = self.0 {
            store.update_running_job_stage(id, stage)?;
        }
        Ok(())
    }

    pub(crate) fn begin_commit(self) -> CoreResult<()> {
        if let Some((store, id)) = self.0 {
            if let Err(error) = store.update_job(id, JobState::Validating, 90, None, None) {
                self.checkpoint()?;
                return Err(error);
            }
        }
        Ok(())
    }
}
