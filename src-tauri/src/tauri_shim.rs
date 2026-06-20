#[cfg(not(test))]
pub use tauri::{AppHandle, Emitter};

#[cfg(test)]
#[derive(Clone, Debug)]
pub struct AppHandle;

#[cfg(test)]
pub trait Emitter {
    fn emit<S: serde::Serialize>(&self, _event: &str, _payload: S) -> Result<(), String>;
}

#[cfg(test)]
impl Emitter for AppHandle {
    fn emit<S: serde::Serialize>(&self, _event: &str, _payload: S) -> Result<(), String> {
        Ok(())
    }
}
