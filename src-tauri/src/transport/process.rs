//! Keep a process tree owned even after its direct parent exits.
use std::time::{Duration, Instant};

pub(crate) const STOP_BUDGET: Duration = Duration::from_secs(30);

#[derive(Debug, PartialEq, Eq)]
pub(crate) enum StopOutcome {
    Exited,
    VerifiedGone,
}

/// Only an identity-bound probe can supersede a failed termination request.
/// Probe errors are unknown state, never evidence that a process disappeared.
pub(crate) fn wait_for_exit(
    termination: Result<(), String>,
    budget: Duration,
    gone: impl FnMut() -> Result<bool, String>,
) -> Result<StopOutcome, String> {
    wait_for_exit_until(termination, Instant::now() + budget, gone)
}

pub(crate) fn wait_for_exit_until(
    termination: Result<(), String>,
    deadline: Instant,
    mut gone: impl FnMut() -> Result<bool, String>,
) -> Result<StopOutcome, String> {
    loop {
        if gone()? {
            return Ok(if termination.is_ok() {
                StopOutcome::Exited
            } else {
                StopOutcome::VerifiedGone
            });
        }
        if Instant::now() >= deadline {
            return Err(format!(
                "Process tree still alive after cleanup budget; retry stop{}",
                termination
                    .as_ref()
                    .err()
                    .map(|error| format!(": {error}"))
                    .unwrap_or_default()
            ));
        }
        std::thread::sleep(
            Duration::from_millis(20).min(deadline.saturating_duration_since(Instant::now())),
        );
    }
}

pub(crate) fn stop_child(
    child: &mut std::process::Child,
    tree: Option<&ProcessTree>,
) -> Result<(), String> {
    let termination = match tree {
        Some(tree) => tree.terminate(),
        None => child.kill().map_err(|error| error.to_string()),
    };
    wait_for_child_exit(child, tree, termination, STOP_BUDGET)
}

pub(crate) fn wait_for_child_exit(
    child: &mut std::process::Child,
    tree: Option<&ProcessTree>,
    termination: Result<(), String>,
    budget: Duration,
) -> Result<(), String> {
    wait_for_exit(termination, budget, || {
        let root_gone = child
            .try_wait()
            .map_err(|error| error.to_string())?
            .is_some();
        let tree_gone = match tree {
            Some(tree) => tree.is_empty()?,
            None => root_gone,
        };
        Ok(root_gone && tree_gone)
    })
    .map(|_| ())
}

#[cfg(windows)]
mod windows {
    use std::ffi::c_void;
    use std::os::windows::io::AsRawHandle;
    #[repr(C)]
    #[derive(Default)]
    struct BasicLimits {
        process_time: i64,
        job_time: i64,
        flags: u32,
        min_working_set: usize,
        max_working_set: usize,
        active_processes: u32,
        affinity: usize,
        priority: u32,
        scheduling: u32,
    }
    #[repr(C)]
    #[derive(Default)]
    struct ExtendedLimits {
        basic: BasicLimits,
        io: [u64; 6],
        process_memory: usize,
        job_memory: usize,
        peak_process_memory: usize,
        peak_job_memory: usize,
    }
    #[repr(C)]
    #[derive(Default)]
    struct ThreadEntry {
        size: u32,
        usage: u32,
        thread_id: u32,
        owner_pid: u32,
        base_priority: i32,
        delta_priority: i32,
        flags: u32,
    }
    #[repr(C)]
    #[derive(Default)]
    struct Accounting {
        times: [i64; 4],
        page_faults: u32,
        total_processes: u32,
        active_processes: u32,
        terminated_processes: u32,
    }
    #[repr(C)]
    struct ProcessEntry {
        size: u32,
        usage: u32,
        pid: u32,
        heap: usize,
        module: u32,
        threads: u32,
        parent: u32,
        priority: i32,
        flags: u32,
        exe: [u16; 260],
    }
    #[link(name = "kernel32")]
    extern "system" {
        fn CreateJobObjectW(attributes: *const c_void, name: *const u16) -> *mut c_void;
        fn SetInformationJobObject(
            job: *mut c_void,
            class: i32,
            information: *const c_void,
            length: u32,
        ) -> i32;
        fn AssignProcessToJobObject(job: *mut c_void, process: *mut c_void) -> i32;
        fn TerminateJobObject(job: *mut c_void, code: u32) -> i32;
        fn QueryInformationJobObject(
            job: *mut c_void,
            class: i32,
            information: *mut c_void,
            length: u32,
            returned: *mut u32,
        ) -> i32;
        fn CloseHandle(handle: *mut c_void) -> i32;
        fn CreateToolhelp32Snapshot(flags: u32, pid: u32) -> *mut c_void;
        fn Thread32First(snapshot: *mut c_void, entry: *mut ThreadEntry) -> i32;
        fn Thread32Next(snapshot: *mut c_void, entry: *mut ThreadEntry) -> i32;
        fn OpenThread(access: u32, inherit: i32, id: u32) -> *mut c_void;
        fn ResumeThread(thread: *mut c_void) -> u32;
        fn Process32FirstW(snapshot: *mut c_void, entry: *mut ProcessEntry) -> i32;
        fn Process32NextW(snapshot: *mut c_void, entry: *mut ProcessEntry) -> i32;
        fn OpenProcess(access: u32, inherit: i32, pid: u32) -> *mut c_void;
        fn GetCurrentProcess() -> *mut c_void;
        fn DuplicateHandle(
            source_process: *mut c_void,
            source: *mut c_void,
            target_process: *mut c_void,
            target: *mut *mut c_void,
            access: u32,
            inherit: i32,
            options: u32,
        ) -> i32;
        fn GetProcessTimes(
            process: *mut c_void,
            created: *mut u64,
            exited: *mut u64,
            kernel: *mut u64,
            user: *mut u64,
        ) -> i32;
        fn WaitForSingleObject(handle: *mut c_void, timeout: u32) -> u32;
    }

