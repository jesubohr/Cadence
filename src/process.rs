//! Runs child programs without a shell, drains their output and kills the
//! whole process tree on cancel.

use std::ffi::OsString;
use std::io::{self, BufRead, BufReader, Read};
use std::os::windows::io::AsRawHandle;
use std::os::windows::process::CommandExt;
use std::path::Path;
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;

use windows::Win32::Foundation::{CloseHandle, HANDLE};
use windows::Win32::System::JobObjects::{
    AssignProcessToJobObject, CreateJobObjectW, JobObjectExtendedLimitInformation,
    SetInformationJobObject, TerminateJobObject, JOBOBJECT_EXTENDED_LIMIT_INFORMATION,
    JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
};
use windows::Win32::System::Threading::CREATE_NO_WINDOW;

const STDERR_LINES_KEPT: usize = 40;

/// Windows job object. Every process started by the child joins it, so one
/// call ends the full tree. Closing the handle also ends the tree.
struct ProcessTree(HANDLE);

// The handle is only used through thread-safe Win32 calls.
unsafe impl Send for ProcessTree {}
unsafe impl Sync for ProcessTree {}

impl ProcessTree {
    fn new() -> io::Result<Self> {
        unsafe {
            let job = CreateJobObjectW(None, None)?;
            let tree = ProcessTree(job);
            let mut info = JOBOBJECT_EXTENDED_LIMIT_INFORMATION::default();
            info.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
            SetInformationJobObject(
                job,
                JobObjectExtendedLimitInformation,
                &info as *const _ as *const _,
                std::mem::size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32,
            )?;
            Ok(tree)
        }
    }

    fn assign(&self, child: &std::process::Child) -> io::Result<()> {
        let process = HANDLE(child.as_raw_handle());
        unsafe { AssignProcessToJobObject(self.0, process)? };
        Ok(())
    }

    fn kill(&self) {
        // The tree may already be gone, which is fine.
        unsafe {
            let _ = TerminateJobObject(self.0, 1);
        }
    }
}

impl Drop for ProcessTree {
    fn drop(&mut self) {
        unsafe {
            let _ = CloseHandle(self.0);
        }
    }
}

/// Shared cancel switch. Clones control the same run.
#[derive(Clone, Default)]
pub struct Cancel(Arc<CancelInner>);

#[derive(Default)]
struct CancelInner {
    flag: AtomicBool,
    tree: Mutex<Option<Arc<ProcessTree>>>,
}

impl Cancel {
    pub fn cancel(&self) {
        self.0.flag.store(true, Ordering::SeqCst);
        if let Some(tree) = self.0.tree.lock().unwrap().as_ref() {
            tree.kill();
        }
    }

    pub fn is_cancelled(&self) -> bool {
        self.0.flag.load(Ordering::SeqCst)
    }

    fn attach(&self, tree: Arc<ProcessTree>) {
        *self.0.tree.lock().unwrap() = Some(tree.clone());
        // A cancel that arrived before the attach must still take effect.
        if self.is_cancelled() {
            tree.kill();
        }
    }

    fn detach(&self) {
        *self.0.tree.lock().unwrap() = None;
    }
}

#[derive(Debug)]
pub struct Finished {
    pub success: bool,
    /// Last lines written to stderr.
    pub stderr: Vec<String>,
}

pub fn command(program: &Path, args: &[OsString]) -> Command {
    let mut cmd = Command::new(program);
    cmd.args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .creation_flags(CREATE_NO_WINDOW.0);
    cmd
}

