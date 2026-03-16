/// convert.rs — Shell out to `qemu-img convert` and report live progress.
///
/// `qemu-img -p` writes progress to **stderr** using `\r` (carriage return)
/// to overwrite the same terminal line.  The format is:
///
///     "    (XX.XX/100%)"
///
/// We read stderr in a raw byte-level loop, splitting on both `\r` and `\n`,
/// and parse each token for a percentage value to drive an indicatif bar.
use anyhow::{Context, Result};
use indicatif::{ProgressBar, ProgressStyle};
use std::io::Read;
use std::path::Path;
use std::process::{Command, Stdio};
use std::time::Duration;

// ─── Public API ──────────────────────────────────────────────────────────────

/// Convert a disk image to a QEMU disk image of the given output `format`.
///
/// `input_format` — qemu-img input format string (e.g. "vmdk", "vpc", "vdi")
pub fn convert_disk(
    qemu_img_path: &Path,
    input_path: &Path,
    output_path: &Path,
    input_format: &str,
    output_format: &str,
    compress: bool,
) -> Result<()> {
    if !input_path.exists() {
        anyhow::bail!(
            "Disk image not found: {}\nMake sure the disk file is in the same directory as the .ovf",
            input_path.display()
        );
    }

    let pb = build_progress_bar();
    pb.set_message(format!(
        "Converting {} → {}.{}",
        input_path.file_name().unwrap_or_default().to_string_lossy(),
        output_path
            .file_stem()
            .unwrap_or_default()
            .to_string_lossy(),
        output_format
    ));

    let input_str = input_path
        .to_str()
        .context("Input path contains non-UTF-8 characters")?;
    let out_str = output_path
        .to_str()
        .context("Output path contains non-UTF-8 characters")?;

    let mut qemu_args = vec!["convert", "-p", "-f", input_format, "-O", output_format];
    if compress {
        qemu_args.push("-c");
    }
    qemu_args.push(input_str);
    qemu_args.push(out_str);

    let mut child = Command::new(qemu_img_path)
        .args(&qemu_args)
        .stderr(Stdio::piped())
        .stdout(Stdio::null())
        .spawn()
        .with_context(|| {
            format!(
                "Failed to launch qemu-img ({}). Is it installed?",
                qemu_img_path.display()
            )
        })?;

    // Drain stderr and update the progress bar
    if let Some(stderr) = child.stderr.take() {
        drain_stderr_progress(stderr, &pb);
    }

    let status = child
        .wait()
        .context("Failed to wait for qemu-img process")?;

    if status.success() {
        pb.finish_with_message("Disk conversion complete");
        Ok(())
    } else {
        pb.abandon_with_message("Disk conversion FAILED");
        anyhow::bail!(
            "qemu-img exited with status {}",
            status.code().unwrap_or(-1)
        )
    }
}

// ─── Internal helpers ────────────────────────────────────────────────────────

fn build_progress_bar() -> ProgressBar {
    let pb = ProgressBar::new(100);
    pb.set_style(
        ProgressStyle::default_bar()
            .template("[{elapsed_precise}] [{bar:40.cyan/blue}] {pos:>3}% | {msg}")
            .expect("static template is valid")
            .progress_chars("##-"),
    );
    pb.enable_steady_tick(Duration::from_millis(120));
    pb
}

/// Read raw bytes from `reader`, split on `\r` / `\n`, parse each token for a
/// `qemu-img -p` progress percentage and push it into `pb`.
fn drain_stderr_progress(mut reader: impl Read, pb: &ProgressBar) {
    let mut buf = [0u8; 512];
    let mut accumulator = String::with_capacity(64);

    loop {
        match reader.read(&mut buf) {
            Ok(0) => break,
            Ok(n) => {
                accumulator.push_str(&String::from_utf8_lossy(&buf[..n]));

                let last_delim = accumulator.rfind(['\r', '\n']);
                if let Some(pos) = last_delim {
                    let complete = &accumulator[..pos];
                    complete
                        .split(['\r', '\n'])
                        .filter_map(parse_qemu_progress)
                        .for_each(|pct| pb.set_position(pct.min(100.0) as u64));

                    accumulator = accumulator[pos + 1..].to_string();
                }
            }
            Err(_) => break,
        }
    }

    if let Some(pct) = parse_qemu_progress(&accumulator) {
        pb.set_position(pct.min(100.0) as u64);
    }
}