    struct Handle(isize);
    impl Handle {
        fn raw(&self) -> *mut c_void {
            self.0 as *mut c_void
        }
        fn times(&self) -> Result<(u64, u64), String> {
            let (mut created, mut exited, mut kernel, mut user) = (0, 0, 0, 0);
            if unsafe {
                GetProcessTimes(
                    self.raw(),
                    &mut created,
                    &mut exited,
                    &mut kernel,
                    &mut user,
                )
            } == 0
            {
                return Err(std::io::Error::last_os_error().to_string());
            }
            Ok((created, exited))
        }
        fn exited(&self) -> Result<bool, String> {
            match unsafe { WaitForSingleObject(self.raw(), 0) } {
                0 => Ok(true),
                258 => Ok(false),
                _ => Err(std::io::Error::last_os_error().to_string()),
            }
        }
    }
    impl Drop for Handle {
        fn drop(&mut self) {
            unsafe {
                CloseHandle(self.raw());
            }
        }
    }
    struct Identity {
        pid: u32,
        created: u64,
        handle: Handle,
    }

    /// A stop-time snapshot. Retained handles bind each PID to its creation time
    /// even if the root exits during taskkill or Windows recycles another PID.
    pub struct CapturedTree {
        identities: Vec<Identity>,
        // An opener failure supplies no creation-time proof. Keep the whole
        // possible subtree until fresh global absence proves it gone.
        unresolved: parking_lot::Mutex<std::collections::HashSet<u32>>,
    }

    type ProcessLinks = Vec<(u32, u32)>;

    fn process_links() -> Result<ProcessLinks, String> {
        let raw = unsafe { CreateToolhelp32Snapshot(2, 0) };
        if raw as isize == -1 {
            return Err(std::io::Error::last_os_error().to_string());
        }
        let snapshot = Handle(raw as isize);
        let mut entry: ProcessEntry = unsafe { std::mem::zeroed() };
        entry.size = std::mem::size_of::<ProcessEntry>() as u32;
        let mut links = Vec::new();
        let mut found = unsafe { Process32FirstW(snapshot.raw(), &mut entry) } != 0;
        if !found {
            return Err(std::io::Error::last_os_error().to_string());
        }
        while found {
            links.push((entry.pid, entry.parent));
            found = unsafe { Process32NextW(snapshot.raw(), &mut entry) } != 0;
        }
        let error = std::io::Error::last_os_error();
        if error.raw_os_error() != Some(18) {
            return Err(error.to_string());
        }
        Ok(links)
    }

    fn open_process(pid: u32) -> Result<Handle, std::io::Error> {
        let raw = unsafe { OpenProcess(0x0010_1000, 0, pid) };
        if raw.is_null() {
            Err(std::io::Error::last_os_error())
        } else {
            Ok(Handle(raw as isize))
        }
    }

    fn include_descendants(pids: &mut std::collections::HashSet<u32>, links: &ProcessLinks) {
        loop {
            let before = pids.len();
            for &(pid, parent) in links {
                if pids.contains(&parent) {
                    pids.insert(pid);
                }
            }
            if pids.len() == before {
                break;
            }
        }
    }

