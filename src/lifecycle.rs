use std::{env, io, os::unix::process::CommandExt, process::Command};

use tokio::sync::watch;

#[derive(Debug, Copy, Clone, PartialEq, Eq)]
pub enum ExitAction {
    Running,
    Stop,
    Restart,
}

#[derive(Clone)]
pub struct Lifecycle {
    tx: watch::Sender<ExitAction>,
}

impl Lifecycle {
    pub fn new() -> Self {
        let (tx, _) = watch::channel(ExitAction::Running);
        Self { tx }
    }

    pub fn request(&self, action: ExitAction) {
        self.tx.send_if_modified(|current| {
            if *current == ExitAction::Running {
                *current = action;
                true
            } else {
                false
            }
        });
    }

    pub async fn wait(&self) -> ExitAction {
        let mut rx = self.tx.subscribe();
        let action = rx
            .wait_for(|action| *action != ExitAction::Running)
            .await
            .expect("lifecycle sender must exist");
        *action
    }

    pub fn request_stop(&self) {
        self.request(ExitAction::Stop);
    }

    pub fn request_restart(&self) {
        self.request(ExitAction::Restart);
    }

    pub fn action(&self) -> ExitAction {
        *self.tx.borrow()
    }
}

pub fn restart_process_on_unix() -> io::Result<()> {
    let exe = env::current_exe()?;
    let args = env::args_os().skip(1);
    Err(Command::new(exe).args(args).exec())
}
