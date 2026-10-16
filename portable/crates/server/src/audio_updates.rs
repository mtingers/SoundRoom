//! Latest-only authored graph preparation. This worker never opens hardware.
use super::*;
use crate::service::GraphSource;
const NONE: u64 = u64::MAX;
pub(super) struct Queue {
    pending: Mutex<Option<Update>>,
    current: Mutex<Option<Running>>,
    wake: SyncSender<()>,
}
struct Running {
    cancel: Arc<AtomicBool>,
    deadline: Instant,
}
struct Update {
    source: GraphSource,
    revision: u64,
    epoch: u64,
    cancel: Arc<AtomicBool>,
}
pub(super) fn channel() -> (Queue, Receiver<()>) {
    let (wake, receiver) = sync_channel(1);
    (
        Queue {
            pending: Mutex::new(None),
            current: Mutex::new(None),
            wake,
        },
        receiver,
    )
}
impl Queue {
    pub(super) fn schedule(&self, source: GraphSource, revision: u64, epoch: u64) {
        if let Ok(current) = self.current.lock() {
            if let Some(cancel) = current.as_ref() {
                cancel.cancel.store(true, Ordering::Release);
            }
        }
        if let Ok(mut pending) = self.pending.lock() {
            if let Some(previous) = pending.replace(Update {
                source,
                revision,
                epoch,
                cancel: Arc::new(AtomicBool::new(false)),
            }) {
                previous.cancel.store(true, Ordering::Release);
            }
            let _ = self.wake.try_send(());
        }
    }
    pub(super) fn cancel_expired(&self) {
        if let Ok(current) = self.current.lock() {
            if let Some(current) = current.as_ref() {
                if Instant::now() > current.deadline {
                    current.cancel.store(true, Ordering::Release);
                }
            }
        }
    }
    pub(super) fn cancel(&self) {
        if let Ok(current) = self.current.lock() {
            if let Some(cancel) = current.as_ref() {
                cancel.cancel.store(true, Ordering::Release);
            }
        }
        if let Ok(mut pending) = self.pending.lock() {
            if let Some(update) = pending.take() {
                update.cancel.store(true, Ordering::Release);
            }
        }
    }
}
fn current(inner: &Inner, update: &Update) -> bool {
    !inner.stop.load(Ordering::Acquire)
        && !update.cancel.load(Ordering::Acquire)
        && inner.epoch.load(Ordering::Acquire) == update.epoch
        && inner.authored_revision.load(Ordering::Acquire) == update.revision
}
pub(super) fn run(backend: Weak<Inner>, wake: Receiver<()>) {
    loop {
        let received = wake.recv_timeout(Duration::from_millis(50));
        let Some(inner) = backend.upgrade() else {
            return;
        };
        if inner.stop.load(Ordering::Acquire)
            || matches!(
                received,
                Err(std::sync::mpsc::RecvTimeoutError::Disconnected)
            )
        {
            return;
        }
        let update = inner
            .updates
            .pending
            .lock()
            .ok()
            .and_then(|mut pending| pending.take());
        let Some(update) = update else { continue };
        if !current(&inner, &update) {
            continue;
        }
        // A frozen graph is refreshed by the next callback-acknowledged Play/Seek.
        if !inner.playing.load(Ordering::Acquire) {
            continue;
        }
        if let Ok(mut running) = inner.updates.current.lock() {
            *running = Some(Running {
                cancel: update.cancel.clone(),
                deadline: Instant::now() + Duration::from_secs(30),
            });
        }
        inner.set_status(
            update.revision,
            format!("Preparing audio for authored revision {}", update.revision),
        );
        let result = prepare(&inner, &update);
        if inner.authored_revision.load(Ordering::Acquire) == update.revision
            && inner.epoch.load(Ordering::Acquire) == update.epoch
            && !inner.stop.load(Ordering::Acquire)
        {
            match result {
                Ok(_) => inner.set_status(update.revision, "Ready".into()),
                Err(error)=>inner.set_status(update.revision,format!("Audio remains on its previous revision: {error}. Adjust or undo the edit, then retry playback.")),
            }
        }
        if let Ok(mut running) = inner.updates.current.lock() {
            if running
                .as_ref()
                .is_some_and(|cancel| Arc::ptr_eq(&cancel.cancel, &update.cancel))
            {
                *running = None;
            }
        };
    }
}
fn prepare(inner: &Inner, update: &Update) -> Result<(), Error> {
    // Move the source out without holding the queue; its deep materialization stays here.
    // Update's source is immutable, but prepared graph source ownership is required.
    if !current(inner, update) {
        return Err(protocol("Prepared audio was superseded"));
    }
    let project = update.source.materialize();
    if !current(inner, update) {
        return Err(protocol("Prepared audio was superseded"));
    }
    let cycle = inner.loop_range.lock().ok().and_then(|v| *v);
    let (zero, loader) =
        PreparedGraph::prepare_paged(&project, 32 * 1024 * 1024, &update.cancel, cycle)
            .map_err(|error| protocol(&error.to_string()))?;
    let loader = Arc::new(Mutex::new(loader));
    let mut candidate = zero.fork();
    let mut checkpoints = CheckpointCache::new(32 * 1024 * 1024);
    for attempt in 0..3 {
        if !current(inner, update) {
            return Err(protocol("Prepared audio was superseded"));
        }
        let baseline = inner
            .telemetry
            .lock()
            .map_err(|_| protocol("Audio clock unavailable"))?
            .ok_or_else(|| protocol("No callback clock"))?;
        if !baseline.playing || baseline.handoff_token == 0 {
            return Err(protocol("Playback stopped during preparation"));
        }
        let lead = 9600u64 << attempt;
        let target = baseline
            .playback_clock
            .checked_add(lead)
            .filter(|target| *target <= soundroom_engine::model::MAX_FRAME)
            .ok_or_else(|| protocol("Audio clock exhausted"))?;
        candidate
            .seek_playback_clock(target, &checkpoints, &update.cancel)
            .map_err(|error| protocol(&error.to_string()))?;
        let _ = checkpoints.store_playback_clock(&candidate);
        loader
            .lock()
            .map_err(|_| protocol("Page loader unavailable"))?
            .prepare_graph(&candidate, 32768, &update.cancel)
            .map_err(|error| protocol(&error.to_string()))?;
        let (reply, completion) = sync_channel(1);
        inner
            .ordinary
            .try_send(Message::Aligned {
                graph: Box::new(candidate),
                revision: update.revision,
                epoch: update.epoch,
                baseline: baseline.handoff_token,
                clock: target,
                cancel: update.cancel.clone(),
                loader: loader.clone(),
                reply,
            })
            .map_err(|_| Error::QueueFull)?;
        inner.wake_driver();
        match wait(completion, Duration::from_secs(3)) {
            Ok(_) => {
                if let Ok(mut cache) = inner.canonical.try_lock() {
                    *cache = Some(Canonical {
                        revision: update.revision,
                        variant: super::audible_variant(&project.session),
                        loop_range: cycle,
                        graph: zero,
                        checkpoints,
                        loader,
                    });
                }
                inner
                    .pending_revision
                    .compare_exchange(update.revision, NONE, Ordering::AcqRel, Ordering::Acquire)
                    .ok();
                return Ok(());
            }
            Err(error) if attempt == 2 || !current(inner, update) => return Err(error),
            Err(_) => candidate = zero.fork(),
        }
    }
    Err(protocol(
        "Audio preparation could not meet its splice deadline",
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn expired_preparation_cancels_without_timer_threads_and_cancel_is_idempotent() {
        let (queue, _receiver) = channel();
        let cancel = Arc::new(AtomicBool::new(false));
        *queue.current.lock().unwrap() = Some(Running {
            cancel: cancel.clone(),
            deadline: Instant::now() - Duration::from_secs(1),
        });
        queue.cancel_expired();
        assert!(cancel.load(Ordering::Acquire));
        let next = Arc::new(AtomicBool::new(false));
        *queue.current.lock().unwrap() = Some(Running {
            cancel: next.clone(),
            deadline: Instant::now() + Duration::from_secs(1),
        });
        queue.cancel_expired();
        assert!(!next.load(Ordering::Acquire));
        queue.cancel();
        queue.cancel();
        assert!(next.load(Ordering::Acquire));
    }
}
