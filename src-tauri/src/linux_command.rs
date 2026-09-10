//! Bounded execution of host desktop utilities.
use std::io::{Read, Seek};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

pub fn run(mut command: Command, deadline: Instant, capture: bool) -> Result<String, String> {
    let program = command.get_program().to_string_lossy().into_owned();
    if Instant::now() >= deadline {
        return Err(format!("{program} timed out"));
    }
    // Host KDE tools must not load the AppImage's bundled Qt libraries.
    if std::env::var_os("APPIMAGE").is_some() {
        command
            .env_remove("LD_LIBRARY_PATH")
            .env_remove("QT_PLUGIN_PATH");
    }
    // A temporary output file cannot fill a pipe and deadlock the child.
    let mut output = capture
        .then(tempfile::tempfile)
        .transpose()
        .map_err(|e| e.to_string())?;
    let stdout = match &output {
        Some(file) => Stdio::from(file.try_clone().map_err(|e| e.to_string())?),
        None => Stdio::null(),
    };
    let mut child = command
        .stdin(Stdio::null())
        .stdout(stdout)
        .stderr(Stdio::null())
        .spawn()
        .map_err(|e| format!("{program}: {e}"))?;
    loop {
        match child.try_wait() {
            Ok(Some(status)) if status.success() => break,
            Ok(Some(status)) => return Err(format!("{program} exited with {status}")),
            Ok(None) if Instant::now() < deadline => std::thread::sleep(Duration::from_millis(10)),
            result => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(match result {
                    Err(error) => error.to_string(),
                    _ => format!("{program} timed out"),
                });
            }
        }
    }
    let mut text = String::new();
    if let Some(file) = &mut output {
        file.rewind().map_err(|e| e.to_string())?;
        file.take(65536)
            .read_to_string(&mut text)
            .map_err(|e| e.to_string())?;
    }
    Ok(text)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn captures_output_and_rejects_failures_and_timeouts() {
        let mut command = Command::new("printf");
        command.arg("12500\n");
        assert_eq!(
            run(command, Instant::now() + Duration::from_secs(2), true).unwrap(),
            "12500\n"
        );
        assert!(run(
            Command::new("false"),
            Instant::now() + Duration::from_secs(2),
            false
        )
        .is_err());
        let mut command = Command::new("sleep");
        command.arg("10");
        let start = Instant::now();
        assert!(run(command, start + Duration::from_millis(30), false).is_err());
        assert!(start.elapsed() < Duration::from_secs(2));
    }
}
