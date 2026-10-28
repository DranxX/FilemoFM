use crate::shared::{models::*, traits::MutationApprover};
use std::time::Duration;

pub struct InteractiveApproval {
    sender: EventSink,
}
impl InteractiveApproval {
    pub fn new(sender: EventSink) -> Self {
        Self { sender }
    }
}
impl MutationApprover for InteractiveApproval {
    fn approve(&self, intent: MutationIntent) -> Result<(), String> {
        let (response, receiver) = crossbeam_channel::bounded(1);
        self.sender
            .send(BackgroundMessage::MutationApproval { intent, response })
            .map_err(|_| "Approval UI is unavailable")?;
        if receiver
            .recv_timeout(Duration::from_secs(120))
            .map_err(|_| "Approval timed out")?
        {
            Ok(())
        } else {
            Err("Operation rejected by the user".into())
        }
    }
}
