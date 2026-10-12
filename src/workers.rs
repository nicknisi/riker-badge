//! Reserve every worker stack before starting network work. Failed startup
//! cancels and joins the waiting workers, rather than leaking a partial service.
use std::{
    io,
    sync::mpsc::sync_channel,
    thread::{Builder, JoinHandle},
};
pub type Work = Box<dyn FnOnce() + Send + 'static>;
pub type Worker = (&'static str, usize, Work);
pub fn start_workers(jobs: Vec<Worker>) -> io::Result<()> {
    start_with(jobs, |builder, task| builder.spawn(task))
}
fn start_with(
    jobs: Vec<Worker>,
    mut spawn: impl FnMut(Builder, Work) -> io::Result<JoinHandle<()>>,
) -> io::Result<()> {
    let mut starts = Vec::with_capacity(jobs.len());
    let mut threads = Vec::with_capacity(jobs.len());
    for (name, stack, work) in jobs {
        let (start, wait) = sync_channel::<()>(0);
        let task: Work = Box::new(move || {
            if wait.recv().is_ok() {
                work();
            }
        });
        match spawn(Builder::new().name(name.into()).stack_size(stack), task) {
            Ok(thread) => {
                starts.push(start);
                threads.push(thread);
            }
            Err(error) => {
                drop(starts);
                for thread in threads {
                    let _ = thread.join();
                }
                return Err(io::Error::new(error.kind(), format!("{name}: {error}")));
            }
        }
    }
    for start in starts {
        let _ = start.send(());
    }
    Ok(())
}
#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{
        atomic::{AtomicUsize, Ordering},
        mpsc, Arc,
    };
    #[test]
    fn allocation_failure_cancels_partial_startup_and_allows_retry() {
        let ran = Arc::new(AtomicUsize::new(0));
        let jobs = (0..3)
            .map(|_| {
                let count = ran.clone();
                (
                    "worker",
                    64 * 1024,
                    Box::new(move || {
                        count.fetch_add(1, Ordering::SeqCst);
                    }) as Work,
                )
            })
            .collect();
        let mut spawned = 0;
        let error = start_with(jobs, |builder, task| {
            spawned += 1;
            if spawned == 3 {
                Err(io::Error::from_raw_os_error(12))
            } else {
                builder.spawn(task)
            }
        })
        .unwrap_err();
        assert_eq!(spawned, 3);
        assert!(error.to_string().starts_with("worker:"));
        assert_eq!(ran.load(Ordering::SeqCst), 0, "no partial service may run");
        let (tx, rx) = mpsc::channel();
        let jobs = (0..3)
            .map(|_| {
                let tx = tx.clone();
                (
                    "worker",
                    64 * 1024,
                    Box::new(move || tx.send(()).unwrap()) as Work,
                )
            })
            .collect();
        start_workers(jobs).unwrap();
        for _ in 0..3 {
            rx.recv_timeout(std::time::Duration::from_secs(2)).unwrap();
        }
    }
}
