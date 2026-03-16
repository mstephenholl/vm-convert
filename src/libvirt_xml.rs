/// libvirt_xml.rs — Generate a libvirt domain XML definition from a VmConfig.
///
/// We emit XML directly as a formatted string rather than using an XML library
/// for output, since the schema is well-known and small.  The result is a
/// complete <domain> element that can be fed to `virsh define`.
///
/// Key decisions reflected in the generated XML:
///  - machine type "q35"  — modern PCIe bus, required for UEFI/OVMF
///  - cpu mode "host-passthrough" — best performance on KVM, avoids migration
///  - VirtIO for disk and NIC — best I/O performance for Linux guests
///  - SPICE graphics + QXL video — works with virt-viewer and virt-manager
///  - OVMF (UEFI) paths follow Ubuntu packaging: /usr/share/OVMF/OVMF_CODE.fd
use crate::ovf::VmConfig;
use anyhow::Result;
use std::path::PathBuf;

// ─── Public API ───────────────────────────────────────────────────────────────

/// Generate a libvirt domain XML string for the given VM.
///
/// `vm_name`     — the domain name (may differ from config.name via --name flag)
/// `qcow2_paths` — absolute paths to the converted disk images (one per OVF disk)
pub fn generate(config: &VmConfig, vm_name: &str, qcow2_paths: &[&PathBuf]) -> Result<String> {
    let os_block = build_os_block(config, vm_name);

    // PCI bus allocator: buses 0x00–0x03 are reserved (root, USB, ISA, virtio-serial).
    // Disks start at 0x04, then NICs, then memballoon + rng follow.
    let mut next_bus: u32 = 0x04;

    let disks = build_disk_devices(qcow2_paths, &mut next_bus)?;
    let networks = build_network_interfaces(config.nic_count, &mut next_bus);
    let memballoon_bus = next_bus;
    next_bus += 1;
    let rng_bus = next_bus;

    let xml = format!(
        r#"<domain type="kvm">
  <name>{vm_name}</name>
  <memory unit="MiB">{memory}</memory>
  <currentMemory unit="MiB">{memory}</currentMemory>
  <vcpu placement="static">{vcpu}</vcpu>
{os_block}
  <features>
    <acpi/>
    <apic/>
    <vmport state="off"/>
  </features>
  <cpu mode="host-passthrough" check="none" migratable="on"/>
  <clock offset="utc">
    <timer name="rtc" tickpolicy="catchup"/>
    <timer name="pit" tickpolicy="delay"/>
    <timer name="hpet" present="no"/>
  </clock>
  <on_poweroff>destroy</on_poweroff>
  <on_reboot>restart</on_reboot>
  <on_crash>destroy</on_crash>
  <pm>
    <suspend-to-mem enabled="no"/>
    <suspend-to-disk enabled="no"/>
  </pm>
  <devices>
    <emulator>/usr/bin/qemu-system-x86_64</emulator>
{disks}
    <controller type="virtio-serial" index="0">
      <address type="pci" domain="0x0000" bus="0x03" slot="0x00" function="0x0"/>
    </controller>
{networks}
    <serial type="pty">
      <target type="isa-serial" port="0">
        <model name="isa-serial"/>
      </target>
    </serial>
    <console type="pty">
      <target type="serial" port="0"/>
    </console>
    <channel type="unix">
      <target type="virtio" name="org.qemu.guest_agent.0"/>
      <address type="virtio-serial" controller="0" bus="0" port="1"/>
    </channel>
    <input type="tablet" bus="usb">
      <address type="usb" bus="0" port="1"/>
    </input>
    <graphics type="spice" autoport="yes">
      <listen type="address"/>
      <image compression="off"/>
    </graphics>
    <sound model="ich9">
      <address type="pci" domain="0x0000" bus="0x00" slot="0x1b" function="0x0"/>
    </sound>
    <video>
      <model type="qxl" ram="65536" vram="65536" vgamem="16384" heads="1" primary="yes"/>
      <address type="pci" domain="0x0000" bus="0x00" slot="0x01" function="0x0"/>
    </video>
    <memballoon model="virtio">
      <address type="pci" domain="0x0000" bus="0x{memballoon_bus:02x}" slot="0x00" function="0x0"/>
    </memballoon>
    <rng model="virtio">
      <backend model="random">/dev/urandom</backend>
      <address type="pci" domain="0x0000" bus="0x{rng_bus:02x}" slot="0x00" function="0x0"/>
    </rng>
  </devices>
</domain>
"#,
        vm_name = vm_name,
        memory = config.memory_mb,
        vcpu = config.vcpu_count,
        os_block = os_block,
        disks = disks,
        networks = networks,
        memballoon_bus = memballoon_bus,
        rng_bus = rng_bus,
    );

    Ok(xml)
}

// ─── Internal helpers ─────────────────────────────────────────────────────────

