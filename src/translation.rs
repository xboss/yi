use serde::{Serialize};
use anyhow::{Result};
    
#[derive(Debug, Serialize)]
pub struct Output {
    pub word: String,
    pub phonetic_us: Option<String>,
    pub phonetic_uk: Option<String>,
    pub audio_us: Option<String>,
    pub audio_uk: Option<String>,
    pub pos: Option<Vec<String>>,
    pub meanings: Option<Vec<String>>,
    pub desc: Option<String>,
}


impl Output {
    pub fn new(word: &str) -> Self {
        Self {
            word: word.to_string(),
            phonetic_us: None,
            phonetic_uk: None,
            audio_us: None,
            audio_uk: None,
            pos: None,
            meanings: None,
            desc: None,
        }
    }
}

pub trait Translation {
    fn translate(&self) -> Result<Output>;
}

