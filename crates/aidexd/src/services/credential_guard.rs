//! Stop processes holding decrypted credentials after an idle or external lock.
use super::Services;
use aidex_vault::VaultState;
use std::{
    io,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    thread::{self, JoinHandle},
    time::Duration,
};

pub(crate) struct CredentialGuard {
    stop: Arc<AtomicBool>,
    worker: Option<JoinHandle<()>>,
}
impl CredentialGuard {
    pub(crate) fn start(services: &Services) -> io::Result<Self> {
        let stop = Arc::new(AtomicBool::new(false));
        let token = stop.clone();
        let vault = services.vault().clone();
        let native = services.native().clone();
        let extensions = services.extensions().clone();
        let runtimes = services.runtime().clone();
        let initial_unlocked = vault.status().state == VaultState::Unlocked;
        let worker = thread::Builder::new()
            .name("aidex-credential-guard".into())
            .spawn(move || {
                let mut unlocked = initial_unlocked;
                while !token.load(Ordering::SeqCst) {
                    let now = vault.status().state == VaultState::Unlocked;
                    if unlocked && !now {
                        native.cancel_all();
                        extensions.disconnect_all();
                        let _ = runtimes.cancel_logins();
                    }
                    unlocked = now;
                    thread::sleep(Duration::from_millis(100));
                }
            })?;
        Ok(Self {
            stop,
            worker: Some(worker),
        })
    }
}
impl Drop for CredentialGuard {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}
