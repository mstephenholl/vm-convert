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
| Folder or archive input | Point at a VM export folder or a compressed archive (`.ova`, `.tar.gz`, `.zip`, etc.) |
| OVF parsing | Extracts name, vCPU, RAM, disk path(s), NIC count |
| Multi-disk support | Converts all `.vmdk` disks referenced in the OVF (vda, vdb, …) |
| File validation | Verifies required `.ovf` and `.vmdk` files are present before conversion |
| UEFI detection | Automatic — checks for a `.nvram` sidecar file in the folder |
| Disk conversion | `qemu-img convert -f vmdk -O qcow2` with live progress bar |
| Compression | Optional `--compress` / `-c` flag for smaller qcow2 output |
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

## Installation

### Download from GitHub Releases

Pre-built binaries are available on the
[Releases page](https://github.com/mstephenholl/vm-convert/releases).

1. **Download the binary for your platform:**

   **Linux x86_64 (glibc):**
   ```bash
   curl -Lo vm-convert https://github.com/mstephenholl/vm-convert/releases/latest/download/vm-convert-x86_64-unknown-linux-gnu
   ```

   **Linux x86_64 (musl/static):**
   ```bash
   curl -Lo vm-convert https://github.com/mstephenholl/vm-convert/releases/latest/download/vm-convert-x86_64-unknown-linux-musl
   ```

   **Linux ARM64:**
   ```bash
   curl -Lo vm-convert https://github.com/mstephenholl/vm-convert/releases/latest/download/vm-convert-aarch64-unknown-linux-gnu
   ```

   **macOS Intel:**
   ```bash
   curl -Lo vm-convert https://github.com/mstephenholl/vm-convert/releases/latest/download/vm-convert-x86_64-apple-darwin
   ```

   **macOS Apple Silicon:**
   ```bash
   curl -Lo vm-convert https://github.com/mstephenholl/vm-convert/releases/latest/download/vm-convert-aarch64-apple-darwin
   ```

2. **Make the binary executable:**

   ```bash
   chmod +x vm-convert
   ```

3. **Move it to a directory on your PATH:**

   ```bash
   sudo mv vm-convert /usr/local/bin/
   ```

   Verify the installation:

   ```bash
   vm-convert --version
   ```

### Updating to the latest version

To update, repeat the download and install steps above — the `latest` URL
always points to the most recent release. For example on macOS Apple Silicon:

```bash
curl -Lo vm-convert https://github.com/mstephenholl/vm-convert/releases/latest/download/vm-convert-aarch64-apple-darwin
chmod +x vm-convert
sudo mv vm-convert /usr/local/bin/
```

To install a specific version, replace `latest/download` with
`download/<tag>`:

```bash
curl -Lo vm-convert https://github.com/mstephenholl/vm-convert/releases/download/v0.2.0/vm-convert-aarch64-apple-darwin
```

### Removing a previous installation

```bash
sudo rm /usr/local/bin/vm-convert
```

---

## Build from source

```bash
cargo build --release
# Binary: ./target/release/vm-convert
```

---

## Usage

```
vm-convert [OPTIONS] <INPUT>

Arguments:
  <INPUT>    Path to a VM export folder (.ovf + disks) or a compressed archive
             (.ova, .tar, .tar.gz/.tgz, .tar.bz2/.tbz2, .tar.xz/.txz,
             .tar.zst/.tzst, .zip)

Options:
  -o, --output-dir <DIR>   Output directory [default: same as VM_DIR]
  -n, --name <NAME>        Override VM name from OVF metadata
      --no-import          Generate XML only, skip virsh define
  -c, --compress           Compress output qcow2 images (smaller files)
      --format <FORMAT>    Disk format: qcow2 | raw [default: qcow2]
      --skip-verify        Skip .mf manifest verification
      --force-virtio       Override all disk bus types to VirtIO
      --ovmf-code <PATH>   Path to OVMF firmware (overrides auto-detection)
  -h, --help               Print help
  -V, --version            Print version
```

### Typical run

```bash
# The VM export folder must contain at least a .ovf and .vmdk file:
#   myvm/
#   ├── myvm.ovf
#   ├── myvm.vmdk        (primary disk)
#   ├── myvm_1.vmdk      (optional — additional disks)
#   └── myvm.nvram       (optional — indicates UEFI)

vm-convert myvm/

# Or pass a compressed archive — it will be extracted automatically:
vm-convert myvm.ova
vm-convert myvm.tar.gz
vm-convert myvm.zip

# With explicit output directory and name override:
vm-convert --output-dir /var/lib/libvirt/images --name prod-server myvm/

# Compress qcow2 output (recommended when source VMDKs are streamOptimized):
vm-convert --compress myvm/

# Generate XML + QCOW2 without auto-importing (useful on macOS or for review):
vm-convert --no-import myvm/
```

### Typical output

```
vm-convert  ─  VMware OVF/VMDK → QEMU/KVM Converter
──────────────────────────────────────────────────────
✓ VM folder : myvm/
  .ovf      : myvm.ovf
  .vmdk     : 1 file(s)
  .nvram    : myvm.nvram
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
  myvm.vmdk → myvm.qcow2
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

### Multi-disk VMs

When an OVF references multiple `.vmdk` files, all disks are converted and
included in the generated libvirt XML. Output filenames use the VM name with a
numeric suffix for secondary disks:

```
myvm.qcow2     ← first disk  (vda)
myvm_1.qcow2   ← second disk (vdb)
myvm_2.qcow2   ← third disk  (vdc)
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

Tests cover archive detection and extraction (all formats), folder inventory
scanning, OVF parsing, libvirt XML generation, progress bar parsing, error
paths, and platform detection. No external tools (qemu-img, virsh) are
required to run the test suite.

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
├── archive.rs       Archive detection & extraction (OVA, tar.gz, zip, etc.)
├── inventory.rs     VM folder scanning & file validation
├── manifest.rs      Manifest (.mf) parsing & hash verification
├── ovf.rs           OVF XML parser (roxmltree)
├── convert.rs       qemu-img invocation + live progress bar
├── libvirt_xml.rs   libvirt domain XML generator
└── platform.rs      Platform detection, virsh import
```
