use std::process::ExitStatus;
use std::sync::{Arc, Mutex};
use std::time::Duration;

#[cfg(target_os = "linux")]
use std::fs;
use tokio::io::AsyncReadExt;
use tokio::process::Command;

pub(crate) const CHILD_STALL_EXIT: i32 = 86;
const CHILD_DEADLINE: Duration = Duration::from_secs(60);
const KILL_REAP_DEADLINE: Duration = Duration::from_secs(5);

pub(crate) async fn run_crash_child(label: &str, mut cmd: Command) -> (ExitStatus, String) {
    cmd.stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .kill_on_drop(true);
    let mut child = cmd
        .spawn()
        .unwrap_or_else(|error| panic!("spawn crash child {label}: {error}"));
    let pid = child.id();
    let stdout = child.stdout.take().expect("piped child stdout");
    let stderr = child.stderr.take().expect("piped child stderr");
    let out_buffer = Arc::new(Mutex::new(Vec::new()));
    let err_buffer = Arc::new(Mutex::new(Vec::new()));
    let out_task = tokio::spawn(read_pipe(stdout, Arc::clone(&out_buffer)));
    let err_task = tokio::spawn(read_pipe(stderr, Arc::clone(&err_buffer)));
    match tokio::time::timeout(CHILD_DEADLINE, child.wait()).await {
        Ok(Ok(status)) => {
            let (out_result, err_result) =
                collect_readers(out_task, err_task, &out_buffer, &err_buffer).await;
            if out_result.is_err() || err_result.is_err() {
                let (out, err) = render_readers(&out_result, &err_result);
                panic!("crash child {label} pipe collection failed\n--- child stdout ---\n{out}\n--- child stderr ---\n{err}");
            }
            let (out, err) = render_readers(&out_result, &err_result);
            (
                status,
                format!("--- child stdout ---\n{out}\n--- child stderr ---\n{err}"),
            )
        }
        Ok(Err(error)) => {
            let snapshot = thread_states(pid);
            let (out, err) =
                terminate_and_collect(&mut child, out_task, err_task, &out_buffer, &err_buffer)
                    .await;
            panic!("waiting for crash child {label} failed: {error}; {snapshot}\n--- child stdout ---\n{out}\n--- child stderr ---\n{err}");
        }
        Err(_) => {
            let snapshot = thread_states(pid);
            let (out, err) =
                terminate_and_collect(&mut child, out_task, err_task, &out_buffer, &err_buffer)
                    .await;
            panic!("crash child {label} exceeded {CHILD_DEADLINE:?}; thread snapshot: {snapshot}\n--- child stdout ---\n{out}\n--- child stderr ---\n{err}");
        }
    }
}

async fn read_pipe<R: tokio::io::AsyncRead + Unpin>(
    mut pipe: R,
    retained: Arc<Mutex<Vec<u8>>>,
) -> Result<(), String> {
    let mut chunk = [0_u8; 4096];
    loop {
        match pipe.read(&mut chunk).await {
            Ok(0) => return Ok(()),
            Ok(count) => retained.lock().unwrap().extend_from_slice(&chunk[..count]),
            Err(error) => return Err(format!("pipe read failed: {error}")),
        }
    }
}

fn snapshot(buffer: &Mutex<Vec<u8>>) -> String {
    String::from_utf8_lossy(&buffer.lock().unwrap()).into_owned()
}