    impl CapturedTree {
        fn root(pid: u32, root: std::os::windows::io::RawHandle) -> Result<Self, String> {
            let mut copied = std::ptr::null_mut();
            let current = unsafe { GetCurrentProcess() };
            if unsafe { DuplicateHandle(current, root, current, &mut copied, 0, 0, 2) } == 0 {
                return Err(std::io::Error::last_os_error().to_string());
            }
            let handle = Handle(copied as isize);
            let (created, _) = handle.times()?;
            Ok(Self {
                identities: vec![Identity {
                    pid,
                    created,
                    handle,
                }],
                unresolved: parking_lot::Mutex::new(std::collections::HashSet::new()),
            })
        }

        pub fn capture(pid: u32, root: std::os::windows::io::RawHandle) -> Result<Self, String> {
            let mut captured = None;
            Self::capture_retaining(&mut captured, pid, root)?;
            Ok(captured.unwrap())
        }

        /// Update in place: a later query failure must not discard identities
        /// already captured before an intermediate parent disappeared.
        pub fn capture_retaining(
            slot: &mut Option<Self>,
            pid: u32,
            root: std::os::windows::io::RawHandle,
        ) -> Result<(), String> {
            if slot.is_none() {
                *slot = Some(Self::root(pid, root)?);
            }
            let tree = slot.as_mut().unwrap();
            if tree.identities[0].pid != pid {
                return Err("PTY capture root identity changed".into());
            }
            tree.refresh(process_links()?, open_process)
        }

        fn refresh(
            &mut self,
            links: ProcessLinks,
            mut opener: impl FnMut(u32) -> Result<Handle, std::io::Error>,
        ) -> Result<(), String> {
            let mut index = 0;
            while index < self.identities.len() {
                let parent_pid = self.identities[index].pid;
                let (parent_created, parent_exited) = self.identities[index].handle.times()?;
                for &(child_pid, _) in links.iter().filter(|(_, owner)| *owner == parent_pid) {
                    if self
                        .identities
                        .iter()
                        .any(|identity| identity.pid == child_pid)
                    {
                        continue;
                    }
                    let handle = match opener(child_pid) {
                        Ok(handle) => handle,
                        Err(_) => {
                            let mut unresolved = self.unresolved.lock();
                            unresolved.insert(child_pid);
                            include_descendants(&mut unresolved, &links);
                            continue;
                        }
                    };
                    let (created, _) = match handle.times() {
                        Ok(times) => times,
                        Err(error) => {
                            let mut unresolved = self.unresolved.lock();
                            unresolved.insert(child_pid);
                            include_descendants(&mut unresolved, &links);
                            return Err(error);
                        }
                    };
                    if created >= parent_created && (parent_exited == 0 || created <= parent_exited)
                    {
                        self.identities.push(Identity {
                            pid: child_pid,
                            created,
                            handle,
                        });
                        self.unresolved.lock().remove(&child_pid);
                    }
                }
                index += 1;
            }
            Ok(())
        }

        pub fn is_empty(&self) -> Result<bool, String> {
            self.is_empty_with_snapshot(process_links)
        }

