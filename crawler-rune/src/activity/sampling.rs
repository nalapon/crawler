//! The only libproc boundary. All reads and identity checks run on a worker.
use std::{
    collections::HashMap,
    ffi::CStr,
    mem::{size_of, zeroed},
    time::Instant,
};

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct ProcessId {
    pub pid: i32,
    pub started: u64,
}
impl ProcessId {
    pub fn key(&self) -> String {
        format!("pid:{}:{}", self.pid, self.started)
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Process {
    pub id: ProcessId,
    pub parent: i32,
    pub name: String,
    pub executable: String,
    pub memory: Option<f64>,
    pub cpu: Option<f64>,
    pub read: Option<f64>,
    pub written: Option<f64>,
    pub threads: Option<u32>,
}
#[derive(Clone)]
pub struct Counters {
    started: u64,
    cpu: u64,
    read: u64,
    written: u64,
    at: Instant,
}
#[derive(Default)]
pub struct Sample {
    pub processes: Vec<Process>,
    pub counters: HashMap<i32, Counters>,
    pub unavailable: usize,
    pub error: Option<String>,
    pub milliseconds: f64,
}

pub fn read(previous: HashMap<i32, Counters>) -> Sample {
    let began = Instant::now();
    let mut sample = Sample::default();
    // SAFETY: libproc writes only to the sized, initialized buffers passed here.
    unsafe {
        let mut timebase = mach2::mach_time::mach_timebase_info { numer: 0, denom: 0 };
        if mach2::mach_time::mach_timebase_info(&mut timebase) != 0 || timebase.denom == 0 {
            sample.error = Some("Unable to read the CPU clock.".into());
            return sample;
        }
        let seconds_per_tick = timebase.numer as f64 / timebase.denom as f64 / 1e9;
        let count = libc::proc_listallpids(std::ptr::null_mut(), 0);
        if count <= 0 {
            sample.error = Some(format!(
                "Unable to read processes: {}",
                std::io::Error::last_os_error()
            ));
            return sample;
        }
        let mut pids = vec![0_i32; count as usize + 256];
        let count = libc::proc_listallpids(
            pids.as_mut_ptr().cast(),
            (pids.len() * size_of::<i32>()) as i32,
        );
        if count <= 0 {
            sample.error = Some("The process list could not be sampled.".into());
            return sample;
        }
        for &pid in pids.iter().take(count as usize).filter(|&&p| p > 0) {
            let mut info: libc::proc_bsdinfo = zeroed();
            if libc::proc_pidinfo(
                pid,
                libc::PROC_PIDTBSDINFO,
                0,
                (&mut info as *mut libc::proc_bsdinfo).cast(),
                size_of::<libc::proc_bsdinfo>() as i32,
            ) != size_of::<libc::proc_bsdinfo>() as i32
            {
                let error = std::io::Error::last_os_error().raw_os_error();
                if matches!(error, Some(libc::EPERM | libc::EACCES)) {
                    sample.unavailable += 1;
                    sample.processes.push(Process {
                        id: ProcessId { pid, started: 0 },
                        parent: 0,
                        name: format!("PID {pid}"),
                        executable: String::new(),
                        memory: None,
                        cpu: None,
                        read: None,
                        written: None,
                        threads: None,
                    });
                }
                continue;
            }
            let decode = |bytes: &[libc::c_char]| {
                String::from_utf8_lossy(
                    &bytes
                        .iter()
                        .take_while(|&&v| v != 0)
                        .map(|&v| v as u8)
                        .collect::<Vec<_>>(),
                )
                .into_owned()
            };
            let name = decode(&info.pbi_name);
            let name = if name.is_empty() {
                decode(&info.pbi_comm)
            } else {
                name
            };
            let mut path = vec![0_u8; 4096];
            let length = libc::proc_pidpath(pid, path.as_mut_ptr().cast(), path.len() as u32);
            let executable = if length > 0 {
                CStr::from_ptr(path.as_ptr().cast())
                    .to_string_lossy()
                    .into_owned()
            } else {
                String::new()
            };
            let started = info.pbi_start_tvsec * 1_000_000 + info.pbi_start_tvusec;
            let mut usage: libc::rusage_info_v4 = zeroed();
            // This historical void** API receives the rusage struct itself, not a pointer slot.
            let readable = libc::proc_pid_rusage(
                pid,
                libc::RUSAGE_INFO_V4,
                (&mut usage as *mut libc::rusage_info_v4).cast(),
            ) == 0;
            let mut task: libc::proc_taskinfo = zeroed();
            let threads = (libc::proc_pidinfo(
                pid,
                libc::PROC_PIDTASKINFO,
                0,
                (&mut task as *mut libc::proc_taskinfo).cast(),
                size_of::<libc::proc_taskinfo>() as i32,
            ) == size_of::<libc::proc_taskinfo>() as i32)
                .then_some(task.pti_threadnum as u32);
            let (mut cpu, mut read, mut written) = (None, None, None);
            if readable {
                let next = Counters {
                    started,
                    cpu: usage.ri_user_time.saturating_add(usage.ri_system_time),
                    read: usage.ri_diskio_bytesread,
                    written: usage.ri_diskio_byteswritten,
                    at: Instant::now(),
                };
                if let Some(old) = previous.get(&pid).filter(|o| o.started == started) {
                    let elapsed = next.at.duration_since(old.at).as_secs_f64();
                    if elapsed > 0.0 {
                        cpu = next
                            .cpu
                            .checked_sub(old.cpu)
                            .map(|v| v as f64 * seconds_per_tick / elapsed * 100.0);
                        read = next.read.checked_sub(old.read).map(|v| v as f64 / elapsed);
                        written = next
                            .written
                            .checked_sub(old.written)
                            .map(|v| v as f64 / elapsed);
                    }
                }
                sample.counters.insert(pid, next);
            } else {
                sample.unavailable += 1;
            }
            sample.processes.push(Process {
                id: ProcessId { pid, started },
                parent: info.pbi_ppid as i32,
                name,
                executable,
                memory: readable.then_some(usage.ri_phys_footprint as f64),
                cpu,
                read,
                written,
                threads,
            });
        }
    }
    sample.milliseconds = began.elapsed().as_secs_f64() * 1000.0;
    sample
}

pub fn terminate(id: ProcessId, force: bool) -> Result<(), String> {
    if id.pid <= 1 || id.started == 0 {
        return Err("The identity of this process cannot be verified.".into());
    }
    // SAFETY: recheck start time immediately before signalling, guarding against reused PIDs.
    unsafe {
        let mut info: libc::proc_bsdinfo = zeroed();
        if libc::proc_pidinfo(
            id.pid,
            libc::PROC_PIDTBSDINFO,
            0,
            (&mut info as *mut libc::proc_bsdinfo).cast(),
            size_of::<libc::proc_bsdinfo>() as i32,
        ) != size_of::<libc::proc_bsdinfo>() as i32
        {
            return Err(std::io::Error::last_os_error().to_string());
        }
        if info.pbi_start_tvsec * 1_000_000 + info.pbi_start_tvusec != id.started {
            return Err("This process has exited. Its PID now belongs to another process.".into());
        }
        if libc::kill(id.pid, if force { libc::SIGKILL } else { libc::SIGTERM }) != 0 {
            return Err(std::io::Error::last_os_error().to_string());
        }
    }
    Ok(())
}