/// Runs the command to the end. Calls `on_line` for each stdout line.
/// Both pipes are drained so the child never blocks on a full pipe.
pub fn run(
    mut cmd: Command,
    cancel: &Cancel,
    mut on_line: impl FnMut(&str),
) -> io::Result<Finished> {
    let tree = Arc::new(ProcessTree::new()?);
    let mut child = cmd.spawn()?;
    if let Err(err) = tree.assign(&child) {
        let _ = child.kill();
        let _ = child.wait();
        return Err(err);
    }
    cancel.attach(tree);

    let stderr_pipe = child.stderr.take().expect("stderr is piped");
    let stderr_reader = thread::spawn(move || collect_tail(stderr_pipe));

    let stdout_pipe = child.stdout.take().expect("stdout is piped");
    let mut reader = BufReader::new(stdout_pipe);
    let mut buf = Vec::new();
    loop {
        buf.clear();
        match reader.read_until(b'\n', &mut buf) {
            Ok(0) | Err(_) => break,
            Ok(_) => on_line(String::from_utf8_lossy(&buf).trim_end_matches(['\r', '\n'])),
        }
    }

    let status = child.wait()?;
    cancel.detach();
    let stderr = stderr_reader.join().unwrap_or_default();
    Ok(Finished {
        success: status.success() && !cancel.is_cancelled(),
        stderr,
    })
}

fn collect_tail(pipe: impl Read) -> Vec<String> {
    let mut reader = BufReader::new(pipe);
    let mut tail: Vec<String> = Vec::new();
    let mut buf = Vec::new();
    loop {
        buf.clear();
        match reader.read_until(b'\n', &mut buf) {
            Ok(0) | Err(_) => break,
            Ok(_) => {
                let line = String::from_utf8_lossy(&buf)
                    .trim_end_matches(['\r', '\n'])
                    .to_string();
                if !line.is_empty() {
                    tail.push(line);
                    if tail.len() > STDERR_LINES_KEPT {
                        tail.remove(0);
                    }
                }
            }
        }
    }
    tail
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{Duration, Instant};

    fn cmd_exe(script: &str) -> Command {
        command(
            Path::new("cmd.exe"),
            &[OsString::from("/c"), OsString::from(script)],
        )
    }

    #[test]
    fn captures_stdout_lines_and_stderr() {
        let mut lines = Vec::new();
        let finished = run(
            cmd_exe("echo one& echo two& echo oops>&2"),
            &Cancel::default(),
            |l| lines.push(l.to_string()),
        )
        .unwrap();
        assert!(finished.success);
        assert_eq!(lines, vec!["one", "two"]);
        assert_eq!(finished.stderr, vec!["oops"]);
    }

    #[test]
    fn reports_failure_exit_code() {
        let finished = run(cmd_exe("exit 3"), &Cancel::default(), |_| {}).unwrap();
        assert!(!finished.success);
    }

    #[test]
    fn drains_more_output_than_a_pipe_holds() {
        // Roughly 600 KB on each stream, far above the 4 KB pipe buffer.
        let script = "for /l %i in (1,1,12000) do @(echo line-%i-aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa& echo err-%i-aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa 1>&2)";
        let mut count = 0;
        let finished = run(cmd_exe(script), &Cancel::default(), |_| count += 1).unwrap();
        assert!(finished.success);
        assert_eq!(count, 12000);
        assert_eq!(finished.stderr.len(), STDERR_LINES_KEPT);
    }

    #[test]
    fn cancel_kills_the_child_and_its_children() {
        // cmd.exe starts ping.exe as a child. Both must end quickly.
        let cancel = Cancel::default();
        let killer = cancel.clone();
        let start = Instant::now();
        let handle = thread::spawn(move || {
            run(cmd_exe("ping -n 60 127.0.0.1 >nul"), &killer, |_| {}).unwrap()
        });
        thread::sleep(Duration::from_millis(800));
        cancel.cancel();
        let finished = handle.join().unwrap();
        assert!(!finished.success);
        assert!(start.elapsed() < Duration::from_secs(10));
    }

    #[test]
    fn cancel_before_start_still_kills() {
        let cancel = Cancel::default();
        cancel.cancel();
        let start = Instant::now();
        let finished = run(cmd_exe("ping -n 60 127.0.0.1 >nul"), &cancel, |_| {}).unwrap();
        assert!(!finished.success);
        assert!(start.elapsed() < Duration::from_secs(10));
    }
}