fn build_os_block(config: &VmConfig, vm_name: &str) -> String {
    if config.uefi {
        format!(
            r#"  <os>
    <type arch="x86_64" machine="q35">hvm</type>
    <loader readonly="yes" type="pflash">/usr/share/OVMF/OVMF_CODE.fd</loader>
    <nvram>/var/lib/libvirt/qemu/nvram/{vm_name}_VARS.fd</nvram>
  </os>"#
        )
    } else {
        r#"  <os>
    <type arch="x86_64" machine="q35">hvm</type>
  </os>"#
            .to_string()
    }
}

/// Generate one `<disk>` element per QCOW2 image.
///
/// Device names follow the VirtIO convention: vda, vdb, vdc, …
fn build_disk_devices(qcow2_paths: &[&PathBuf], next_bus: &mut u32) -> Result<String> {
    let mut blocks = Vec::new();
    for (i, path) in qcow2_paths.iter().enumerate() {
        let path_str = path
            .to_str()
            .ok_or_else(|| anyhow::anyhow!("QCOW2 path contains non-UTF-8 characters"))?;
        // vda, vdb, vdc, …
        let dev_letter = (b'a' + i as u8) as char;
        let bus = *next_bus;
        *next_bus += 1;
        blocks.push(format!(
            r#"    <disk type="file" device="disk">
      <driver name="qemu" type="qcow2" discard="unmap"/>
      <source file="{path_str}"/>
      <target dev="vd{dev_letter}" bus="virtio"/>
      <address type="pci" domain="0x0000" bus="0x{bus:02x}" slot="0x00" function="0x0"/>
    </disk>"#
        ));
    }
    Ok(blocks.join("\n"))
}

/// Generate one `<interface>` element per NIC.  Always emit at least one.
fn build_network_interfaces(nic_count: u32, next_bus: &mut u32) -> String {
    let count = nic_count.max(1);
    (0..count)
        .map(|_| {
            let bus = *next_bus;
            *next_bus += 1;
            format!(
                r#"    <interface type="network">
      <source network="default"/>
      <model type="virtio"/>
      <address type="pci" domain="0x0000" bus="0x{bus:02x}" slot="0x00" function="0x0"/>
    </interface>"#
            )
        })
        .collect::<Vec<_>>()
        .join("\n")
        + "\n"
}

