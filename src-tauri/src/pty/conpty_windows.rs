//! Windows ConPTY whose root cannot execute before job assignment.
use crate::transport::process::ProcessTree;
use anyhow::{ensure, Context, Result};
use portable_pty::{
    Child, ChildKiller, CommandBuilder, ExitStatus, MasterPty, PtyPair, PtySize, SlavePty,
};
use std::{
    ffi::{c_void, OsStr, OsString},
    fs::File,
    io::{self, Read, Write},
    mem::{size_of, zeroed},
    os::windows::{
        ffi::OsStrExt,
        io::{AsRawHandle, FromRawHandle, OwnedHandle, RawHandle},
    },
    path::Path,
    ptr::{null, null_mut},
    sync::{Arc, Mutex},
};
use windows_sys::Win32::{
    Foundation::*,
    System::{Console::*, Pipes::CreatePipe, Threading::*},
    UI::WindowsAndMessaging::SW_HIDE,
};

fn checked(result: i32) -> io::Result<()> {
    if result == 0 {
        Err(io::Error::last_os_error())
    } else {
        Ok(())
    }
}
fn pipe() -> io::Result<(File, File)> {
    let (mut input, mut output) = (null_mut(), null_mut());
    unsafe {
        checked(CreatePipe(&mut input, &mut output, null(), 0))?;
        Ok((File::from_raw_handle(input), File::from_raw_handle(output)))
    }
}
struct Console(HPCON);
impl Drop for Console {
    fn drop(&mut self) {
        unsafe { ClosePseudoConsole(self.0) }
    }
}
struct Inner {
    console: Console,
    reader: File,
    writer: Option<File>,
    size: PtySize,
}
struct Master(Arc<Mutex<Inner>>);
struct Slave {
    inner: Arc<Mutex<Inner>>,
    job: Arc<ProcessTree>,
}

