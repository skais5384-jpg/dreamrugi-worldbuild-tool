//! 프런트는 원본 경로를 지정하지 않는다. 파일 대화상자가 선택한 경로는 worker 안에서만 쓴다.
use std::{
    path::PathBuf,
    sync::{Arc, OnceLock},
};
type Picker = dyn Fn(bool) -> Result<Option<PathBuf>, super::dto::ErrorDto> + Send + Sync;
static PICKER: OnceLock<Arc<Picker>> = OnceLock::new();
pub(crate) fn install<R: tauri::Runtime>(app: tauri::AppHandle<R>) {
    use tauri_plugin_dialog::DialogExt;
    let _ = PICKER.set(Arc::new(move |image| {
        let dialog = app.dialog().file();
        let dialog = if image {
            dialog.add_filter(
                "Images",
                &[
                    "png", "apng", "jpg", "jpeg", "jpe", "jfif", "webp", "gif", "bmp",
                ],
            )
        } else {
            dialog
        };
        dialog
            .blocking_pick_file()
            .map(|p| {
                p.into_path()
                    .map_err(|_| super::dto::Code::InvalidInput.into())
            })
            .transpose()
    }));
}
pub(crate) fn pick(image: bool) -> Result<Option<PathBuf>, super::dto::ErrorDto> {
    PICKER.get().ok_or(super::dto::Code::Unavailable)?(image)
}