// ─── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn bios_config() -> VmConfig {
        VmConfig {
            name: "test-vm".into(),
            vcpu_count: 2,
            memory_mb: 2048,
            disk_files: vec!["disk.vmdk".into()],
            nic_count: 1,
            uefi: false,
        }
    }

    fn uefi_config() -> VmConfig {
        VmConfig {
            name: "uefi-vm".into(),
            vcpu_count: 4,
            memory_mb: 8192,
            disk_files: vec!["disk.vmdk".into()],
            nic_count: 2,
            uefi: true,
        }
    }

    fn gen(cfg: &VmConfig, name: &str, paths: &[PathBuf]) -> String {
        let refs: Vec<&PathBuf> = paths.iter().collect();
        generate(cfg, name, &refs).unwrap()
    }

    // ── Top-level structure ──────────────────────────────────────────────────

    #[test]
    fn test_xml_opens_and_closes_domain() {
        let xml = gen(
            &bios_config(),
            "test-vm",
            &[PathBuf::from("/tmp/test.qcow2")],
        );
        assert!(xml.starts_with(r#"<domain type="kvm">"#));
        assert!(xml.trim_end().ends_with("</domain>"));
    }

    #[test]
    fn test_domain_name_element() {
        let xml = gen(
            &bios_config(),
            "test-vm",
            &[PathBuf::from("/tmp/test.qcow2")],
        );
        assert!(xml.contains("<name>test-vm</name>"));
    }

    // ── Memory ───────────────────────────────────────────────────────────────

    #[test]
    fn test_memory_value() {
        let xml = gen(
            &bios_config(),
            "test-vm",
            &[PathBuf::from("/tmp/test.qcow2")],
        );
        assert!(xml.contains(r#"<memory unit="MiB">2048</memory>"#));
        assert!(xml.contains(r#"<currentMemory unit="MiB">2048</currentMemory>"#));
    }

    // ── CPU ──────────────────────────────────────────────────────────────────

    #[test]
    fn test_vcpu_value() {
        let xml = gen(
            &bios_config(),
            "test-vm",
            &[PathBuf::from("/tmp/test.qcow2")],
        );
        assert!(xml.contains(r#"<vcpu placement="static">2</vcpu>"#));
    }

    // ── Disk ─────────────────────────────────────────────────────────────────

    #[test]
    fn test_disk_source_path() {
        let xml = gen(
            &bios_config(),
            "test-vm",
            &[PathBuf::from("/var/lib/libvirt/images/test-vm.qcow2")],
        );
        assert!(xml.contains(r#"<source file="/var/lib/libvirt/images/test-vm.qcow2"/>"#));
    }

    #[test]
    fn test_disk_uses_virtio() {
        let xml = gen(
            &bios_config(),
            "test-vm",
            &[PathBuf::from("/tmp/test.qcow2")],
        );
        assert!(xml.contains(r#"<target dev="vda" bus="virtio"/>"#));
    }

    #[test]
    fn test_multiple_disks() {
        let xml = gen(
            &bios_config(),
            "test-vm",
            &[
                PathBuf::from("/tmp/os.qcow2"),
                PathBuf::from("/tmp/data.qcow2"),
            ],
        );
        assert!(xml.contains(r#"<source file="/tmp/os.qcow2"/>"#));
        assert!(xml.contains(r#"<target dev="vda" bus="virtio"/>"#));
        assert!(xml.contains(r#"<source file="/tmp/data.qcow2"/>"#));
        assert!(xml.contains(r#"<target dev="vdb" bus="virtio"/>"#));
        assert_eq!(xml.matches(r#"<disk type="file""#).count(), 2);
    }

    #[test]
    fn test_multiple_disks_no_bus_collision() {
        let xml = gen(
            &bios_config(),
            "test-vm",
            &[
                PathBuf::from("/tmp/os.qcow2"),
                PathBuf::from("/tmp/data.qcow2"),
            ],
        );
        // Disks at bus 0x04, 0x05; NIC at 0x06; memballoon at 0x07; rng at 0x08
        assert!(xml.contains(r#"bus="0x04""#)); // disk 1
        assert!(xml.contains(r#"bus="0x05""#)); // disk 2
        assert!(xml.contains(r#"bus="0x06""#)); // NIC
        assert!(xml.contains(r#"bus="0x07""#)); // memballoon
        assert!(xml.contains(r#"bus="0x08""#)); // rng
    }

    // ── Firmware ─────────────────────────────────────────────────────────────

    #[test]
    fn test_bios_has_no_ovmf_loader() {
        let xml = gen(
            &bios_config(),
            "test-vm",
            &[PathBuf::from("/tmp/test.qcow2")],
        );
        assert!(!xml.contains("OVMF_CODE.fd"));
        assert!(!xml.contains("nvram"));
    }

    #[test]
    fn test_uefi_has_ovmf_loader() {
        let xml = gen(
            &uefi_config(),
            "uefi-vm",
            &[PathBuf::from("/tmp/uefi.qcow2")],
        );
        assert!(xml.contains("/usr/share/OVMF/OVMF_CODE.fd"));
    }

    #[test]
    fn test_uefi_nvram_contains_vm_name() {
        let xml = gen(
            &uefi_config(),
            "uefi-vm",
            &[PathBuf::from("/tmp/uefi.qcow2")],
        );
        assert!(xml.contains("uefi-vm_VARS.fd"));
    }

    // ── Networking ───────────────────────────────────────────────────────────

    #[test]
    fn test_one_nic_produces_one_interface() {
        let xml = gen(
            &bios_config(),
            "test-vm",
            &[PathBuf::from("/tmp/test.qcow2")],
        );
        assert_eq!(xml.matches(r#"<interface type="network">"#).count(), 1);
    }

    #[test]
    fn test_two_nics_produce_two_interfaces() {
        let xml = gen(
            &uefi_config(),
            "uefi-vm",
            &[PathBuf::from("/tmp/uefi.qcow2")],
        );
        assert_eq!(xml.matches(r#"<interface type="network">"#).count(), 2);
    }

    #[test]
    fn test_zero_nics_defaults_to_one_interface() {
        let mut cfg = bios_config();
        cfg.nic_count = 0;
        let xml = gen(&cfg, "test-vm", &[PathBuf::from("/tmp/test.qcow2")]);
        assert_eq!(xml.matches(r#"<interface type="network">"#).count(), 1);
    }

    #[test]
    fn test_interfaces_use_virtio_model() {
        let xml = gen(
            &bios_config(),
            "test-vm",
            &[PathBuf::from("/tmp/test.qcow2")],
        );
        assert!(xml.contains(r#"<model type="virtio"/>"#));
    }

    // ── Misc devices ─────────────────────────────────────────────────────────

    #[test]
    fn test_spice_graphics_present() {
        let xml = gen(
            &bios_config(),
            "test-vm",
            &[PathBuf::from("/tmp/test.qcow2")],
        );
        assert!(xml.contains(r#"<graphics type="spice""#));
    }

    #[test]
    fn test_machine_type_is_q35() {
        let xml = gen(
            &bios_config(),
            "test-vm",
            &[PathBuf::from("/tmp/test.qcow2")],
        );
        assert!(xml.contains(r#"machine="q35""#));
    }

    #[test]
    fn test_cpu_host_passthrough() {
        let xml = gen(
            &bios_config(),
            "test-vm",
            &[PathBuf::from("/tmp/test.qcow2")],
        );
        assert!(xml.contains(r#"<cpu mode="host-passthrough""#));
    }

    // ── build_os_block ───────────────────────────────────────────────────────

    #[test]
    fn test_build_os_block_bios() {
        let cfg = bios_config();
        let block = build_os_block(&cfg, "test-vm");
        assert!(!block.contains("loader"));
        assert!(block.contains(r#"machine="q35""#));
    }

    #[test]
    fn test_build_os_block_uefi() {
        let cfg = uefi_config();
        let block = build_os_block(&cfg, "uefi-vm");
        assert!(block.contains("loader"));
        assert!(block.contains("uefi-vm_VARS.fd"));
    }
}
