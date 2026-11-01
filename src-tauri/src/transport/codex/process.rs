//! Keep a process tree owned even after its direct parent exits.
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
        fn CloseHandle(handle: *mut c_void) -> i32;
        fn CreateToolhelp32Snapshot(flags: u32, pid: u32) -> *mut c_void;
        fn Thread32First(snapshot: *mut c_void, entry: *mut ThreadEntry) -> i32;
        fn Thread32Next(snapshot: *mut c_void, entry: *mut ThreadEntry) -> i32;
        fn OpenThread(access: u32, inherit: i32, id: u32) -> *mut c_void;
        fn ResumeThread(thread: *mut c_void) -> u32;
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
                    "Cannot own Codex process tree: {}. Select PTY.",
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
                return Err("Cannot resume owned Codex process; select PTY".into());
            }
            Ok(job)
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
}
#[cfg(windows)]
pub use windows::ProcessTree;

#[cfg(unix)]
pub struct ProcessTree(u32);
#[cfg(unix)]
impl ProcessTree {
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
}