        fn is_empty_with_snapshot(
            &self,
            snapshot: impl FnOnce() -> Result<ProcessLinks, String>,
        ) -> Result<bool, String> {
            let mut unresolved = self.unresolved.lock();
            if !unresolved.is_empty() {
                let links = snapshot()?;
                // Include new child links before testing absence; an exited
                // unopenable parent cannot hide a surviving orphan.
                include_descendants(&mut unresolved, &links);
                if links.iter().any(|(pid, _)| unresolved.contains(pid)) {
                    return Ok(false);
                }
                // Complete snapshot: no PID of any generation, nor surviving
                // child link. This is absence evidence, not an OpenProcess error.
                unresolved.clear();
            }
            drop(unresolved);
            for identity in &self.identities {
                if identity.handle.times()?.0 != identity.created {
                    return Err("Captured process identity changed".into());
                }
                if !identity.handle.exited()? {
                    return Ok(false);
                }
            }
            Ok(true)
        }
    }
    pub struct ProcessTree(isize);
    impl ProcessTree {
        pub fn attach(child: &std::process::Child) -> Result<Self, String> {
            // The handle belongs solely to this guard; Windows propagates job
            // membership to subsequently created descendants, including npm shims.
            let raw = unsafe { CreateJobObjectW(std::ptr::null(), std::ptr::null()) };
            if raw.is_null() {
                return Err(std::io::Error::last_os_error().to_string());
            }
            let job = Self(raw as isize);
            let mut limits = ExtendedLimits::default();
            limits.basic.flags = 0x2000; // KILL_ON_JOB_CLOSE
            let configured = unsafe {
                SetInformationJobObject(
                    raw,
                    9,
                    &limits as *const _ as *const c_void,
                    std::mem::size_of::<ExtendedLimits>() as u32,
                )
            };
            let assigned = configured != 0
                && unsafe { AssignProcessToJobObject(raw, child.as_raw_handle()) } != 0;
            if !assigned {
                return Err(format!(
                    "Cannot own transport process tree: {}. Select PTY.",
                    std::io::Error::last_os_error()
                ));
            }
            // RpcClient creates the root suspended. Attach before any shim can
            // create an unowned Node child, then resume its primary thread.
            let snapshot = unsafe { CreateToolhelp32Snapshot(4, 0) };
            if snapshot as isize == -1 {
                return Err(std::io::Error::last_os_error().to_string());
            }
            let mut entry = ThreadEntry {
                size: std::mem::size_of::<ThreadEntry>() as u32,
                ..Default::default()
            };
            let mut resumed = false;
            let mut found = unsafe { Thread32First(snapshot, &mut entry) } != 0;
            while found {
                if entry.owner_pid == child.id() {
                    let thread = unsafe { OpenThread(2, 0, entry.thread_id) };
                    if !thread.is_null() {
                        resumed = unsafe { ResumeThread(thread) } != u32::MAX;
                        unsafe {
                            CloseHandle(thread);
                        }
                        if resumed {
                            break;
                        }
                    }
                }
                found = unsafe { Thread32Next(snapshot, &mut entry) } != 0;
            }
            unsafe {
                CloseHandle(snapshot);
            }
            if !resumed {
                return Err("Cannot resume owned transport process; select PTY".into());
            }
            Ok(job)
        }
        pub fn is_empty(&self) -> Result<bool, String> {
            let mut accounting = Accounting::default();
            if unsafe {
                QueryInformationJobObject(
                    self.0 as *mut c_void,
                    1,
                    &mut accounting as *mut _ as *mut c_void,
                    std::mem::size_of::<Accounting>() as u32,
                    std::ptr::null_mut(),
                )
            } == 0
            {
                return Err(std::io::Error::last_os_error().to_string());
            }
            Ok(accounting.active_processes == 0)
        }
        pub fn terminate(&self) -> Result<(), String> {
            if unsafe { TerminateJobObject(self.0 as *mut c_void, 1) } == 0 {
                return Err(std::io::Error::last_os_error().to_string());
            }
            Ok(())
        }
    }
    impl Drop for ProcessTree {
        fn drop(&mut self) {
            unsafe {
                CloseHandle(self.0 as *mut c_void);
            }
        }
    }

    #[cfg(test)]
    mod denied_opener_tests {
        use super::*;
        use crate::transport::process::{stop_child, wait_for_exit, STOP_BUDGET};
        use std::os::windows::process::CommandExt;
        use std::process::{Child, Command, Stdio};
        use std::time::{Duration, Instant};

        struct Fixture {
            root: Child,
            job: ProcessTree,
            temp: tempfile::TempDir,
            child: u32,
        }

        impl Fixture {
            fn new() -> Self {
                let temp = tempfile::tempdir().unwrap();
                let ready = temp.path().join("ready");
                let release = temp.path().join("release");
                let script = format!(
                    "$p=Start-Process ping.exe -ArgumentList '-n 600 127.0.0.1' -PassThru -NoNewWindow; \
                     [IO.File]::WriteAllText('{}',[string]$p.Id); while(!(Test-Path '{}')){{Start-Sleep -Milliseconds 20}}",
                    ready.display(), release.display());
                let root = Command::new("powershell.exe")
                    .args(["-NoProfile", "-NonInteractive", "-Command", &script])
                    .creation_flags(0x0800_0004)
                    .stdin(Stdio::null())
                    .stdout(Stdio::null())
                    .stderr(Stdio::null())
                    .spawn()
                    .unwrap();
                let job = ProcessTree::attach(&root).unwrap();
                let deadline = Instant::now() + Duration::from_secs(120);
                let child = loop {
                    if let Ok(text) = std::fs::read_to_string(&ready) {
                        if let Ok(pid) = text.parse::<u32>() {
                            break pid;
                        }
                    }
                    assert!(
                        Instant::now() < deadline,
                        "fixture failsafe expired waiting for denied-opener child"
                    );
                    std::thread::sleep(Duration::from_millis(20));
                };
                Self {
                    root,
                    job,
                    temp,
                    child,
                }
            }

