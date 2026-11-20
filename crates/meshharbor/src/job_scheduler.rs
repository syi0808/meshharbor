use std::{
    collections::VecDeque,
    sync::{Arc, Mutex},
};

use meshharbor_core::{CoreError, Job, JobState, Store, is_generation_operation};
use serde_json::Value;
use tokio::sync::{OwnedSemaphorePermit, Semaphore, TryAcquireError, watch};

struct PendingJob {
    id: String,
    generation: bool,
}

pub(crate) struct JobScheduler {
    slot: Arc<Semaphore>,
    pending: Mutex<VecDeque<PendingJob>>,
    changes: watch::Sender<()>,
}

impl Default for JobScheduler {
    fn default() -> Self {
        Self {
            slot: Arc::new(Semaphore::new(1)),
            pending: Mutex::new(VecDeque::new()),
            changes: watch::channel(()).0,
        }
    }
}

impl JobScheduler {
    pub(crate) fn submit(
        &self,
        store: &Store,
        operation: &str,
        asset_id: Option<&str>,
        parameters: &Value,
    ) -> Result<(Job, Option<OwnedSemaphorePermit>), CoreError> {
        let mut pending = self.pending.lock().expect("job queue mutex poisoned");
        if pending.is_empty()
            && let Ok(permit) = self.slot.clone().try_acquire_owned()
        {
            return Ok((
                store.create_running_job(operation, asset_id, parameters)?,
                Some(permit),
            ));
        }
        let job = store.create_job(operation, asset_id, parameters)?;
        pending.push_back(PendingJob {
            id: job.id.clone(),
            generation: is_generation_operation(operation),
        });
        self.changes.send_replace(());
        Ok((job, None))
    }

    pub(crate) async fn start(
        &self,
        store: &Store,
        id: &str,
        reserved: Option<OwnedSemaphorePermit>,
    ) -> Option<OwnedSemaphorePermit> {
        if reserved.is_some() {
            return reserved;
        }
        let permit = self.acquire(id).await?;
        store
            .update_job(id, JobState::Running, 5, None, None)
            .ok()?;
        Some(permit)
    }

    #[cfg(test)]
    pub(crate) fn enqueue(&self, id: &str, operation: &str) {
        self.pending
            .lock()
            .expect("job queue mutex poisoned")
            .push_back(PendingJob {
                id: id.into(),
                generation: is_generation_operation(operation),
            });
        self.changes.send_replace(());
    }

    pub(crate) fn remove(&self, id: &str) {
        self.pending
            .lock()
            .expect("job queue mutex poisoned")
            .retain(|item| item.id != id);
        self.changes.send_replace(());
    }

    pub(crate) async fn acquire(&self, id: &str) -> Option<OwnedSemaphorePermit> {
        let mut changes = self.changes.subscribe();
        loop {
            let position = self
                .pending
                .lock()
                .expect("job queue mutex poisoned")
                .iter()
                .position(|item| item.id == id)?;
            if position != 0 {
                changes.changed().await.ok()?;
                continue;
            }
            let permit = tokio::select! {
                _ = changes.changed() => continue,
                permit = self.slot.clone().acquire_owned() => permit.ok()?,
            };
            let mut pending = self.pending.lock().expect("job queue mutex poisoned");
            if pending.front().is_some_and(|item| item.id == id) {
                pending.pop_front();
                self.changes.send_replace(());
                return Some(permit);
            }
        }
    }

    pub(crate) fn try_acquire_owned(&self) -> Result<OwnedSemaphorePermit, TryAcquireError> {
        let pending = self.pending.lock().expect("job queue mutex poisoned");
        if !pending.is_empty() {
            return Err(TryAcquireError::NoPermits);
        }
        self.slot.clone().try_acquire_owned()
    }

    pub(crate) fn generation_queue(&self) -> Vec<String> {
        let pending = self.pending.lock().expect("job queue mutex poisoned");
        generation_ids(&pending)
    }

