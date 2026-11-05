//! Keep a process tree owned even after its direct parent exits.
use std::time::{Duration, Instant};

pub(crate) const STOP_BUDGET: Duration = Duration::from_secs(30);

/// Shared deterministic seam for Unix generation ownership and signal delivery.
/// The platform probe must retain the original waitable child until retirement.
#[cfg(any(unix, test))]
fn terminate_unix_group_with(
    retired: &mut bool,
    root_exit: impl FnOnce() -> Result<Option<u32>, String>,
    live_members: impl FnOnce() -> Result<bool, String>,
    signal: impl FnOnce() -> Result<(), String>,
) -> Result<(), String> {
    if *retired { return Ok(()); }
    let exited = root_exit()?.is_some();
    if exited && !live_members()? {
        *retired = true;
        return Ok(());
    }
    signal()
}

pub(crate) fn child_has_exited(child: &mut std::process::Child) -> Result<bool, String> {
    #[cfg(unix)]
    return unix_child_exit_status(child.id()).map(|status| status.is_some());
    #[cfg(windows)]
    return child.try_wait().map(|status| status.is_some()).map_err(|error| error.to_string());
}

// PROC_PGRP_ONLY returns bytes, unlike proc_listpgrppids which returns
// a PID count. Two slots suffice: only the root in a non-full result proves
// emptiness; any second member (or truncation) conservatively stays occupied.
#[cfg(any(target_os = "macos", test))]
fn group_members_present(root: u32, bytes: i32, members: &[i32; 2]) -> Result<bool, String> {
    let width = std::mem::size_of::<i32>() as i32;
    if bytes <= 0 || bytes > 2 * width || bytes % width != 0 {
        return Err("Cannot obtain atomic process-group membership".into());
    }
    let members = &members[..(bytes / width) as usize];
    if members.iter().any(|pid| *pid <= 0) { return Err("Invalid process-group identity".into()); }
    if members.iter().any(|pid| *pid as u32 != root) { return Ok(true); }
    if members.len() == 1 { Ok(false) } else { Err("Ambiguous process-group membership".into()) }
}

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
        #[cfg(unix)]
        let root_gone = if tree.is_some() { child_has_exited(child)? } else {
            child.try_wait().map_err(|error| error.to_string())?.is_some()
        };
        #[cfg(windows)]
        let root_gone = child
            .try_wait()
            .map_err(|error| error.to_string())?
            .is_some();
        let tree_gone = match tree {
            Some(tree) => tree.is_empty()?,
            None => root_gone,
        };
        let gone = root_gone && tree_gone;
        // Keep the original leader allocated until the whole group retires.
        #[cfg(unix)]
        if gone { child.wait().map_err(|error| error.to_string())?; }
        Ok(gone)
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
        fn IsProcessInJob(process: *mut c_void, job: *mut c_void, result: *mut i32) -> i32;
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
        fn TerminateProcess(handle: *mut c_void, code: u32) -> i32;
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
        owned_job: Option<std::sync::Arc<ProcessTree>>,
        // An opener failure supplies no creation-time proof. Keep the whole
        // possible subtree until fresh global absence proves it gone. Each
        // unknown branch inherits the birth-time lower bound of its last exact
        // ancestor, so historical PPID links can still be disproved safely.
        unresolved: parking_lot::Mutex<Unresolved>,
    }

    type ProcessLinks = Vec<(u32, u32)>;
    type Unresolved = std::collections::HashMap<u32, u64>;

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
        // Prefer termination and job-assignment rights, retaining a query-only
        // identity when access is restricted. Verification stays conservative.
        let mut raw = unsafe { OpenProcess(0x0010_1101, 0, pid) };
        if raw.is_null() { raw = unsafe { OpenProcess(0x0010_1000, 0, pid) }; }
        if raw.is_null() {
            Err(std::io::Error::last_os_error())
        } else {
            Ok(Handle(raw as isize))
        }
    }

    fn remember_unresolved(unresolved: &mut Unresolved, pid: u32, earliest: u64) -> bool {
        match unresolved.get_mut(&pid) {
            Some(bound) if earliest < *bound => {
                // Alternative possible ancestry must keep the looser bound.
                *bound = earliest;
                true
            }
            Some(_) => false,
            None => {
                unresolved.insert(pid, earliest);
                true
            }
        }
    }

    fn process_created_with(
        pid: u32,
        opener: &mut impl FnMut(u32) -> Result<Handle, std::io::Error>,
    ) -> Option<u64> {
        opener(pid).ok()?.times().ok().map(|(created, _)| created)
    }

    fn include_descendants(
        unresolved: &mut Unresolved,
        links: &ProcessLinks,
        opener: &mut impl FnMut(u32) -> Result<Handle, std::io::Error>,
    ) {
        let mut created = std::collections::HashMap::new();
        loop {
            let mut changed = false;
            for &(pid, parent) in links {
                let Some(earliest) = unresolved.get(&parent).copied() else {
                    continue;
                };
                if unresolved.contains_key(&pid) {
                    changed |= remember_unresolved(unresolved, pid, earliest);
                    continue;
                }
                let child_created = created
                    .entry(pid)
                    .or_insert_with(|| process_created_with(pid, opener));
                if child_created.is_some_and(|time| time < earliest) {
                    // The queried process predates an exact ancestor. This is
                    // a historical PPID match, not a possible descendant.
                    continue;
                }
                // Failed opens/times remain unknown. Never tighten this bound
                // from a later-opened unknown PID: it could be a new generation
                // with an older genuine orphan still alive.
                changed |= remember_unresolved(unresolved, pid, earliest);
            }
            if !changed {
                break;
            }
        }
    }

    impl CapturedTree {
        #[cfg(test)]
        pub(crate) fn fully_opened_for_test(&self) -> bool {
            self.unresolved.lock().is_empty()
        }

        fn root(pid: u32, root: std::os::windows::io::RawHandle) -> Result<Self, String> {
            let mut copied = std::ptr::null_mut();
            let current = unsafe { GetCurrentProcess() };
            if unsafe { DuplicateHandle(current, root, current, &mut copied, 0, 0, 2) } == 0 {
                return Err(std::io::Error::last_os_error().to_string());
            }
            let handle = Handle(copied as isize);
            let (created, _) = handle.times()?;
            Ok(Self {
                owned_job: None,
                identities: vec![Identity {
                    pid,
                    created,
                    handle,
                }],
                unresolved: parking_lot::Mutex::new(Unresolved::new()),
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
            Self::capture_retaining_owned(slot, pid, root, None)
        }

        pub(crate) fn capture_retaining_owned(
            slot: &mut Option<Self>,
            pid: u32,
            root: std::os::windows::io::RawHandle,
            job: Option<std::sync::Arc<ProcessTree>>,
        ) -> Result<(), String> {
            if slot.is_none() {
                *slot = Some(Self::root(pid, root)?);
            }
            let tree = slot.as_mut().unwrap();
            if let Some(job) = job { tree.owned_job = Some(job); }
            if tree.identities[0].pid != pid {
                return Err("PTY capture root identity changed".into());
            }
            tree.refresh(process_links()?, open_process)?;
            // Include still-identifiable startup children that predate the
            // immediate post-spawn assignment. The lifetime job then tracks
            // their future children even after every PPID link disappears.
            if let Some(job) = &tree.owned_job {
                for identity in &tree.identities {
                    if !identity.handle.exited()? { job.include(identity.handle.raw())?; }
                }
            }
            Ok(())
        }

        pub(crate) fn terminate(&self) -> Result<(), String> {
            let mut errors = Vec::new();
            if let Some(job) = &self.owned_job {
                if let Err(error) = job.terminate() { errors.push(error); }
            }
            // Root-directed taskkill cannot find retained, orphaned identities.
            // These handles still name their exact original generations.
            for identity in &self.identities {
                match identity.handle.exited() {
                    Ok(true) => continue,
                    Err(error) => { errors.push(error); continue; },
                    Ok(false) => {},
                }
                if unsafe { TerminateProcess(identity.handle.raw(), 1) } == 0 {
                    errors.push(std::io::Error::last_os_error().to_string());
                }
            }
            if errors.is_empty() { Ok(()) } else { Err(errors.join("; ")) }
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
                            remember_unresolved(&mut unresolved, child_pid, parent_created);
                            include_descendants(&mut unresolved, &links, &mut opener);
                            continue;
                        }
                    };
                    let (created, _) = match handle.times() {
                        Ok(times) => times,
                        Err(error) => {
                            let mut unresolved = self.unresolved.lock();
                            remember_unresolved(&mut unresolved, child_pid, parent_created);
                            include_descendants(&mut unresolved, &links, &mut opener);
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
                        // A later open may be a reused PID generation. Keep
                        // any earlier unknown ancestry until fresh absence;
                        // replacing its bound could hide an older live orphan.
                    }
                }
                index += 1;
            }
            Ok(())
        }

        #[cfg(test)]
        pub(crate) fn cleanup_state(&self, job: &ProcessTree) -> String {
            let unresolved = self.unresolved.lock().clone();
            let identities: Vec<_> = self
                .identities
                .iter()
                .map(|identity| {
                    let mut member = 0;
                    let queried = unsafe {
                        IsProcessInJob(identity.handle.raw(), job.0 as *mut c_void, &mut member)
                    };
                    (
                        identity.pid,
                        identity.created,
                        identity.handle.times(),
                        identity.handle.exited(),
                        queried,
                        member,
                    )
                })
                .collect();
            let unknown: Vec<_> = unresolved
                .iter()
                .map(|(pid, earliest)| {
                    (
                        *pid,
                        *earliest,
                        open_process(*pid).map(|handle| (handle.times(), handle.exited())),
                    )
                })
                .collect();
            let links: Result<Vec<_>, _> = process_links().map(|links| {
                links
                    .into_iter()
                    .filter(|(pid, parent)| {
                        unresolved.contains_key(pid) || unresolved.contains_key(parent)
                    })
                    .collect()
            });
            format!("identities={identities:?}; unresolved={unknown:?}; links={links:?}; job_empty={:?}", job.is_empty())
        }

        pub fn is_empty(&self) -> Result<bool, String> {
            if let Some(job) = &self.owned_job {
                if !job.is_empty()? { return Ok(false); }
            }
            self.is_empty_with_snapshot(process_links)
        }

        fn is_empty_with_snapshot(
            &self,
            snapshot: impl FnOnce() -> Result<ProcessLinks, String>,
        ) -> Result<bool, String> {
            self.is_empty_with_sources(snapshot, open_process)
        }

        fn is_empty_with_sources(
            &self,
            snapshot: impl FnOnce() -> Result<ProcessLinks, String>,
            mut opener: impl FnMut(u32) -> Result<Handle, std::io::Error>,
        ) -> Result<bool, String> {
            let mut unresolved = self.unresolved.lock();
            if !unresolved.is_empty() {
                let links = snapshot()?;
                // Include new child links before testing absence; an exited
                // unopenable parent cannot hide a surviving orphan.
                unresolved.retain(|pid, earliest| {
                    process_created_with(*pid, &mut opener)
                        .map_or(true, |created| created >= *earliest)
                });
                include_descendants(&mut unresolved, &links, &mut opener);
                if links.iter().any(|(pid, _)| unresolved.contains_key(pid)) {
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
        fn include(&self, process: std::os::windows::io::RawHandle) -> Result<(), String> {
            let mut member = 0;
            if unsafe { IsProcessInJob(process, self.0 as *mut c_void, &mut member) } == 0 {
                return Err(std::io::Error::last_os_error().to_string());
            }
            if member != 0 || unsafe { AssignProcessToJobObject(self.0 as *mut c_void, process) } != 0 {
                return Ok(());
            }
            Err(std::io::Error::last_os_error().to_string())
        }
        pub fn attach(child: &std::process::Child) -> Result<Self, String> {
            let job = Self::attach_running(child.as_raw_handle())?;
            Self::resume(child.id())?;
            Ok(job)
        }

        /// Assign an already running PTY without changing its launch contract.
        /// This owns future descendants; children born before assignment need
        /// the retained stop-time capture as well.
        pub(crate) fn attach_running(process: std::os::windows::io::RawHandle) -> Result<Self, String> {
            let job = Self::new()?;
            job.assign_suspended(process)?;
            Ok(job)
        }

        /// Create the lifetime owner before creating the suspended ConPTY root.
        pub(crate) fn new() -> Result<Self, String> {
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
            if configured == 0 {
                return Err(format!(
                    "Cannot own transport process tree: {}. Select PTY.",
                    std::io::Error::last_os_error()
                ));
            }
            Ok(job)
        }

        /// The caller retains the primary thread and resumes only after success.
        pub(crate) fn assign_suspended(&self, process: std::os::windows::io::RawHandle) -> Result<(), String> {
            self.include(process).map_err(|error| format!(
                "Cannot own transport process tree: {error}. Select PTY."
            ))
        }

        fn resume(pid: u32) -> Result<(), String> {
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
                if entry.owner_pid == pid {
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
            Ok(())
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
        use crate::transport::process::tests::{assert_fixture_image, fixture_powershell};
        use crate::transport::process::{stop_child, wait_for_exit, STOP_BUDGET};
        use std::os::windows::process::CommandExt;
        use std::process::{Child, Command, Stdio};
        use std::time::{Duration, Instant};

        struct Fixture {
            root: Child,
            job: ProcessTree,
            temp: tempfile::TempDir,
            child: u32,
            // Keep this generation allocated during synthetic denial and job
            // cleanup. This handle is deliberately outside CapturedTree.
            _child_handle: Handle,
        }

        impl Fixture {
            fn new() -> Self {
                Self::with_late_orphan(false)
            }

            fn with_late_orphan(late_orphan: bool) -> Self {
                let temp = tempfile::tempdir().unwrap();
                let ready = temp.path().join("ready");
                let release = temp.path().join("release");
                let child_command = if late_orphan {
                    let child_script = temp.path().join("child.ps1");
                    std::fs::write(
                        &child_script,
                        format!(
                            "while(!(Test-Path '{}')){{Start-Sleep -Milliseconds 20}}; \
                             $p=Start-Process ping.exe -ArgumentList '-n 600 127.0.0.1' -PassThru -NoNewWindow; \
                             [IO.File]::WriteAllText('{}',[string]$p.Id); \
                             while(!(Test-Path '{}')){{Start-Sleep -Milliseconds 20}}",
                            temp.path().join("spawn-orphan").display(),
                            temp.path().join("orphan-ready").display(),
                            temp.path().join("release-child").display(),
                        ),
                    )
                    .unwrap();
                    format!(
                        "Start-Process '{}' -ArgumentList '-NoProfile -NonInteractive -ExecutionPolicy Bypass -File \"{}\"' -PassThru -NoNewWindow",
                        fixture_powershell().replace('\'', "''"),
                        child_script.display(),
                    )
                } else {
                    "Start-Process ping.exe -ArgumentList '-n 600 127.0.0.1' -PassThru -NoNewWindow"
                        .into()
                };
                let script = format!(
                    "$p={child_command}; \
                     [IO.File]::WriteAllText('{}',[string]$p.Id); while(!(Test-Path '{}')){{Start-Sleep -Milliseconds 20}}",
                    ready.display(), release.display());
                let root = Command::new(fixture_powershell())
                    .args(["-NoProfile", "-NonInteractive", "-Command", &script])
                    .creation_flags(0x0800_0004)
                    .stdin(Stdio::null())
                    .stdout(Stdio::null())
                    .stderr(Stdio::null())
                    .spawn()
                    .unwrap();
                let job = ProcessTree::attach(&root).unwrap();
                assert_fixture_image(root.id());
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
                if late_orphan {
                    assert_fixture_image(child);
                }
                Self {
                    root,
                    job,
                    temp,
                    child,
                    _child_handle: open_process(child).unwrap(),
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

            fn spawn_orphan(&self) -> u32 {
                std::fs::write(self.temp.path().join("spawn-orphan"), []).unwrap();
                let deadline = Instant::now() + Duration::from_secs(120);
                loop {
                    if let Ok(text) = std::fs::read_to_string(self.temp.path().join("orphan-ready"))
                    {
                        if let Ok(pid) = text.parse::<u32>() {
                            return pid;
                        }
                    }
                    assert!(
                        Instant::now() < deadline,
                        "fixture failsafe expired waiting for orphan"
                    );
                    std::thread::sleep(Duration::from_millis(20));
                }
            }

            fn denied_tree(&self) -> CapturedTree {
                self.denied_tree_with_links(process_links().unwrap())
            }

            fn denied_tree_with_links(&self, links: ProcessLinks) -> CapturedTree {
                let mut tree =
                    CapturedTree::root(self.root.id(), self.root.as_raw_handle()).unwrap();
                let mut denied = false;
                let branch = (self.child, self.root.id());
                assert!(
                    links.contains(&branch),
                    "the injected denial must target the fixture's real child link"
                );
                // Isolate the known fixture branch. Global PPIDs can refer to a
                // previous generation of the root PID; denying every apparent
                // child would suppress refresh's creation-time rejection and
                // retain unrelated processes that the fixture job cannot stop.
                // Keep only the root handle so no other opened descendant can
                // mask a missing unresolved-PID check. Exit probes stay global.
                let fixture_links = links.into_iter().filter(|link| *link == branch).collect();
                tree.refresh(fixture_links, |pid| {
                    assert_eq!(pid, self.child);
                    denied = true;
                    Err(std::io::Error::from_raw_os_error(5))
                })
                .unwrap();
                assert!(
                    denied,
                    "regression must exercise an actual injected access denial"
                );
                assert_eq!(
                    tree.identities.len(),
                    1,
                    "only the root may have an opened handle in this fixture"
                );
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
            wait_for_exit(Ok(()), STOP_BUDGET, || tree.is_empty())
                .unwrap_or_else(|error| panic!("{error}; {}", tree.cleanup_state(&fixture.job)));
        }

        #[test]
        fn g53_denied_opener_snapshot_error_and_new_orphan_fail_closed() {
            let mut fixture = Fixture::with_late_orphan(true);
            let tree = fixture.denied_tree();
            let denied_parent = open_process(fixture.child).unwrap();
            fixture.exit_root();
            assert!(tree
                .is_empty_with_snapshot(|| Err("synthetic snapshot failure".into()))
                .is_err());

            // The denied parent creates a real descendant after capture and
            // root exit, then exits itself. Its PPID link must still reveal the
            // surviving orphan even though neither ancestor is in the snapshot.
            let orphan = fixture.spawn_orphan();
            let orphan_handle = open_process(orphan).unwrap();
            std::fs::write(fixture.temp.path().join("release-child"), []).unwrap();
            wait_for_exit(Ok(()), STOP_BUDGET, || denied_parent.exited()).unwrap();
            let links = process_links().unwrap();
            assert!(!links.iter().any(|(pid, _)| *pid == fixture.child));
            assert!(links.contains(&(orphan, fixture.child)));
            assert!(!orphan_handle.exited().unwrap());
            assert!(
                !tree.is_empty_with_snapshot(|| Ok(links)).unwrap(),
                "an absent denied PID cannot hide a fresh surviving child link"
            );
            assert!(
                tree.unresolved.lock().contains_key(&orphan),
                "fresh ancestry must persist across probes"
            );
            assert!(
                wait_for_exit(Ok(()), Duration::ZERO, || tree.is_empty()).is_err(),
                "the real orphan must prevent assumed-empty success"
            );
            fixture.job.terminate().unwrap();
            wait_for_exit(Ok(()), STOP_BUDGET, || fixture.job.is_empty()).unwrap();
            wait_for_exit(Ok(()), STOP_BUDGET, || tree.is_empty())
                .unwrap_or_else(|error| panic!("{error}; {}", tree.cleanup_state(&fixture.job)));
            assert!(orphan_handle.exited().unwrap());
        }

        #[test]
        fn g53_denied_opener_does_not_deny_historical_parent_links() {
            let mut fixture = Fixture::new();
            let unrelated = std::process::id();
            let mut tree = fixture.denied_tree_with_links(vec![
                (fixture.child, fixture.root.id()),
                (unrelated, fixture.root.id()),
            ]);
            assert!(
                !tree.unresolved.lock().contains_key(&unrelated),
                "synthetic denial must not include historical PPID matches outside the fixture"
            );
            let unrelated_handle = open_process(unrelated).unwrap();
            assert!(unrelated_handle.times().unwrap().0 < tree.identities[0].created);
            // A historical PPID match is possible after PID reuse. Let the
            // production opener disprove it; it is not an injected denial.
            tree.refresh(vec![(unrelated, fixture.root.id())], open_process)
                .unwrap();
            assert!(!tree
                .identities
                .iter()
                .any(|identity| identity.pid == unrelated));
            assert!(!tree.unresolved.lock().contains_key(&unrelated));
            assert!(tree.unresolved.lock().contains_key(&fixture.child));
            fixture.exit_root();
            assert!(wait_for_exit(Ok(()), Duration::ZERO, || tree.is_empty()).is_err());
            fixture.job.terminate().unwrap();
            wait_for_exit(Ok(()), STOP_BUDGET, || tree.is_empty())
                .unwrap_or_else(|error| panic!("{error}; {}", tree.cleanup_state(&fixture.job)));
            assert!(!unrelated_handle.exited().unwrap());
        }

        #[test]
        fn g53_unresolved_capture_rejects_historical_descendant_links() {
            let mut fixture = Fixture::new();
            let unrelated = std::process::id();
            let mut tree =
                CapturedTree::root(fixture.root.id(), fixture.root.as_raw_handle()).unwrap();
            assert!(
                open_process(unrelated).unwrap().times().unwrap().0 < tree.identities[0].created
            );
            tree.refresh(
                vec![
                    (fixture.child, fixture.root.id()),
                    (unrelated, fixture.child),
                ],
                |pid| {
                    if pid == fixture.child {
                        Err(std::io::Error::from_raw_os_error(5))
                    } else {
                        open_process(pid)
                    }
                },
            )
            .unwrap();
            assert!(tree.unresolved.lock().contains_key(&fixture.child));
            assert!(
                !tree.unresolved.lock().contains_key(&unrelated),
                "unresolved capture must reject a process older than its exact ancestor"
            );
            fixture.exit_root();
            assert!(wait_for_exit(Ok(()), Duration::ZERO, || tree.is_empty()).is_err());
            fixture.job.terminate().unwrap();
            wait_for_exit(Ok(()), STOP_BUDGET, || tree.is_empty())
                .unwrap_or_else(|error| panic!("{error}; {}", tree.cleanup_state(&fixture.job)));
        }

        #[test]
        fn g53_unresolved_probe_rejects_historical_links_and_rechecks_old_entries() {
            let mut fixture = Fixture::new();
            let tree = fixture.denied_tree();
            let unrelated = std::process::id();
            let earliest = tree.identities[0].created;
            // A prior failed query leaves this entry unknown. A later query can
            // disprove it, but a newer generation must never tighten the bound.
            tree.unresolved.lock().insert(unrelated, earliest);
            fixture.exit_root();
            fixture.job.terminate().unwrap();
            wait_for_exit(Ok(()), STOP_BUDGET, || fixture.job.is_empty()).unwrap();
            wait_for_exit(Ok(()), STOP_BUDGET, || {
                tree.is_empty_with_snapshot(|| {
                    let mut links = process_links()?;
                    links.push((unrelated, fixture.child));
                    Ok(links)
                })
            })
            .unwrap();
            assert!(!open_process(unrelated).unwrap().exited().unwrap());
            assert!(tree.unresolved.lock().is_empty());
        }

        #[test]
        fn g53_unresolved_identity_errors_remain_unknown() {
            let mut fixture = Fixture::new();
            let tree = fixture.denied_tree();
            let unrelated = std::process::id();
            fixture.exit_root();
            fixture.job.terminate().unwrap();
            wait_for_exit(Ok(()), STOP_BUDGET, || fixture.job.is_empty()).unwrap();
            let links = vec![(unrelated, fixture.child)];
            assert!(!tree
                .is_empty_with_sources(
                    || Ok(links.clone()),
                    |_| { Err(std::io::Error::from_raw_os_error(5)) }
                )
                .unwrap());
            assert!(tree.unresolved.lock().contains_key(&unrelated));
            assert!(!tree
                .is_empty_with_sources(|| Ok(links), |_| Ok(Handle(0)))
                .unwrap());
            assert!(tree.unresolved.lock().contains_key(&unrelated));
            wait_for_exit(Ok(()), STOP_BUDGET, || tree.is_empty())
                .unwrap_or_else(|error| panic!("{error}; {}", tree.cleanup_state(&fixture.job)));
        }

        #[test]
        fn g53_unresolved_bounds_inherit_and_merge_without_tightening() {
            let pid = std::process::id();
            let created = open_process(pid).unwrap().times().unwrap().0;
            let mut unresolved = Unresolved::from([(u32::MAX, created)]);
            include_descendants(&mut unresolved, &vec![(pid, u32::MAX)], &mut open_process);
            assert_eq!(unresolved.get(&pid), Some(&created));
            assert!(!remember_unresolved(&mut unresolved, pid, created + 1));
            assert!(remember_unresolved(&mut unresolved, pid, created - 1));
            assert_eq!(unresolved.get(&pid), Some(&(created - 1)));
        }

        #[test]
        fn g53_later_opened_generation_cannot_hide_an_older_orphan() {
            let mut fixture = Fixture::with_late_orphan(true);
            let mut tree = fixture.denied_tree();
            let orphan = fixture.spawn_orphan();
            let orphan_handle = open_process(orphan).unwrap();
            let mut replacement = Fixture::new();
            assert!(
                orphan_handle.times().unwrap().0 < replacement._child_handle.times().unwrap().0
            );
            let earliest = tree.unresolved.lock()[&fixture.child];
            // Model a PID recycled between snapshot and open: the returned
            // handle belongs to a real newer process, while the old generation
            // has a genuine older orphan. The injected opener controls this race.
            tree.refresh(
                vec![(fixture.child, fixture.root.id()), (orphan, fixture.child)],
                |pid| {
                    open_process(if pid == fixture.child {
                        replacement.child
                    } else {
                        pid
                    })
                },
            )
            .unwrap();
            assert_eq!(tree.unresolved.lock().get(&fixture.child), Some(&earliest));
            assert!(!tree
                .identities
                .iter()
                .any(|identity| identity.pid == orphan));
            std::fs::write(fixture.temp.path().join("release-child"), []).unwrap();
            wait_for_exit(Ok(()), STOP_BUDGET, || fixture._child_handle.exited()).unwrap();
            fixture.exit_root();
            stop_child(&mut replacement.root, Some(&replacement.job)).unwrap();
            wait_for_exit(Ok(()), STOP_BUDGET, || replacement._child_handle.exited()).unwrap();
            assert!(!orphan_handle.exited().unwrap());
            assert!(
                wait_for_exit(Ok(()), Duration::ZERO, || tree.is_empty()).is_err(),
                "a later opened generation cannot retire unresolved old-generation ancestry"
            );
            assert!(tree.unresolved.lock().contains_key(&orphan));
            fixture.job.terminate().unwrap();
            wait_for_exit(Ok(()), STOP_BUDGET, || tree.is_empty())
                .unwrap_or_else(|error| panic!("{error}; {}", tree.cleanup_state(&fixture.job)));
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
            wait_for_exit(Ok(()), STOP_BUDGET, || tree.is_empty())
                .unwrap_or_else(|error| panic!("{error}; {}", tree.cleanup_state(&fixture.job)));
        }
    }
}
#[cfg(windows)]
pub(crate) use windows::CapturedTree;
#[cfg(windows)]
pub use windows::ProcessTree;

#[cfg(unix)]
pub struct ProcessTree {
    pid: u32,
    retired: parking_lot::Mutex<bool>,
    #[cfg(test)]
    signal_observer: parking_lot::Mutex<Option<std::sync::Arc<std::sync::atomic::AtomicUsize>>>,
}

/// Observe exit without freeing the root PID. An unreaped group leader reserves
/// its PID/PGID even after natural exit, so a later stop cannot hit a new group.
#[cfg(unix)]
pub(crate) fn unix_child_exit_status(pid: u32) -> Result<Option<u32>, String> {
    let mut info: libc::siginfo_t = unsafe { std::mem::zeroed() };
    let result = unsafe { libc::waitid(libc::P_PID, pid as libc::id_t, &mut info,
        libc::WEXITED | libc::WNOHANG | libc::WNOWAIT) };
    if result < 0 { return Err(std::io::Error::last_os_error().to_string()); }
    if unsafe { info.si_pid() } == 0 { return Ok(None); }
    let status = unsafe { info.si_status() };
    Ok(Some(if info.si_code == libc::CLD_EXITED { status as u32 } else { (128 + status) as u32 }))
}

#[cfg(unix)]
impl ProcessTree {
    fn new(pid: u32) -> Self {
        Self { pid, retired: parking_lot::Mutex::new(false),
            #[cfg(test)]
            signal_observer: parking_lot::Mutex::new(None),
        }
    }
    /// portable-pty calls setsid before exec. Validate the initial group before
    /// any natural-exit observer can reap the owned child.
    pub fn for_pty(pid: u32) -> Result<Self, String> {
        let group = unsafe { libc::getpgid(pid as i32) };
        if group < 0 { return Err(std::io::Error::last_os_error().to_string()); }
        if group != pid as i32 { return Err("PTY root does not own its initial process group".into()); }
        Ok(Self::new(pid))
    }
    pub fn attach(child: &std::process::Child) -> Result<Self, String> {
        Ok(Self::new(child.id()))
    }

    /// The kernel copies group PID membership under one proc_list_lock.
    /// Exclude only our known waitable leader; all other PIDs, even zombies,
    /// prevent retirement. Unlike ps, no per-PID query can omit a late child.
    #[cfg(target_os = "macos")]
    fn live_group_members(&self) -> Result<bool, String> {
        let mut members: [libc::pid_t; 2] = [0; 2];
        let bytes = unsafe { libc::proc_listpids(2, self.pid,
            members.as_mut_ptr().cast(), std::mem::size_of_val(&members) as i32) };
        group_members_present(self.pid, bytes, &members)
    }

    #[cfg(not(target_os = "macos"))]
    fn live_group_members(&self) -> Result<bool, String> {
        // Windows and macOS are the shipped targets. Other Unix kernels need
        // their own atomic membership authority; never infer emptiness from ps.
        Err("Atomic process-group verification unavailable on this Unix platform; refusing unverified cleanup".into())
    }
    pub fn terminate(&self) -> Result<(), String> {
        let mut retired = self.retired.lock();
        terminate_unix_group_with(&mut retired,
            || unix_child_exit_status(self.pid),
            || self.live_group_members(),
            || {
                #[cfg(test)]
                if let Some(observer) = self.signal_observer.lock().as_ref() {
                    observer.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                    return Err("synthetic replacement group observed a signal".into());
                }
                if unsafe { libc::kill(-(self.pid as i32), libc::SIGKILL) } == 0 { return Ok(()); }
                let error = std::io::Error::last_os_error();
                if error.raw_os_error() == Some(libc::ESRCH) { Ok(()) } else { Err(error.to_string()) }
            })
    }

    pub fn is_empty(&self) -> Result<bool, String> {
        let mut retired = self.retired.lock();
        if *retired { return Ok(true); }
        if unix_child_exit_status(self.pid)?.is_none() { return Ok(false); }
        if self.live_group_members()? { return Ok(false); }
        // No live member can fork after this snapshot; retire before reaping.
        *retired = true;
        Ok(true)
    }

    #[cfg(test)]
    pub(crate) fn observe_reused_generation(&self, observer: std::sync::Arc<std::sync::atomic::AtomicUsize>) {
        *self.signal_observer.lock() = Some(observer);
    }
}
#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(windows)]
    pub(super) fn fixture_powershell() -> String {
        let path = std::path::PathBuf::from(std::env::var_os("SystemRoot").expect("SystemRoot"))
            .join(r"System32\WindowsPowerShell\v1.0\powershell.exe");
        assert!(path.is_absolute() && path.is_file(), "fixture shell must be an absolute system executable");
        path.to_string_lossy().into_owned()
    }

    #[cfg(windows)]
    pub(super) fn assert_fixture_image(pid: u32) {
        use std::{ffi::c_void, os::windows::ffi::OsStringExt};
        #[link(name = "kernel32")]
        extern "system" {
            fn OpenProcess(access: u32, inherit: i32, id: u32) -> *mut c_void;
            fn QueryFullProcessImageNameW(handle: *mut c_void, flags: u32, image: *mut u16, size: *mut u32) -> i32;
            fn CloseHandle(handle: *mut c_void) -> i32;
        }
        let handle = unsafe { OpenProcess(0x1000, 0, pid) };
        assert!(!handle.is_null(), "fixture process must be queryable");
        let mut image = vec![0u16; 32768];
        let mut size = image.len() as u32;
        let queried = unsafe { QueryFullProcessImageNameW(handle, 0, image.as_mut_ptr(), &mut size) };
        unsafe { CloseHandle(handle); }
        assert_ne!(queried, 0, "fixture image query failed");
        let actual = std::path::PathBuf::from(std::ffi::OsString::from_wide(&image[..size as usize]));
        assert_eq!(actual.canonicalize().unwrap(), std::path::Path::new(&fixture_powershell()).canonicalize().unwrap());
    }

    fn reused_group_stop(signals: &std::sync::atomic::AtomicUsize) -> Result<(), String> {
        terminate_unix_group_with(&mut false,
            || Err("ECHILD: original root was reaped; numeric PGID now names replacement".into()),
            || Ok(true),
            || { signals.fetch_add(1, std::sync::atomic::Ordering::SeqCst); Ok(()) })
    }

    #[test]
    fn g53_unix_first_stop_after_natural_exit_never_signals_reused_group() {
        let signals = std::sync::atomic::AtomicUsize::new(0);
        let _ = reused_group_stop(&signals);
        assert_eq!(signals.load(std::sync::atomic::Ordering::SeqCst), 0,
            "first stop must reject lost generation authority before signalling");
    }

    #[test]
    fn g53_unix_drop_after_natural_exit_never_signals_reused_group() {
        struct Cleanup<'a>(&'a std::sync::atomic::AtomicUsize);
        impl Drop for Cleanup<'_> {
            fn drop(&mut self) { let _ = reused_group_stop(self.0); }
        }
        let signals = std::sync::atomic::AtomicUsize::new(0);
        drop(Cleanup(&signals));
        assert_eq!(signals.load(std::sync::atomic::Ordering::SeqCst), 0,
            "Drop must reject lost generation authority before signalling");
    }

    #[test]
    fn g53_unix_retired_group_never_signals_after_leader_reap() {
        let mut retired = false;
        terminate_unix_group_with(&mut retired, || Ok(Some(0)), || Ok(false),
            || panic!("empty original group needs no signal")).unwrap();
        assert!(retired);
        // The caller may now reap the leader and a replacement may reuse PGID.
        terminate_unix_group_with(&mut retired,
            || panic!("retired group must not inspect a replacement generation"),
            || panic!("retired group must not inspect replacement membership"),
            || panic!("retired group must never be signalled again")).unwrap();
    }

    #[test]
    fn g53_atomic_group_snapshot_retires_only_the_reserved_root() {
        assert_eq!(group_members_present(42, 4, &[42, 0]), Ok(false));
        assert_eq!(group_members_present(42, 8, &[42, 43]), Ok(true));
        assert_eq!(group_members_present(42, 8, &[43, 44]), Ok(true));
        assert!(group_members_present(42, 0, &[0, 0]).is_err());
        assert!(group_members_present(42, 3, &[42, 0]).is_err());
        assert!(group_members_present(42, 12, &[42, 43]).is_err());
        assert!(group_members_present(42, 4, &[0, 0]).is_err());
    }

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
        let mut root = Command::new(fixture_powershell())
            .args(["-NoProfile", "-NonInteractive", "-Command", &script])
            .creation_flags(0x0800_0004)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .unwrap();
        let job = ProcessTree::attach(&root).unwrap();
        assert_fixture_image(root.id());
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
        wait_for_exit(Ok(()), STOP_BUDGET, || captured.is_empty())
            .unwrap_or_else(|error| panic!("{error}; {}", captured.cleanup_state(&job)));
    }
}
