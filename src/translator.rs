//! All TranslationSession objects live on ONE dedicated OS thread.
//! No TranslationSession, MutexGuard, or other non-Send object crosses .await.

use std::{
    collections::{HashMap, VecDeque},
    io,
    sync::mpsc::{self, Receiver, SyncSender, TrySendError},
};

use tokio::sync::oneshot;
use translation::{
    Language, TranslationConfiguration, TranslationError, TranslationSession,
};

const QUEUE_CAPACITY: usize = 16;
const SESSION_CACHE_CAPACITY: usize = 4;

type Pair = (String, String);

#[derive(Debug)]
pub enum JobError {
    InvalidLanguage(String),
    Framework(TranslationError),
}

#[derive(Debug)]
pub enum SubmitError {
    QueueFull,
    WorkerStopped,
}

struct Job {
    text: String,
    from: String,
    to: String,
    reply: oneshot::Sender<Result<String, JobError>>,
}

#[derive(Clone)]
pub struct TranslatorHandle {
    sender: SyncSender<Job>,
}

impl TranslatorHandle {
    pub fn start() -> io::Result<Self> {
        let (sender, receiver) = mpsc::sync_channel(QUEUE_CAPACITY);
        std::thread::Builder::new()
            .name("apple-translation-worker".to_string())
            .spawn(move || worker_main(receiver))?;
        Ok(Self { sender })
    }

    pub fn submit(
        &self,
        text: String,
        from: String,
        to: String,
    ) -> Result<oneshot::Receiver<Result<String, JobError>>, SubmitError> {
        let (reply, receiver) = oneshot::channel();
        let job = Job { text, from, to, reply };
        match self.sender.try_send(job) {
            Ok(()) => Ok(receiver),
            Err(TrySendError::Full(_)) => Err(SubmitError::QueueFull),
            Err(TrySendError::Disconnected(_)) => Err(SubmitError::WorkerStopped),
        }
    }
}

struct SessionCache {
    entries: HashMap<Pair, TranslationSession>,
    recency: VecDeque<Pair>,
}

impl SessionCache {
    fn new() -> Self {
        Self {
            entries: HashMap::new(),
            recency: VecDeque::new(),
        }
    }

    fn translate(&mut self, from: &str, to: &str, text: &str) -> Result<String, JobError> {
        // Canonicalize language identifiers so e.g. equivalent spelling maps
        // to the same cache key and the Apple framework sees expected tags.
        let source = Language::new(from)
            .canonicalized()
            .map_err(|e| JobError::InvalidLanguage(e.to_string()))?;
        let target = Language::new(to)
            .canonicalized()
            .map_err(|e| JobError::InvalidLanguage(e.to_string()))?;
        // Canonicalized Language's string conversion is intentionally avoided:
        // user-supplied normalized tags are sufficient as a stable cache key.
        let key = (from.to_ascii_lowercase(), to.to_ascii_lowercase());

        if !self.entries.contains_key(&key) {
            let config = TranslationConfiguration::new()
                .with_source(source)
                .with_target(target);

            let session = TranslationSession::from_translation_configuration(&config)
                .map_err(JobError::Framework)?;

            if self.entries.len() >= SESSION_CACHE_CAPACITY {
                if let Some(oldest) = self.recency.pop_front() {
                    self.entries.remove(&oldest);
                }
            }
            self.entries.insert(key.clone(), session);
        }

        self.recency.retain(|cached| cached != &key);
        self.recency.push_back(key.clone());

        let session = self.entries.get(&key).expect("session inserted above");
        session
            .translate(text)
            .map(|response| response.target_text().to_owned())
            .map_err(JobError::Framework)
    }
}

fn worker_main(receiver: Receiver<Job>) {
    let mut cache = SessionCache::new();

    while let Ok(job) = receiver.recv() {
        // The HTTP client may have timed out while this job was queued.
        if job.reply.is_closed() {
            continue;
        }
        let translated = cache.translate(&job.from, &job.to, &job.text);
        let _ = job.reply.send(translated);
    }
}

pub fn valid_language_tag(tag: &str) -> bool {
    // Conservative BCP-47-style input filter, not a language support check.
    !tag.is_empty()
        && tag.len() <= 35
        && tag.split('-').all(|part| {
            !part.is_empty() && part.len() <= 8 && part.bytes().all(|ch| ch.is_ascii_alphanumeric())
        })
        && tag.as_bytes()[0].is_ascii_alphabetic()
}

#[cfg(test)]
mod tests {
    use super::valid_language_tag;

    #[test]
    fn accepts_normal_language_tags() {
        for tag in ["en", "zh-Hans", "zh-Hant", "en-US", "fr", "ja"] {
            assert!(valid_language_tag(tag), "{tag}");
        }
    }

    #[test]
    fn rejects_bad_tags() {
        for tag in ["", "--", "zh--Hans", "/etc/passwd", "a b", "1en"] {
            assert!(!valid_language_tag(tag), "{tag}");
        }
    }
}