    pub(crate) fn swap_generation(&self, id: &str, other: &str) -> Result<Vec<String>, CoreError> {
        let mut pending = self.pending.lock().expect("job queue mutex poisoned");
        let mut previous = None;
        for (index, item) in pending
            .iter()
            .enumerate()
            .filter(|(_, item)| item.generation)
        {
            if let Some((previous_index, previous_id)) = previous
                && ((item.id == id && previous_id == other)
                    || (item.id == other && previous_id == id))
            {
                pending.swap(previous_index, index);
                self.changes.send_replace(());
                return Ok(generation_ids(&pending));
            }
            previous = Some((index, item.id.as_str()));
        }
        Err(CoreError::Conflict(
            "The generation queue changed. Refresh and try again.".into(),
        ))
    }
}

fn generation_ids(pending: &VecDeque<PendingJob>) -> Vec<String> {
    pending
        .iter()
        .filter(|item| item.generation)
        .map(|item| item.id.clone())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn only_jobs_that_wait_for_the_slot_are_queued() {
        let root =
            std::env::temp_dir().join(format!("meshharbor-scheduler-{}", uuid::Uuid::new_v4()));
        let store = Store::open(&root).unwrap();
        let scheduler = JobScheduler::default();
        let submit = || {
            scheduler
                .submit(&store, "model.generate", None, &serde_json::json!({}))
                .unwrap()
        };
        let (first, first_slot) = submit();
        assert_eq!(first.state, JobState::Running);
        assert!(first_slot.is_some());
        assert!(scheduler.generation_queue().is_empty());
        let (second, second_slot) = submit();
        assert_eq!(second.state, JobState::Queued);
        assert!(second.started_at.is_none());
        assert!(second_slot.is_none());
        drop(first_slot);
        let (third, third_slot) = submit();
        assert_eq!(third.state, JobState::Queued);
        assert!(third_slot.is_none());
        assert_eq!(
            scheduler.generation_queue(),
            [second.id.clone(), third.id.clone()]
        );
        let second_permit = scheduler
            .start(&store, &second.id, second_slot)
            .await
            .unwrap();
        let states = store
            .job_steps(&second.id)
            .unwrap()
            .into_iter()
            .map(|step| step.state)
            .collect::<Vec<_>>();
        assert_eq!(states, [JobState::Queued, JobState::Running]);
        drop(second_permit);
        drop(store);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[tokio::test]
    async fn reordering_changes_worker_order_without_moving_other_operations() {
        let scheduler = Arc::new(JobScheduler::default());
        let busy = scheduler.try_acquire_owned().unwrap();
        scheduler.enqueue("first", "image.generate");
        scheduler.enqueue("mesh", "mesh.cleanup");
        scheduler.enqueue("second", "model.generate");
        let claim = |id| {
            let scheduler = scheduler.clone();
            tokio::spawn(async move { scheduler.acquire(id).await.unwrap() })
        };
        let first = claim("first");
        let mesh = claim("mesh");
        let second = claim("second");
        tokio::task::yield_now().await;
        assert!(scheduler.swap_generation("first", "mesh").is_err());
        assert_eq!(
            scheduler.swap_generation("first", "second").unwrap(),
            ["second", "first"]
        );
        drop(busy);
        let permit = tokio::time::timeout(std::time::Duration::from_secs(2), second)
            .await
            .unwrap()
            .unwrap();
        assert!(scheduler.swap_generation("first", "second").is_err());
        assert!(!first.is_finished());
        drop(permit);
        let permit = tokio::time::timeout(std::time::Duration::from_secs(2), mesh)
            .await
            .unwrap()
            .unwrap();
        assert!(!first.is_finished());
        drop(permit);
        drop(
            tokio::time::timeout(std::time::Duration::from_secs(2), first)
                .await
                .unwrap()
                .unwrap(),
        );
        assert!(scheduler.generation_queue().is_empty());
    }

    #[tokio::test]
    async fn removing_a_waiting_job_releases_the_next_job() {
        let scheduler = Arc::new(JobScheduler::default());
        let busy = scheduler.try_acquire_owned().unwrap();
        scheduler.enqueue("cancelled", "image.generate");
        scheduler.enqueue("next", "image.generate");
        let cancelled = {
            let scheduler = scheduler.clone();
            tokio::spawn(async move { scheduler.acquire("cancelled").await })
        };
        scheduler.remove("cancelled");
        assert!(cancelled.await.unwrap().is_none());
        assert_eq!(scheduler.generation_queue(), ["next"]);
        drop(busy);
        drop(scheduler.acquire("next").await.unwrap());
    }
}
