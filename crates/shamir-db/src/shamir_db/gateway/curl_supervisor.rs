use std::path::PathBuf;
use std::process::Stdio;
use std::time::Duration;

use shamir_engine::function::{HttpResponse, NetGatewayError};
use tokio::io::AsyncReadExt;
use tokio::process::{Child, ChildStderr, ChildStdout, Command};
use tokio::sync::oneshot;

use super::curl_gateway::parse_response_headers;

#[cfg(test)]
#[derive(Debug)]
pub(crate) enum LifecycleEvent {
    Spawned {
        pid: u32,
        path: PathBuf,
    },
    SpawnFailed(String),
    Completed {
        path: PathBuf,
        wait: Result<std::process::ExitStatus, String>,
    },
}

// Cancellation-safe for callers: dropping the run future closes its receiver;
// the detached supervisor still terminates and waits for curl before dropping temp.
// The test observer emits at most two events per invocation (one on spawn failure).
pub(super) async fn run(
    temp: tempfile::TempDir,
    config: PathBuf,
    headers: PathBuf,
    limit: usize,
    #[cfg(test)] observer: Option<tokio::sync::mpsc::UnboundedSender<LifecycleEvent>>,
) -> Result<HttpResponse, NetGatewayError> {
    let (mut sender, receiver) = oneshot::channel();
    // Explicitly detach: the supervisor owns child and tempdir beyond requester cancellation.
    drop(tokio::spawn(async move {
        let mut child = match Command::new("curl")
            .arg("-K")
            .arg(config)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true)
            .spawn()
        {
            Ok(child) => child,
            Err(source) => {
                #[cfg(test)]
                if let Some(observer) = &observer {
                    let _ = observer.send(LifecycleEvent::SpawnFailed(source.to_string()));
                }
                let operation = if source.kind() == std::io::ErrorKind::NotFound {
                    "egress unavailable: curl not found"
                } else {
                    "curl spawn error"
                };
                let _ = sender.send(Err(NetGatewayError::Io { operation, source }));
                return;
            }
        };
        #[cfg(test)]
        let temp_path = temp.path().to_path_buf();
        #[cfg(test)]
        if let Some(observer) = &observer {
            let _ = observer.send(LifecycleEvent::Spawned {
                pid: child.id().expect("spawned curl PID"),
                path: temp_path.clone(),
            });
        }
        let stdout = child.stdout.take().expect("piped curl stdout");
        let stderr = child.stderr.take().expect("piped curl stderr");
        let result = tokio::select! {
            _ = sender.closed() => None,
            result = tokio::time::timeout(Duration::from_secs(35), transfer(&mut child, stdout, stderr, limit)) => {
                Some(match result {
                    Ok(result) => result,
                    Err(_) => Err(NetGatewayError::Transport("egress: curl timed out".into())),
                })
            }
        };
        if result.as_ref().is_none_or(Result::is_err) {
            if let Err(error) = child.kill().await {
                log::warn!("egress: curl termination failed: {error}");
            }
        }
        let wait = child.wait().await;
        if let Err(error) = &wait {
            log::warn!("egress: curl reap failed: {error}");
        }
        let result = match (result, &wait) {
            (Some(_), Err(error)) => Some(Err(NetGatewayError::Transport(format!(
                "egress: curl reap failed: {error}"
            )))),
            (Some(Ok(body)), Ok(_)) => Some(match parse_response_headers(&headers).await {
                Ok((status, headers)) => Ok(HttpResponse {
                    status,
                    headers,
                    body,
                }),
                Err(error) => Err(error),
            }),
            (Some(Err(error)), Ok(_)) => Some(Err(error)),
            (None, _) => None,
        };
        drop(temp);
        #[cfg(test)]
        if let Some(observer) = &observer {
            let _ = observer.send(LifecycleEvent::Completed {
                path: temp_path,
                wait: wait.map_err(|error| error.to_string()),
            });
        }
        if let Some(result) = result {
            let _ = sender.send(result);
        }
    }));
    receiver.await.unwrap_or_else(|_| {
        Err(NetGatewayError::Transport(
            "egress: curl supervisor failed".into(),
        ))
    })
}

async fn transfer(
    child: &mut Child,
    mut stdout: ChildStdout,
    mut stderr: ChildStderr,
    limit: usize,
) -> Result<Vec<u8>, NetGatewayError> {
    let body = async {
        let mut body = Vec::new();
        let mut chunk = [0u8; 8192];
        loop {
            let n = stdout.read(&mut chunk).await.map_err(|e| {
                NetGatewayError::Transport(format!("egress: curl body read failed: {e}"))
            })?;
            if n == 0 {
                return Ok(body);
            }
            if n > limit.saturating_sub(body.len()) {
                return Err(NetGatewayError::Transport(format!(
                    "egress: response body exceeds {limit} bytes"
                )));
            }
            body.extend_from_slice(&chunk[..n]);
        }
    };
    let errors = async {
        let mut captured = Vec::new();
        let mut chunk = [0u8; 4096];
        loop {
            let n = stderr.read(&mut chunk).await.map_err(|e| {
                NetGatewayError::Transport(format!("egress: curl stderr read failed: {e}"))
            })?;
            if n == 0 {
                return Ok::<_, NetGatewayError>(captured);
            }
            let keep = n.min(8192usize.saturating_sub(captured.len()));
            captured.extend_from_slice(&chunk[..keep]);
        }
    };
    let (body, errors, status) = tokio::try_join!(body, errors, async {
        child
            .wait()
            .await
            .map_err(|e| NetGatewayError::Transport(format!("egress: curl wait failed: {e}")))
    })?;
    if !status.success() {
        return Err(NetGatewayError::Transport(format!(
            "egress: curl failed ({status}): {}",
            String::from_utf8_lossy(&errors)
        )));
    }
    Ok(body)
}