pub(crate) fn openpty(size: PtySize) -> Result<(PtyPair, Arc<ProcessTree>)> {
    ensure!(
        size.cols > 0
            && size.cols <= i16::MAX as u16
            && size.rows > 0
            && size.rows <= i16::MAX as u16,
        "invalid ConPTY size"
    );
    let job = Arc::new(ProcessTree::new().map_err(anyhow::Error::msg)?);
    let (input_read, input_write) = pipe()?;
    let (output_read, output_write) = pipe()?;
    let mut hpc = 0;
    let hr = unsafe {
        CreatePseudoConsole(
            COORD {
                X: size.cols as i16,
                Y: size.rows as i16,
            },
            input_read.as_raw_handle(),
            output_write.as_raw_handle(),
            0x2 | 0x4,
            &mut hpc,
        )
    };
    ensure!(hr >= 0, "CreatePseudoConsole failed: {hr:#x}");
    let console = Console(hpc);
    drop(input_read);
    drop(output_write);
    let inner = Arc::new(Mutex::new(Inner {
        console,
        reader: output_read,
        writer: Some(input_write),
        size,
    }));
    Ok((
        PtyPair {
            slave: Box::new(Slave {
                inner: inner.clone(),
                job: job.clone(),
            }),
            master: Box::new(Master(inner)),
        },
        job,
    ))
}
impl MasterPty for Master {
    fn resize(&self, size: PtySize) -> Result<()> {
        ensure!(
            size.cols > 0
                && size.cols <= i16::MAX as u16
                && size.rows > 0
                && size.rows <= i16::MAX as u16,
            "invalid ConPTY size"
        );
        let mut inner = self.0.lock().unwrap();
        let hr = unsafe {
            ResizePseudoConsole(
                inner.console.0,
                COORD {
                    X: size.cols as i16,
                    Y: size.rows as i16,
                },
            )
        };
        ensure!(hr >= 0, "ResizePseudoConsole failed: {hr:#x}");
        inner.size = size;
        Ok(())
    }
    fn get_size(&self) -> Result<PtySize> {
        Ok(self.0.lock().unwrap().size)
    }
    fn try_clone_reader(&self) -> Result<Box<dyn Read + Send>> {
        Ok(Box::new(self.0.lock().unwrap().reader.try_clone()?))
    }
    fn take_writer(&self) -> Result<Box<dyn Write + Send>> {
        Ok(Box::new(
            self.0
                .lock()
                .unwrap()
                .writer
                .take()
                .context("PTY writer already taken")?,
        ))
    }
}
struct Attributes {
    _storage: Vec<usize>,
    raw: LPPROC_THREAD_ATTRIBUTE_LIST,
}
impl Attributes {
    fn new(hpc: HPCON) -> Result<Self> {
        unsafe {
            let mut bytes = 0;
            InitializeProcThreadAttributeList(null_mut(), 1, 0, &mut bytes);
            ensure!(bytes > 0, "attribute list size");
            let mut storage = vec![0usize; bytes.div_ceil(size_of::<usize>())];
            let raw = storage.as_mut_ptr().cast();
            checked(InitializeProcThreadAttributeList(raw, 1, 0, &mut bytes))?;
            let list = Self {
                _storage: storage,
                raw,
            };
            checked(UpdateProcThreadAttribute(
                raw,
                0,
                PROC_THREAD_ATTRIBUTE_PSEUDOCONSOLE as usize,
                hpc as *const c_void,
                size_of::<HPCON>(),
                null_mut(),
                null(),
            ))?;
            Ok(list)
        }
    }
}
impl Drop for Attributes {
    fn drop(&mut self) {
        unsafe { DeleteProcThreadAttributeList(self.raw) }
    }
}
struct Suspended {
    process: Arc<OwnedHandle>,
    armed: bool,
}
impl Drop for Suspended {
    fn drop(&mut self) {
        if self.armed {
            unsafe {
                TerminateProcess(self.process.as_raw_handle(), 1);
                WaitForSingleObject(self.process.as_raw_handle(), INFINITE);
            }
        }
    }
}
impl SlavePty for Slave {
    fn spawn_command(&self, cmd: CommandBuilder) -> Result<Box<dyn Child + Send + Sync>> {
        let inner = self.inner.lock().unwrap();
        let attrs = Attributes::new(inner.console.0)?;
        let (exe, mut line) = command_line(&cmd)?;
        let env = environment(&cmd)?;
        let cwd = directory(&cmd);
        let mut si: STARTUPINFOEXW = unsafe { zeroed() };
        si.StartupInfo.cb = size_of::<STARTUPINFOEXW>() as u32;
        si.StartupInfo.dwFlags = STARTF_USESTDHANDLES | STARTF_USESHOWWINDOW;
        si.StartupInfo.wShowWindow = SW_HIDE as u16;
        si.StartupInfo.hStdInput = INVALID_HANDLE_VALUE;
        si.StartupInfo.hStdOutput = INVALID_HANDLE_VALUE;
        si.StartupInfo.hStdError = INVALID_HANDLE_VALUE;
        si.lpAttributeList = attrs.raw;
        let mut pi: PROCESS_INFORMATION = unsafe { zeroed() };
        unsafe {
            checked(CreateProcessW(
                exe.as_ptr(),
                line.as_mut_ptr(),
                null(),
                null(),
                0,
                CREATE_SUSPENDED | EXTENDED_STARTUPINFO_PRESENT | CREATE_UNICODE_ENVIRONMENT,
                env.as_ptr().cast(),
                cwd.as_ref().map_or(null(), |p| p.as_ptr()),
                &si.StartupInfo,
                &mut pi,
            ))?;
        }
        let process = Arc::new(unsafe { OwnedHandle::from_raw_handle(pi.hProcess) });
        let thread = unsafe { OwnedHandle::from_raw_handle(pi.hThread) };
        let mut suspended = Suspended {
            process: process.clone(),
            armed: true,
        };
        #[cfg(test)]
        super::real_session::run_pre_assignment_hook(Some(thread.as_raw_handle()));
        self.job
            .assign_suspended(pi.hProcess)
            .map_err(anyhow::Error::msg)?;
        let previous = unsafe { ResumeThread(thread.as_raw_handle()) };
        ensure!(
            previous == 1,
            "ResumeThread failed: {}",
            io::Error::last_os_error()
        );
        suspended.armed = false;
        Ok(Box::new(WinChild {
            process,
            pid: pi.dwProcessId,
        }))
    }
}
#[derive(Debug)]
struct WinChild {
    process: Arc<OwnedHandle>,
    pid: u32,
}
#[derive(Debug)]
struct Killer(Arc<OwnedHandle>);
fn terminate(process: &OwnedHandle) -> io::Result<()> {
    unsafe {
        if TerminateProcess(process.as_raw_handle(), 1) != 0 {
            return Ok(());
        }
        let error = io::Error::last_os_error();
        if WaitForSingleObject(process.as_raw_handle(), 0) == WAIT_OBJECT_0 {
            Ok(())
        } else {
            Err(error)
        }
    }
}
impl ChildKiller for Killer {
    fn kill(&mut self) -> io::Result<()> {
        terminate(&self.0)
    }
    fn clone_killer(&self) -> Box<dyn ChildKiller + Send + Sync> {
        Box::new(Killer(self.0.clone()))
    }
}
impl ChildKiller for WinChild {
    fn kill(&mut self) -> io::Result<()> {
        terminate(&self.process)
    }
    fn clone_killer(&self) -> Box<dyn ChildKiller + Send + Sync> {
        Box::new(Killer(self.process.clone()))
    }
}
impl WinChild {
    fn status(&self, timeout: u32) -> io::Result<Option<ExitStatus>> {
        unsafe {
            match WaitForSingleObject(self.process.as_raw_handle(), timeout) {
                WAIT_OBJECT_0 => {
                    let mut code = 0;
                    checked(GetExitCodeProcess(self.process.as_raw_handle(), &mut code))?;
                    Ok(Some(ExitStatus::with_exit_code(code)))
                }
                WAIT_TIMEOUT => Ok(None),
                _ => Err(io::Error::last_os_error()),
            }
        }
    }
}
impl Child for WinChild {
    fn try_wait(&mut self) -> io::Result<Option<ExitStatus>> {
        self.status(0)
    }
    fn wait(&mut self) -> io::Result<ExitStatus> {
        self.status(INFINITE)?
            .ok_or_else(|| io::Error::other("process wait timed out"))
    }
    fn process_id(&self) -> Option<u32> {
        Some(self.pid)
    }
    fn as_raw_handle(&self) -> Option<RawHandle> {
        Some(self.process.as_raw_handle())
    }
}
fn wide(value: &OsStr) -> Vec<u16> {
    value.encode_wide().chain(Some(0)).collect()
}
fn search_path(cmd: &CommandBuilder, exe: &OsStr) -> OsString {
    if let Some(paths) = cmd.get_env("PATH") {
        let extensions = cmd.get_env("PATHEXT").unwrap_or(OsStr::new(".EXE"));
        for path in std::env::split_paths(paths) {
            let candidate = path.join(exe);
            if candidate.exists() {
                return candidate.into_os_string();
            }
            for ext in std::env::split_paths(extensions) {
                if let Some(ext) = ext.to_str().and_then(|s| s.strip_prefix('.')) {
                    let candidate = path.join(exe).with_extension(ext);
                    if candidate.exists() {
                        return candidate.into_os_string();
                    }
                }
            }
        }
    }
    exe.to_owned()
}
fn quote(arg: &OsStr, line: &mut Vec<u16>) {
    let units: Vec<_> = arg.encode_wide().collect();
    if !units.is_empty() && !units.iter().any(|c| [32, 9, 10, 11, 34].contains(c)) {
        line.extend(units);
        return;
    }
    line.push(34);
    let mut slashes = 0;
    for c in units {
        if c == 92 {
            slashes += 1;
            continue;
        }
        line.extend(std::iter::repeat_n(
            92,
            if c == 34 { slashes * 2 + 1 } else { slashes },
        ));
        line.push(c);
        slashes = 0;
    }
    line.extend(std::iter::repeat_n(92, slashes * 2));
    line.push(34);
}
fn command_line(cmd: &CommandBuilder) -> Result<(Vec<u16>, Vec<u16>)> {
    let exe = if cmd.is_default_prog() {
        cmd.get_env("ComSpec")
            .unwrap_or(OsStr::new("cmd.exe"))
            .to_owned()
    } else {
        search_path(cmd, &cmd.get_argv()[0])
    };
    ensure!(!exe.encode_wide().any(|u| u == 0), "NUL in PTY executable");
    let mut line = Vec::new();
    quote(&exe, &mut line);
    for arg in cmd.get_argv().iter().skip(1) {
        ensure!(!arg.encode_wide().any(|u| u == 0), "NUL in PTY argument");
        line.push(32);
        quote(arg, &mut line)
    }
    line.push(0);
    Ok((wide(&exe), line))
}
fn environment(cmd: &CommandBuilder) -> Result<Vec<u16>> {
    // Refuse a public-API representation that would silently lose raw UTF-16.
    let mut hidden = cmd.clone();
    for (key, _) in cmd.iter_full_env_as_str() {
        hidden.env_remove(key);
    }
    let mut empty = cmd.clone();
    empty.env_clear();
    if hidden != empty {
        let known: Vec<_> = std::env::vars_os()
            .filter_map(|(key, _)| {
                let name = key.to_str()?;
                cmd.get_env(&key)
                    .filter(|value| value.to_str().is_none())
                    .map(|_| name.to_owned())
            })
            .collect();
        anyhow::bail!("PTY environment has an unpaired UTF-16 surrogate in a key or value unavailable through CommandBuilder's public UTF-8 iterator; discoverable affected keys: {}",if known.is_empty(){"unavailable".to_owned()}else{known.join(", ")});
    }
    let mut block = Vec::new();
    for (key, value) in cmd.iter_full_env_as_str() {
        ensure!(
            !key.contains('\0') && !value.contains('\0'),
            "NUL in PTY environment"
        );
        block.extend(key.encode_utf16());
        block.push(61);
        block.extend(value.encode_utf16());
        block.push(0)
    }
    block.push(0);
    if block.len() == 1 {
        block.push(0)
    }
    Ok(block)
}
fn directory(cmd: &CommandBuilder) -> Option<Vec<u16>> {
    let explicit = cmd.get_cwd().filter(|p| Path::new(p).is_dir());
    let home = cmd.get_env("USERPROFILE").filter(|p| Path::new(p).is_dir());
    explicit.map(|p| p.as_os_str()).or(home).map(|dir| {
        let path = Path::new(dir);
        if path.is_relative() {
            if let Ok(cwd) = std::env::current_dir() {
                return wide(cwd.join(path).as_os_str());
            }
        }
        wide(dir)
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn argv_quote_matches_command_builder_getters() {
        let values = [
            "plain",
            "",
            "two words",
            "a\"b",
            r"space end\",
            "space end\\\\",
            r#"two \"quoted\" values\"#,
            "tab\tvalue",
        ];
        let expected = [
            "plain",
            r#""""#,
            r#""two words""#,
            r#""a\"b""#,
            r#""space end\\""#,
            r#""space end\\\\""#,
            r#""two \\\"quoted\\\" values\\""#,
            "\"tab\tvalue\"",
        ];
        let mut cmd = CommandBuilder::new("cmd.exe");
        cmd.args(values);
        for (arg, expected) in cmd.get_argv().iter().skip(1).zip(expected) {
            let mut actual = Vec::new();
            quote(arg, &mut actual);
            assert_eq!(String::from_utf16(&actual).unwrap(), expected, "{arg:?}")
        }
    }
    #[test]
    fn env_block_preserves_public_iterator_order_and_preferred_casing() {
        let mut cmd = CommandBuilder::new("cmd.exe");
        cmd.env_clear();
        cmd.env("zLast", "3");
        cmd.env("ALPHA", "1");
        cmd.env("beta", "2");
        cmd.env("aLpHa", "updated");
        let entries = cmd
            .iter_full_env_as_str()
            .map(|(k, v)| format!("{k}={v}"))
            .collect::<Vec<_>>();
        let mut expected = entries.join("\0");
        expected.push('\0');
        expected.push('\0');
        assert_eq!(
            environment(&cmd).unwrap(),
            expected.encode_utf16().collect::<Vec<_>>()
        );
        assert_eq!(entries, ["aLpHa=updated", "beta=2", "zLast=3"]);
    }
    #[test]
    fn unrepresentable_env_is_refused_instead_of_silently_dropped() {
        use std::os::windows::ffi::OsStringExt;
        let mut cmd = CommandBuilder::new("cmd.exe");
        cmd.env_clear();
        cmd.env("SYNTHETIC_RAW", OsString::from_wide(&[0xD800]));
        assert_eq!(cmd.iter_full_env_as_str().count(), 0);
        assert!(cmd.get_env("SYNTHETIC_RAW").is_some());
        let error = environment(&cmd).unwrap_err();
        assert!(error.to_string().contains("unpaired UTF-16"));
    }
    #[test]
    fn path_search_and_cwd_match_builder_getters() {
        let root = std::path::PathBuf::from(std::env::var_os("SystemRoot").unwrap());
        let bin = root.join("System32");
        let mut cmd = CommandBuilder::new("cmd");
        cmd.env("PATH", bin.as_os_str());
        cmd.env("PATHEXT", ".EXE;.COM");
        cmd.cwd(&root);
        assert_eq!(
            search_path(&cmd, &cmd.get_argv()[0]),
            bin.join("cmd.EXE").into_os_string()
        );
        assert_eq!(
            directory(&cmd),
            Some(wide(cmd.get_cwd().unwrap().as_os_str()))
        );
        cmd.cwd(root.join("synthetic-nonexistent-spike-dir"));
        cmd.env("USERPROFILE", &root);
        assert_eq!(directory(&cmd), Some(wide(root.as_os_str())));
    }
}