            fn exit_root(&mut self) {
                std::fs::write(self.temp.path().join("release"), []).unwrap();
                let deadline = Instant::now() + Duration::from_secs(120);
                while self.root.try_wait().unwrap().is_none() {
                    assert!(
                        Instant::now() < deadline,
                        "fixture failsafe expired waiting for root exit"
                    );
                    std::thread::sleep(Duration::from_millis(20));
                }
            }

            fn denied_tree(&self) -> CapturedTree {
                let mut tree =
                    CapturedTree::root(self.root.id(), self.root.as_raw_handle()).unwrap();
                let mut denied = false;
                tree.refresh(process_links().unwrap(), |pid| {
                    if pid == self.child {
                        denied = true;
                    }
                    // Deny every descendant so an independently opened console
                    // host cannot mask a missing unresolved-PID check.
                    Err(std::io::Error::from_raw_os_error(5))
                })
                .unwrap();
                assert!(
                    denied,
                    "regression must exercise an actual injected access denial"
                );
                assert_eq!(tree.identities.len(), 1, "only the root may have an opened handle in this fixture");
                tree
            }
        }

        impl Drop for Fixture {
            fn drop(&mut self) {
                let _ = stop_child(&mut self.root, Some(&self.job));
            }
        }

        #[test]
        fn g53_denied_opener_requires_fresh_absence_not_root_exit() {
            let mut fixture = Fixture::new();
            let tree = fixture.denied_tree();
            fixture.exit_root();
            assert!(
                wait_for_exit(Ok(()), Duration::ZERO, || tree.is_empty()).is_err(),
                "access-denied live descendant must survive root-only exit as unresolved"
            );
            fixture.job.terminate().unwrap();
            wait_for_exit(Ok(()), STOP_BUDGET, || tree.is_empty()).unwrap();
        }

        #[test]
        fn g53_denied_opener_snapshot_error_and_new_orphan_fail_closed() {
            let mut fixture = Fixture::new();
            let tree = fixture.denied_tree();
            fixture.exit_root();
            assert!(tree
                .is_empty_with_snapshot(|| Err("synthetic snapshot failure".into()))
                .is_err());
            assert!(
                !tree
                    .is_empty_with_snapshot(|| Ok(vec![(u32::MAX, fixture.child)]))
                    .unwrap(),
                "an absent denied PID cannot hide a fresh surviving child link"
            );
            assert!(
                tree.unresolved.lock().contains(&u32::MAX),
                "fresh ancestry must persist across probes"
            );
            fixture.job.terminate().unwrap();
            wait_for_exit(Ok(()), STOP_BUDGET, || tree.is_empty()).unwrap();
        }

        #[test]
        fn g53_partial_capture_and_later_successful_open_retain_exact_descendant() {
            let mut fixture = Fixture::new();
            let mut tree = fixture.denied_tree();
            tree.refresh(process_links().unwrap(), open_process)
                .unwrap();
            assert!(tree
                .identities
                .iter()
                .any(|identity| identity.pid == fixture.child));
            tree.refresh(vec![(u32::MAX, fixture.root.id())], |_| {
                Err(std::io::Error::from_raw_os_error(5))
            })
            .unwrap();
            assert!(
                tree.identities
                    .iter()
                    .any(|identity| identity.pid == fixture.child),
                "a later opener failure must retain earlier exact handles"
            );
            fixture.exit_root();
            assert!(!tree.is_empty().unwrap());
            fixture.job.terminate().unwrap();
            wait_for_exit(Ok(()), STOP_BUDGET, || tree.is_empty()).unwrap();
        }
    }
}
#[cfg(windows)]
pub(crate) use windows::CapturedTree;
#[cfg(windows)]
pub use windows::ProcessTree;