async fn collect_readers(
    out_task: tokio::task::JoinHandle<Result<(), String>>,
    err_task: tokio::task::JoinHandle<Result<(), String>>,
    out_buffer: &Mutex<Vec<u8>>,
    err_buffer: &Mutex<Vec<u8>>,
) -> (Result<String, String>, Result<String, String>) {
    let mut out_task = out_task;
    let mut err_task = err_task;
    let readers = tokio::time::timeout(KILL_REAP_DEADLINE, async {
        (
            collect_pipe(&mut out_task).await,
            collect_pipe(&mut err_task).await,
        )
    })
    .await;
    match readers {
        Ok((out, err)) => (
            describe_reader(out, out_buffer),
            describe_reader(err, err_buffer),
        ),
        Err(_) => {
            out_task.abort();
            err_task.abort();
            (
                Err(format!(
                    "{}\npipe reader exceeded {KILL_REAP_DEADLINE:?}",
                    snapshot(out_buffer)
                )),
                Err(format!(
                    "{}\npipe reader exceeded {KILL_REAP_DEADLINE:?}",
                    snapshot(err_buffer)
                )),
            )
        }
    }
}
fn render_readers(out: &Result<String, String>, err: &Result<String, String>) -> (String, String) {
    let out = out
        .as_ref()
        .map_or_else(|error| error.clone(), Clone::clone);
    let err = err
        .as_ref()
        .map_or_else(|error| error.clone(), Clone::clone);
    (out, err)
}

fn describe_reader(result: Result<(), String>, buffer: &Mutex<Vec<u8>>) -> Result<String, String> {
    match result {
        Ok(()) => Ok(snapshot(buffer)),
        Err(error) => Err(format!("{}\n{error}", snapshot(buffer))),
    }
}

async fn collect_pipe(
    task: &mut tokio::task::JoinHandle<Result<(), String>>,
) -> Result<(), String> {
    (&mut *task)
        .await
        .map_err(|error| format!("pipe reader task failed: {error}"))?
}

async fn terminate_and_collect(
    child: &mut tokio::process::Child,
    out_task: tokio::task::JoinHandle<Result<(), String>>,
    err_task: tokio::task::JoinHandle<Result<(), String>>,
    out_buffer: &Mutex<Vec<u8>>,
    err_buffer: &Mutex<Vec<u8>>,
) -> (String, String) {
    let _ = child.start_kill();
    let _ = tokio::time::timeout(KILL_REAP_DEADLINE, child.wait()).await;
    let (out, err) = collect_readers(out_task, err_task, out_buffer, err_buffer).await;
    render_readers(&out, &err)
}

#[cfg(target_os = "linux")]
fn thread_states(pid: Option<u32>) -> String {
    let Some(pid) = pid else {
        return "child pid unavailable".to_owned();
    };
    let root = format!("/proc/{pid}");
    let mut result = format!(
        "core_pattern: {}\n",
        fs::read_to_string("/proc/sys/kernel/core_pattern")
            .unwrap_or_else(|error| error.to_string())
    );
    if let Ok(status) = fs::read_to_string(format!("{root}/status")) {
        for line in status.lines().filter(|line| {
            line.starts_with("State:")
                || line.starts_with("CoreDumping:")
                || line.starts_with("Threads:")
        }) {
            result.push_str(line);
            result.push('\n');
        }
    }
    if let Ok(tasks) = fs::read_dir(format!("{root}/task")) {
        for task in tasks.flatten() {
            let tid = task.file_name().to_string_lossy().into_owned();
            let base = format!("{root}/task/{tid}");
            let comm = fs::read_to_string(format!("{base}/comm")).unwrap_or_default();
            let stat = fs::read_to_string(format!("{base}/stat")).unwrap_or_default();
            let state = stat
                .rsplit_once(") ")
                .and_then(|(_, rest)| rest.chars().next())
                .unwrap_or('?');
            let wchan = fs::read_to_string(format!("{base}/wchan")).unwrap_or_default();
            let syscall = fs::read_to_string(format!("{base}/syscall")).unwrap_or_default();
            result.push_str(&format!(
                "tid={tid} comm={} state={state} wchan={} syscall={}\n",
                comm.trim(),
                wchan.trim(),
                syscall.trim()
            ));
        }
    }
    result
}

#[cfg(not(target_os = "linux"))]
fn thread_states(_pid: Option<u32>) -> String {
    "thread state not collected on this OS".to_owned()
}
