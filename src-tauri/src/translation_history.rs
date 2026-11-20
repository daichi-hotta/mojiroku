//! Read what a meeting saved from its live captions, without aligning it to final STT:
//! the completed translations (ADR-0038) and the live transcript itself (ADR-0048).
use mojiroku_core::store::{SavedLiveCaption, SavedLiveTranslation, SqliteStore};
use tauri::State;

#[tauri::command]
pub(crate) fn list_live_translations(
    store: State<'_, SqliteStore>,
    id: String,
) -> Result<Vec<SavedLiveTranslation>, String> {
    store.list_live_translations(&id).map_err(|e| e.to_string())
}

#[tauri::command]
pub(crate) fn list_live_captions(
    store: State<'_, SqliteStore>,
    id: String,
) -> Result<Vec<SavedLiveCaption>, String> {
    store.list_live_captions(&id).map_err(|e| e.to_string())
}
