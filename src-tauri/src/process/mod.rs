#[cfg(windows)]
const CREATE_NO_WINDOW: u32 = 0x08000000;

/// Prevent a standard-library child process from opening a console window on Windows.
pub(crate) fn hide_std_console_window(command: &mut std::process::Command) {
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;

        command.creation_flags(CREATE_NO_WINDOW);
    }

    #[cfg(not(windows))]
    let _ = command;
}

/// Prevent a Tokio child process from opening a console window on Windows.
pub(crate) fn hide_tokio_console_window(command: &mut tokio::process::Command) {
    #[cfg(windows)]
    command.creation_flags(CREATE_NO_WINDOW);

    #[cfg(not(windows))]
    let _ = command;
}
