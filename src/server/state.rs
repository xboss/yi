use crate::translator::{backends::ct2::Ct2Backend, pipeline::Translator};
use std::sync::{Arc, Mutex};

#[derive(Clone)]
pub struct AppState {
    pub en2cn_translator: Arc<Mutex<Translator<Ct2Backend>>>,
}
