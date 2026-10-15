use std::{
    fs,
    io::{BufRead, BufReader},
    process::{Child, Command, Stdio},
    sync::mpsc,
    time::Duration,
};

fn fixture(name: &str) -> std::path::PathBuf {
    let root = std::env::temp_dir()
        .join("codesearch-tests")
        .join(format!("cli-{name}-{}", std::process::id()));
    fs::create_dir_all(&root).unwrap();
    root
}

struct WatchedChild(Child);
impl Drop for WatchedChild {
    fn drop(&mut self) {
        if self.0.try_wait().is_ok_and(|status| status.is_none())
            && let Err(error) = self.0.kill()
        {
            eprintln!("failed to stop test child: {error}");
        }
        if let Err(error) = self.0.wait() {
            eprintln!("failed to reap test child: {error}");
        }
    }
}

#[test]
fn exact_case_insensitive_mode_and_range_errors_reach_cli_exit_codes() {
    let root = fixture("case");
    let path = root.join("case.txt");
    fs::write(&path, "NEEDLE\nneedle\n").unwrap();
    for (extra, expected_lines) in [(None, 1), (Some("--ignore-case"), 2)] {
        let mut command = Command::new(env!("CARGO_BIN_EXE_codesearch"));
        command
            .args(["--content", "--dir"])
            .arg(&root)
            .arg("NEEDLE")
            .env("CODESEARCH_CACHE_DIR", root.join("cache"));
        if let Some(flag) = extra {
            command.arg(flag);
        }
        let output = command.output().unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert_eq!(
            String::from_utf8(output.stdout).unwrap().lines().count(),
            expected_lines
        );
    }
    let invalid = Command::new(env!("CARGO_BIN_EXE_codesearch"))
        .args(["--lines", "0-2"])
        .arg(&path)
        .output()
        .unwrap();
    assert_eq!(invalid.status.code(), Some(2));
}

#[test]
fn cli_watch_stays_alive_reports_changes_and_does_not_index_its_cache() {
    let root = fixture("watch");
    let child = Command::new(env!("CARGO_BIN_EXE_codesearch"))
        .args(["--watch", "--dir"])
        .arg(&root)
        .arg("created")
        .env("CODESEARCH_CACHE_DIR", root.join("cache"))
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let mut child = WatchedChild(child);
    let stdout = child.0.stdout.take().unwrap();
    let (sender, receiver) = mpsc::channel();
    let reader = std::thread::spawn(move || {
        for line in BufReader::new(stdout).lines() {
            let line = line.unwrap();
            if line.ends_with("created.txt") {
                sender.send(line).unwrap();
                break;
            }
        }
    });
    std::thread::sleep(Duration::from_millis(200));
    assert!(
        child.0.try_wait().unwrap().is_none(),
        "--watch exited after its initial query"
    );
    fs::write(root.join("created.txt"), "new").unwrap();
    let output = receiver.recv_timeout(Duration::from_secs(5)).unwrap();
    assert!(output.ends_with("created.txt"));
    drop(child);
    reader.join().unwrap();
}