#[cfg(unix)]
pub struct ProcessTree(u32);
#[cfg(unix)]
impl ProcessTree {
    /// portable-pty calls setsid before exec. Validate and retain that initial
    /// group while its child is still owned and unreaped; never use tcgetpgrp.
    pub fn for_pty(pid: u32) -> Result<Self, String> {
        extern "C" { fn getpgid(pid: i32) -> i32; }
        let pid_i32 = i32::try_from(pid).map_err(|error| error.to_string())?;
        let group = unsafe { getpgid(pid_i32) };
        if group < 0 { return Err(std::io::Error::last_os_error().to_string()); }
        if group != pid_i32 { return Err("PTY root does not own its initial process group".into()); }
        Ok(Self(pid))
    }
    pub fn attach(child: &std::process::Child) -> Result<Self, String> {
        Ok(Self(child.id()))
    }
    pub fn terminate(&self) -> Result<(), String> {
        extern "C" {
            fn kill(pid: i32, signal: i32) -> i32;
        }
        if unsafe { kill(-(self.0 as i32), 9) } == 0 {
            return Ok(());
        }
        let error = std::io::Error::last_os_error();
        if error.raw_os_error() == Some(3) {
            Ok(())
        } else {
            Err(error.to_string())
        }
    }
    pub fn is_empty(&self) -> Result<bool, String> {
        extern "C" {
            fn kill(pid: i32, signal: i32) -> i32;
        }
        if unsafe { kill(-(self.0 as i32), 0) } == 0 {
            return Ok(false);
        }
        let error = std::io::Error::last_os_error();
        if error.raw_os_error() == Some(3) {
            Ok(true)
        } else {
            Err(error.to_string())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn g53_live_tree_never_succeeds_even_when_termination_reports_success() {
        assert!(wait_for_exit(Ok(()), Duration::ZERO, || Ok(false))
            .unwrap_err()
            .contains("still alive"));
    }

    #[test]
    fn g53_verified_gone_supersedes_late_termination_error() {
        assert_eq!(
            wait_for_exit(Err("helper timed out".into()), Duration::ZERO, || Ok(true)),
            Ok(StopOutcome::VerifiedGone)
        );
        assert_eq!(
            wait_for_exit(Ok(()), Duration::ZERO, || Ok(true)),
            Ok(StopOutcome::Exited)
        );
    }

    #[test]
    fn g53_unknown_liveness_is_not_verified_gone() {
        assert!(wait_for_exit(Ok(()), Duration::ZERO, || Err("access denied".into())).is_err());
    }

    #[test]
    fn g53_expired_deadline_still_requires_a_final_exact_probe() {
        assert!(wait_for_exit_until(Ok(()), Instant::now(), || Ok(false)).is_err());
        assert_eq!(
            wait_for_exit_until(Err("late helper".into()), Instant::now(), || Ok(true)),
            Ok(StopOutcome::VerifiedGone)
        );
    }

    #[cfg(windows)]
    #[test]
    fn g53_captured_tree_and_job_reject_root_only_exit() {
        use std::os::windows::{io::AsRawHandle, process::CommandExt};
        use std::process::{Command, Stdio};
        let temp = tempfile::tempdir().unwrap();
        let ready = temp.path().join("ready");
        let release = temp.path().join("release");
        let script =
            format!(
            "$p=Start-Process ping.exe -ArgumentList '-n 600 127.0.0.1' -PassThru -NoNewWindow; \
             [IO.File]::WriteAllText('{}',[string]$p.Id); \
             while(!(Test-Path '{}')){{Start-Sleep -Milliseconds 20}}",
            ready.display(), release.display());
        let mut root = Command::new("powershell.exe")
            .args(["-NoProfile", "-NonInteractive", "-Command", &script])
            .creation_flags(0x0800_0004)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .unwrap();
        let job = ProcessTree::attach(&root).unwrap();
        let deadline = Instant::now() + Duration::from_secs(120);
        while !ready.exists() {
            assert!(
                root.try_wait().unwrap().is_none(),
                "fixture exited before descendant publication"
            );
            assert!(
                Instant::now() < deadline,
                "fixture failsafe expired waiting for descendant publication"
            );
            std::thread::sleep(Duration::from_millis(20));
        }
        let captured = CapturedTree::capture(root.id(), root.as_raw_handle()).unwrap();
        std::fs::write(&release, []).unwrap();
        while root.try_wait().unwrap().is_none() {
            assert!(
                Instant::now() < deadline,
                "fixture failsafe expired waiting for root exit"
            );
            std::thread::sleep(Duration::from_millis(20));
        }
        assert!(
            !job.is_empty().unwrap(),
            "exited root cannot prove its job empty"
        );
        assert!(
            wait_for_exit(Ok(()), Duration::ZERO, || captured.is_empty()).is_err(),
            "exact captured descendant must prevent root-only success"
        );
        stop_child(&mut root, Some(&job)).unwrap();
        wait_for_exit(Ok(()), STOP_BUDGET, || captured.is_empty()).unwrap();
    }
}
