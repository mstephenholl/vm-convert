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

// ─── Public API ───────────────────────────────────────────────────────────────

/// Convert a `.vmdk` file to a QEMU disk image of the given `format`.
///
/// Requires `qemu-img` to be installed and available at `qemu_img_path`.
/// Displays a progress bar on stdout while the conversion runs.
pub fn convert_disk(
    qemu_img_path: &Path,
    vmdk_path: &Path,
    output_path: &Path,
    format: &str,
) -> Result<()> {
    if !vmdk_path.exists() {
        anyhow::bail!(
            "VMDK file not found: {}\nMake sure the .vmdk is in the same directory as the .ovf",
            vmdk_path.display()
        );
    }

    let pb = build_progress_bar();
    pb.set_message(format!(
        "Converting {} → {}.{}",
        vmdk_path.file_name().unwrap_or_default().to_string_lossy(),
        output_path
            .file_stem()
            .unwrap_or_default()
            .to_string_lossy(),
        format
    ));

    let vmdk_str = vmdk_path
        .to_str()
        .context("VMDK path contains non-UTF-8 characters")?;
    let out_str = output_path
        .to_str()
        .context("Output path contains non-UTF-8 characters")?;

    let mut child = Command::new(qemu_img_path)
        .args([
            "convert", "-p", "-f", "vmdk", "-O", format, vmdk_str, out_str,
        ])
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
        pb.finish_with_message("Disk conversion complete ✓");
        Ok(())
    } else {
        pb.abandon_with_message("Disk conversion FAILED ✗");
        anyhow::bail!(
            "qemu-img exited with status {}",
            status.code().unwrap_or(-1)
        )
    }
}

// ─── Internal helpers ─────────────────────────────────────────────────────────

fn build_progress_bar() -> ProgressBar {
    let pb = ProgressBar::new(100);
    pb.set_style(
        ProgressStyle::default_bar()
            .template("[{elapsed_precise}] [{bar:40.cyan/blue}] {pos:>3}% | {msg}")
            .expect("static template is valid")
            .progress_chars("█▉▊▋▌▍▎▏ "),
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

                // Split on carriage-return or newline, process all complete tokens
                // except the last one which might be partial.
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

    // Process any remaining bytes
    if let Some(pct) = parse_qemu_progress(&accumulator) {
        pb.set_position(pct.min(100.0) as u64);
    }
}

/// Parse a single `qemu-img -p` progress token.
///
/// Expected format: `"    (XX.XX/100%)"` (may have leading whitespace).
/// Returns `None` for any line that doesn't match.
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
    fn test_convert_fails_when_vmdk_missing() {
        let dir = tempdir().unwrap();
        let qemu_img = PathBuf::from("qemu-img");
        let vmdk = dir.path().join("nonexistent.vmdk");
        let out = dir.path().join("out.qcow2");

        let err = convert_disk(&qemu_img, &vmdk, &out, "qcow2")
            .unwrap_err()
            .to_string();

        assert!(
            err.contains("VMDK file not found"),
            "Unexpected error: {err}"
        );
    }

    #[test]
    fn test_convert_fails_with_fake_binary() {
        let dir = tempdir().unwrap();

        // Create a real (empty) vmdk placeholder so the existence check passes
        let vmdk = dir.path().join("disk.vmdk");
        std::fs::write(&vmdk, b"").unwrap();

        let out = dir.path().join("out.qcow2");
        let fake_qemu = PathBuf::from("/definitely/not/a/real/binary");

        let result = convert_disk(&fake_qemu, &vmdk, &out, "qcow2");
        assert!(result.is_err());
    }

    // ── drain_stderr_progress ────────────────────────────────────────────────

    #[test]
    fn test_drain_stderr_progress_no_panic_on_garbage() {
        let pb = ProgressBar::hidden();
        let garbage = b"some random garbage bytes\n\nmore garbage\r\n";
        drain_stderr_progress(garbage.as_ref(), &pb);
        // Should not panic and position stays 0
        assert_eq!(pb.position(), 0);
    }

    #[test]
    fn test_drain_stderr_progress_parses_correctly() {
        let pb = ProgressBar::new(100);
        // Simulate qemu-img -p output with \r delimiters
        let data = b"    (  0.00/100%)\r    ( 25.00/100%)\r    ( 50.00/100%)\r    (100.00/100%)\r";
        drain_stderr_progress(data.as_ref(), &pb);
        assert_eq!(pb.position(), 100);
    }
}