/// Parse a single `qemu-img -p` progress token.
pub fn parse_qemu_progress(token: &str) -> Option<f64> {
    let t = token.trim();
    if !t.starts_with('(') || !t.ends_with(')') {
        return None;
    }
    let inner = t.trim_start_matches('(').trim_end_matches(')');
    let slash = inner.find('/')?;
    inner[..slash].trim().parse().ok()
}

// ─── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;
    use tempfile::tempdir;

    // ── parse_qemu_progress ──────────────────────────────────────────────────

    #[test]
    fn test_progress_zero() {
        assert_eq!(parse_qemu_progress("    (  0.00/100%)"), Some(0.0));
    }

    #[test]
    fn test_progress_fifty() {
        assert_eq!(parse_qemu_progress("    ( 50.00/100%)"), Some(50.0));
    }

    #[test]
    fn test_progress_hundred() {
        assert_eq!(parse_qemu_progress("    (100.00/100%)"), Some(100.0));
    }

    #[test]
    fn test_progress_decimal() {
        assert_eq!(parse_qemu_progress("(75.50/100%)"), Some(75.5));
    }

    #[test]
    fn test_progress_no_whitespace() {
        assert_eq!(parse_qemu_progress("(33.33/100%)"), Some(33.33));
    }

    #[test]
    fn test_progress_empty_string() {
        assert_eq!(parse_qemu_progress(""), None);
    }

    #[test]
    fn test_progress_random_text() {
        assert_eq!(parse_qemu_progress("some random output"), None);
    }

    #[test]
    fn test_progress_incomplete_token() {
        assert_eq!(parse_qemu_progress("(50.00"), None);
    }

    #[test]
    fn test_progress_no_slash() {
        assert_eq!(parse_qemu_progress("(50.00)"), None);
    }

    // ── convert_disk error paths ─────────────────────────────────────────────

    #[test]
    fn test_convert_fails_when_disk_missing() {
        let dir = tempdir().unwrap();
        let qemu_img = PathBuf::from("qemu-img");
        let input = dir.path().join("nonexistent.vmdk");
        let out = dir.path().join("out.qcow2");

        let err = convert_disk(&qemu_img, &input, &out, "vmdk", "qcow2", false)
            .unwrap_err()
            .to_string();

        assert!(
            err.contains("Disk image not found"),
            "Unexpected error: {err}"
        );
    }

    #[test]
    fn test_convert_fails_with_fake_binary() {
        let dir = tempdir().unwrap();

        let input = dir.path().join("disk.vmdk");
        std::fs::write(&input, b"").unwrap();

        let out = dir.path().join("out.qcow2");
        let fake_qemu = PathBuf::from("/definitely/not/a/real/binary");

        let result = convert_disk(&fake_qemu, &input, &out, "vmdk", "qcow2", false);
        assert!(result.is_err());
    }

    // ── drain_stderr_progress ────────────────────────────────────────────────

    #[test]
    fn test_drain_stderr_progress_no_panic_on_garbage() {
        let pb = ProgressBar::hidden();
        let garbage = b"some random garbage bytes\n\nmore garbage\r\n";
        drain_stderr_progress(garbage.as_ref(), &pb);
        assert_eq!(pb.position(), 0);
    }

    #[test]
    fn test_drain_stderr_progress_parses_correctly() {
        let pb = ProgressBar::new(100);
        let data = b"    (  0.00/100%)\r    ( 25.00/100%)\r    ( 50.00/100%)\r    (100.00/100%)\r";
        drain_stderr_progress(data.as_ref(), &pb);
        assert_eq!(pb.position(), 100);
    }
}
