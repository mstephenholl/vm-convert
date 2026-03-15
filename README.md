# vm-convert

Convert VMware OVF/VMDK virtual machine images to QEMU/KVM format
(`.qcow2` disk + libvirt domain XML) for use with **Virtual Machine Manager**
on Ubuntu 24.04, or any Linux host running KVM.

Cross-platform: runs on **Linux** and **macOS** (macOS produces the artefacts
without the final `virsh define` step).

---

## Features

| Capability | Detail |
|---|---|
| OVF parsing | Extracts name, vCPU, RAM, disk path, NIC count |
| UEFI detection | Automatic — checks for a `.nvram` sidecar file |
| Disk conversion | `qemu-img convert -f vmdk -O qcow2` with live progress bar |
| Libvirt XML | Generates a complete `<domain>` definition (q35 + VirtIO + SPICE) |
| Auto-import | Runs `virsh define` on Linux when conversion is complete |
| macOS support | Produces artefacts only; manual transfer + import required |

---

## Prerequisites

### Ubuntu / Debian

```bash
sudo apt install qemu-utils libvirt-daemon-system libvirt-clients virt-manager
```

### macOS

```bash
brew install qemu
```

---

## Build

```bash
cargo build --release
# Binary: ./target/release/vm-convert
```

---

## Usage

```
vm-convert [OPTIONS] <OVF_FILE>

Arguments:
  <OVF_FILE>   Path to the .ovf file (the .vmdk must be in the same directory)

Options:
  -o, --output-dir <DIR>   Output directory [default: same as .ovf]
  -n, --name <NAME>        Override VM name from OVF metadata
      --no-import          Generate XML only, skip virsh define
      --format <FORMAT>    Disk format: qcow2 | raw [default: qcow2]
  -h, --help               Print help
  -V, --version            Print version
```

### Typical run

```bash
# All three source files must be in the same directory:
#   myvm.ovf   myvm.vmdk   myvm.nvram  (nvram = UEFI)

vm-convert myvm.ovf

# With explicit output directory and name override:
vm-convert --output-dir /var/lib/libvirt/images --name prod-server myvm.ovf

# Generate XML + QCOW2 without auto-importing (useful on macOS or for review):
vm-convert --no-import myvm.ovf
```

### Typical output

```
vm-convert  ─  VMware OVF/VMDK → QEMU/KVM Converter
──────────────────────────────────────────────────────
✓ qemu-img  : /usr/bin/qemu-img
✓ OVF parsed
  Name     : myvm
  vCPUs    : 4
  RAM      : 8192 MiB
  Disk src : myvm.vmdk
  Firmware : UEFI (OVMF)
  NICs     : 1
──────────────────────────────────────────────────────
Output dir : /var/lib/libvirt/images
VMDK source: /tmp/export/myvm.vmdk
QCOW2 dest : /var/lib/libvirt/images/myvm.qcow2
──────────────────────────────────────────────────────
[00:01:23] [████████████████████████] 100% | Converting myvm.vmdk → myvm.qcow2
✓ Disk converted → /var/lib/libvirt/images/myvm.qcow2
✓ Libvirt XML → /var/lib/libvirt/images/myvm.xml
Importing to libvirt… done ✓
──────────────────────────────────────────────────────
Next steps:
  Start VM : virsh start myvm
  Open GUI : virt-manager
  Find IP  : virsh domifaddr myvm
──────────────────────────────────────────────────────
✓ Conversion complete.
```

---

## Notes

### UEFI VMs (`.nvram` sidecar present)

The generated XML references Ubuntu's OVMF paths:

```
/usr/share/OVMF/OVMF_CODE.fd   ← firmware (read-only)
/var/lib/libvirt/qemu/nvram/<name>_VARS.fd  ← mutable NVRAM
```

Install OVMF if not already present:

```bash
sudo apt install ovmf
```

### VirtIO drivers

This tool assumes the guest OS already has VirtIO drivers — true for all
Linux guests. Windows guests need VirtIO drivers injected before conversion
(not currently in scope).

### Disk placement

For best performance, place the `.qcow2` file in libvirt's default image pool:

```bash
sudo mv myvm.qcow2 /var/lib/libvirt/images/
sudo chown libvirt-qemu:libvirt-qemu /var/lib/libvirt/images/myvm.qcow2
```

---

## Running tests

```bash
cargo test
```

Tests cover OVF parsing, libvirt XML generation, progress bar parsing,
error paths, and platform detection. No external tools (qemu-img, virsh)
are required to run the test suite.

---

## Development

### CI/CD

- **CI** runs on every push and pull request to `main`: format check (`cargo fmt`), clippy lints, tests on Linux + macOS, and a release build
- **Releases** are triggered by pushing a SemVer git tag (e.g., `v0.2.0`)
- Release builds produce binaries for 5 targets across Linux and macOS
- Each release includes SHA-256 checksums for all binaries

### Versioning

This project follows [Semantic Versioning](https://semver.org/). The version source of truth is `Cargo.toml`.

**Release process:**

```bash
# 1. Bump version in Cargo.toml
# 2. Update Cargo.lock
cargo check
# 3. Commit
git add Cargo.toml Cargo.lock
git commit -m "release: v0.2.0"
# 4. Tag and push
git tag v0.2.0
git push origin main --tags
```

### Release targets

| Target | Description |
|--------|-------------|
| `x86_64-unknown-linux-gnu` | Linux x86_64 (glibc) |
| `x86_64-unknown-linux-musl` | Linux x86_64 (musl, static) |
| `aarch64-unknown-linux-gnu` | Linux ARM64 |
| `x86_64-apple-darwin` | macOS Intel |
| `aarch64-apple-darwin` | macOS Apple Silicon |

---

## Architecture

```
src/
├── main.rs          Orchestration / CLI entry point
├── cli.rs           clap argument definitions
├── ovf.rs           OVF XML parser (roxmltree)
├── convert.rs       qemu-img invocation + live progress bar
├── libvirt_xml.rs   libvirt domain XML generator
└── platform.rs      Platform detection, virsh import
```
