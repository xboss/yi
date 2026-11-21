use crate::translator::{backends::ct2::Ct2Backend, pipeline::Translator};
use anyhow::{Context, Result};
use std::sync::{Arc, Mutex};

#[derive(Clone)]
pub struct AppState {
    pub en2cn_translator: Arc<Mutex<Translator<Ct2Backend>>>,
}

impl AppState {
    pub async fn translate_en2cn_texts(&self, texts: Vec<String>) -> Result<Vec<String>> {
        let translator = Arc::clone(&self.en2cn_translator);

        tokio::task::spawn_blocking(move || {
            let mut translator = translator
                .lock()
                .map_err(|_| anyhow::anyhow!("translator mutex posisoned"))?;
            texts
                .into_iter()
                .map(|text| {
                    let result = translator.translate_text(&text)?;
                    Ok(result.text)
                })
                .collect::<Result<Vec<_>>>()
        })
        .await
        .context("english to chinese translation worker failed")?
    }
}
