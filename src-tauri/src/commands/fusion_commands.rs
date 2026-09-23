#[cfg(not(test))]
use tauri::State;

#[cfg(not(test))]
use super::SessionControllerState;

fn apply_with<F>(session_id: &str, variant_name: &str, select: F) -> Result<(), String>
where
    F: FnOnce(&str, &str) -> Result<(), String>,
{
    select(session_id, variant_name)
}

#[cfg(not(test))]
#[tauri::command]
pub async fn apply_fusion_winner(
    state: State<'_, SessionControllerState>,
    session_id: String,
    variant_name: String,
) -> Result<(), String> {
    let controller = state.0.write();
    apply_with(&session_id, &variant_name, |session_id, variant_name| {
        controller.select_fusion_winner(session_id, variant_name)
    })
}

#[cfg(test)]
mod tests {
    use super::apply_with;

    #[test]
    fn apply_passes_session_and_variant_and_propagates_success() {
        let result = apply_with("fusion-session", "Fast Path", |session_id, variant_name| {
            assert_eq!(session_id, "fusion-session");
            assert_eq!(variant_name, "Fast Path");
            Ok(())
        });
        assert!(result.is_ok());
    }

    #[test]
    fn apply_preserves_unknown_variant_error() {
        let result = apply_with("fusion-session", "Unknown", |_, _| {
            Err("Variant 'Unknown' not found for session fusion-session".to_string())
        });
        assert_eq!(
            result.unwrap_err(),
            "Variant 'Unknown' not found for session fusion-session"
        );
    }
}
