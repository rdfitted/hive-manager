fn main() {
    // Loading the application permission manifest makes custom `#[tauri::command]`
    // handlers participate in Tauri's runtime ACL. Without an app manifest Tauri 2
    // intentionally treats application commands as globally callable.
    tauri_build::try_build(
        tauri_build::Attributes::new()
            .app_manifest(tauri_build::AppManifest::new()),
    )
    .expect("failed to run Tauri build script");
}
