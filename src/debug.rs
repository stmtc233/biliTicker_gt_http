use image::{DynamicImage, ImageFormat};
use std::env;
use std::fs;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::OnceLock;
use std::time::{SystemTime, UNIX_EPOCH};

static DEBUG_ENABLED: OnceLock<bool> = OnceLock::new();
static ARTIFACT_COUNTER: AtomicU64 = AtomicU64::new(0);

pub(crate) fn init_from_startup_options() -> bool {
    let enabled = env::args().any(|arg| arg == "--debug" || arg == "-d")
        || env::var("BILI_TICKET_GT_DEBUG")
            .map(|value| matches!(value.trim().to_ascii_lowercase().as_str(), "1" | "true" | "yes" | "on"))
            .unwrap_or(false);

    let _ = DEBUG_ENABLED.set(enabled);
    enabled
}

pub(crate) fn enabled() -> bool {
    DEBUG_ENABLED.get().copied().unwrap_or(false)
}

pub(crate) fn save_image(category: &str, image: &DynamicImage) {
    if !enabled() {
        return;
    }

    let root = match env::current_dir() {
        Ok(path) => path,
        Err(error) => {
            tracing::warn!(error = %error, "无法确定调试图片保存目录");
            return;
        }
    };
    let directory = root.join("debug_artifacts");
    if let Err(error) = fs::create_dir_all(&directory) {
        tracing::warn!(path = %directory.display(), error = %error, "无法创建调试图片目录");
        return;
    }

    let timestamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis())
        .unwrap_or_default();
    let sequence = ARTIFACT_COUNTER.fetch_add(1, Ordering::Relaxed);
    let path: PathBuf = directory.join(format!("{category}-{timestamp}-{sequence}.png"));

    match image.save_with_format(&path, ImageFormat::Png) {
        Ok(()) => tracing::debug!(path = %path.display(), "已保存调试图片"),
        Err(error) => tracing::warn!(path = %path.display(), error = %error, "保存调试图片失败"),
    }
}
