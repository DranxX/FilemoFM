use crate::shared::{constants::CHANNEL_CAPACITY, models::*, traits::ConfigRepository};
use crossbeam_channel::{Sender, bounded};
use std::sync::Arc;

struct SaveRequest {
    config: AppConfig,
    notify: bool,
    completion: Option<Sender<Result<(), String>>>,
}
pub struct ConfigurationService {
    requests: Sender<SaveRequest>,
}
impl ConfigurationService {
    pub fn new(storage: Arc<dyn ConfigRepository>, sender: EventSink) -> Self {
        let (requests, receiver) = bounded::<SaveRequest>(CHANNEL_CAPACITY);
        std::thread::spawn(move || {
            while let Ok(request) = receiver.recv() {
                let result = storage
                    .save(&request.config)
                    .map_err(|error| error.to_string());
                if let Some(completion) = request.completion {
                    let _ = completion.send(result.clone());
                }
                if request.notify {
                    let _ = sender.send(BackgroundMessage::ConfigurationSaved {
                        config: Box::new(request.config),
                        result,
                    });
                } else if let Err(error) = result {
                    let _ = sender.send(BackgroundMessage::Diagnostic(Err(error)));
                }
            }
        });
        Self { requests }
    }
    pub fn save(&self, config: AppConfig, notify: bool) -> Result<(), String> {
        self.requests
            .try_send(SaveRequest {
                config,
                notify,
                completion: None,
            })
            .map_err(|_| "Settings save queue is full".into())
    }
    pub fn flush(&self, config: AppConfig) -> Result<(), String> {
        let (completion, receiver) = bounded(1);
        self.requests
            .send_timeout(
                SaveRequest {
                    config,
                    notify: false,
                    completion: Some(completion),
                },
                std::time::Duration::from_secs(5),
            )
            .map_err(|_| "Settings storage stopped")?;
        receiver
            .recv_timeout(std::time::Duration::from_secs(5))
            .map_err(|_| "Saving settings timed out")?
    }
}
