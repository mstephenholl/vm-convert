/// ovf.rs — Parse VMware OVF XML to extract VM configuration.
///
/// OVF uses multiple XML namespaces; we use roxmltree which handles
/// namespaced attributes/elements transparently via local-name lookup.
///
/// CIM ResourceType values we care about:
///   3  = Processor (VirtualQuantity = vCPU count)
///   4  = Memory    (VirtualQuantity in AllocationUnits, default MiB)
///   10 = Ethernet Adapter
///   17 = Disk Drive (we resolve the actual file via References)
use anyhow::{Context, Result};
use std::path::Path;

// ─── Public data model ────────────────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq)]
pub struct VmConfig {
    /// Sanitised VM name (safe for use as a filename / libvirt domain name)
    pub name: String,
    /// Number of virtual CPUs
    pub vcpu_count: u32,
    /// RAM in MiB
    pub memory_mb: u64,
    /// Relative path to the .vmdk file (href from OVF References section)
    pub disk_file: String,
    /// Number of virtual NICs
    pub nic_count: u32,
    /// True when a .nvram sidecar file was present alongside the .ovf
    pub uefi: bool,
}

// ─── Public API ───────────────────────────────────────────────────────────────

/// Parse an OVF file on disk.  Detects UEFI by checking for a .nvram sidecar.
pub fn parse_ovf(ovf_path: &Path) -> Result<VmConfig> {
    let content = std::fs::read_to_string(ovf_path)
        .with_context(|| format!("Cannot read OVF file: {}", ovf_path.display()))?;

    let nvram_path = ovf_path.with_extension("nvram");
    let uefi = nvram_path.exists();

    parse_ovf_str(&content, uefi)
}

/// Parse OVF XML from a string (useful for tests).
pub fn parse_ovf_str(content: &str, uefi: bool) -> Result<VmConfig> {
    let doc = roxmltree::Document::parse(content).context("Failed to parse OVF XML")?;
    let root = doc.root_element();

    let name = extract_vm_name(&root).unwrap_or_else(|| "converted-vm".to_string());
    let disk_file = extract_disk_file(&root).context("OVF contains no .vmdk file reference")?;
    let (vcpu_count, memory_mb, nic_count) = extract_hardware(&root)?;

    Ok(VmConfig {
        name,
        vcpu_count,
        memory_mb,
        disk_file,
        nic_count,
        uefi,
    })
}

// ─── Internal helpers ─────────────────────────────────────────────────────────

fn extract_vm_name<'a>(root: &'a roxmltree::Node<'a, 'a>) -> Option<String> {
    // Prefer the <Name> text child of <VirtualSystem>
    root.descendants()
        .find(|n| n.tag_name().name() == "Name")
        .and_then(|n| n.text())
        .map(|t| t.trim())
        .filter(|t| !t.is_empty())
        .map(sanitize_vm_name)
        // Fall back to the ovf:id attribute on <VirtualSystem>
        .or_else(|| {
            root.descendants()
                .find(|n| n.tag_name().name() == "VirtualSystem")
                .and_then(|n| n.attributes().find(|a| a.name() == "id"))
                .map(|a| sanitize_vm_name(a.value()))
        })
}

/// Replace any character that is not alphanumeric, `-`, or `_` with `-`.
pub fn sanitize_vm_name(name: &str) -> String {
    name.chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' || c == '_' {
                c
            } else {
                '-'
            }
        })
        .collect()
}

fn extract_disk_file<'a>(root: &'a roxmltree::Node<'a, 'a>) -> Option<String> {
    // OVF References section: <File ovf:href="disk.vmdk" ovf:id="file1"/>
    root.descendants()
        .filter(|n| n.tag_name().name() == "File")
        .find_map(|n| {
            n.attributes()
                .find(|a| a.name() == "href")
                .filter(|a| a.value().to_lowercase().ends_with(".vmdk"))
                .map(|a| a.value().to_string())
        })
}

fn extract_hardware<'a>(root: &'a roxmltree::Node<'a, 'a>) -> Result<(u32, u64, u32)> {
    let mut vcpu_count: u32 = 1;
    let mut memory_mb: u64 = 1024;
    let mut nic_count: u32 = 0;

    let child_text = |item: &roxmltree::Node, tag: &str| -> Option<String> {
        item.descendants()
            .find(|n| n.tag_name().name() == tag)
            .and_then(|n| n.text())
            .map(|t| t.trim().to_string())
    };

    for item in root.descendants().filter(|n| n.tag_name().name() == "Item") {
        let resource_type: Option<u32> =
            child_text(&item, "ResourceType").and_then(|t| t.parse().ok());

        let virtual_quantity: Option<u64> =
            child_text(&item, "VirtualQuantity").and_then(|t| t.parse().ok());

        match resource_type {
            Some(3) => {
                // CPU
                if let Some(qty) = virtual_quantity {
                    vcpu_count = qty as u32;
                }
            }
            Some(4) => {
                // Memory
                if let Some(qty) = virtual_quantity {
                    let units = child_text(&item, "AllocationUnits")
                        .unwrap_or_else(|| "byte * 2^20".to_string());
                    memory_mb = normalize_memory_to_mib(qty, &units);
                }
            }
            Some(10) => {
                // Ethernet
                nic_count += 1;
            }
            _ => {}
        }
    }

    Ok((vcpu_count, memory_mb, nic_count))
}

/// Convert OVF AllocationUnits value to MiB.
///
/// Common OVF memory unit strings:
///   "byte * 2^20"  → MiB  (most common, VMware default)
///   "byte * 2^30"  → GiB
///   "byte * 2^10"  → KiB
///   "MegaBytes"    → MB ≈ MiB (close enough for VM provisioning)
///   "GigaBytes"    → GB ≈ GiB
pub fn normalize_memory_to_mib(qty: u64, units: &str) -> u64 {
    let u = units.trim().to_lowercase();
    if u.contains("2^30") || u.contains("giga") {
        qty * 1024
    } else if u.contains("2^10") || u.contains("kilo") {
        qty / 1024
    } else {
        // Default: "byte * 2^20" or "MegaBytes" → MiB
        qty
    }
}

// ─── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    // Minimal but complete OVF covering all resource types we parse.
    const SAMPLE_OVF: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<Envelope
  xmlns="http://schemas.dmtf.org/ovf/envelope/1"
  xmlns:ovf="http://schemas.dmtf.org/ovf/envelope/1"
  xmlns:rasd="http://schemas.dmtf.org/wbem/wscim/1/cim-schema/2/CIM_ResourceAllocationSettingData">
  <References>
    <File ovf:href="ubuntu-server.vmdk" ovf:id="file1"/>
  </References>
  <VirtualSystem ovf:id="ubuntu-server-fallback">
    <Name>Ubuntu Server Test</Name>
    <VirtualHardwareSection>
      <Item>
        <rasd:ResourceType>3</rasd:ResourceType>
        <rasd:VirtualQuantity>4</rasd:VirtualQuantity>
      </Item>
      <Item>
        <rasd:ResourceType>4</rasd:ResourceType>
        <rasd:VirtualQuantity>4096</rasd:VirtualQuantity>
        <rasd:AllocationUnits>byte * 2^20</rasd:AllocationUnits>
      </Item>
      <Item>
        <rasd:ResourceType>10</rasd:ResourceType>
        <rasd:VirtualQuantity>1</rasd:VirtualQuantity>
      </Item>
    </VirtualHardwareSection>
  </VirtualSystem>
</Envelope>"#;

    const OVF_GIB_MEMORY: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<Envelope xmlns="http://schemas.dmtf.org/ovf/envelope/1"
  xmlns:ovf="http://schemas.dmtf.org/ovf/envelope/1"
  xmlns:rasd="http://schemas.dmtf.org/wbem/wscim/1/cim-schema/2/CIM_ResourceAllocationSettingData">
  <References>
    <File ovf:href="disk.vmdk" ovf:id="file1"/>
  </References>
  <VirtualSystem ovf:id="gib-vm">
    <Name>GiB VM</Name>
    <VirtualHardwareSection>
      <Item>
        <rasd:ResourceType>3</rasd:ResourceType>
        <rasd:VirtualQuantity>2</rasd:VirtualQuantity>
      </Item>
      <Item>
        <rasd:ResourceType>4</rasd:ResourceType>
        <rasd:VirtualQuantity>8</rasd:VirtualQuantity>
        <rasd:AllocationUnits>byte * 2^30</rasd:AllocationUnits>
      </Item>
    </VirtualHardwareSection>
  </VirtualSystem>
</Envelope>"#;

    const OVF_NO_DISK: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<Envelope xmlns="http://schemas.dmtf.org/ovf/envelope/1">
  <VirtualSystem ovf:id="no-disk">
    <Name>No Disk</Name>
  </VirtualSystem>
</Envelope>"#;

    #[test]
    fn test_parse_basic_fields() {
        let cfg = parse_ovf_str(SAMPLE_OVF, false).unwrap();
        assert_eq!(cfg.name, "Ubuntu-Server-Test");
        assert_eq!(cfg.vcpu_count, 4);
        assert_eq!(cfg.memory_mb, 4096);
        assert_eq!(cfg.disk_file, "ubuntu-server.vmdk");
        assert_eq!(cfg.nic_count, 1);
        assert!(!cfg.uefi);
    }

    #[test]
    fn test_uefi_flag_propagated() {
        let cfg = parse_ovf_str(SAMPLE_OVF, true).unwrap();
        assert!(cfg.uefi);
    }

    #[test]
    fn test_gib_memory_converted_to_mib() {
        let cfg = parse_ovf_str(OVF_GIB_MEMORY, false).unwrap();
        assert_eq!(cfg.memory_mb, 8192); // 8 GiB → 8192 MiB
    }

    #[test]
    fn test_missing_disk_file_is_error() {
        let result = parse_ovf_str(OVF_NO_DISK, false);
        assert!(
            result.is_err(),
            "Expected error when no vmdk reference exists"
        );
    }

    #[test]
    fn test_zero_nics_when_no_ethernet_items() {
        let cfg = parse_ovf_str(OVF_GIB_MEMORY, false).unwrap();
        assert_eq!(cfg.nic_count, 0);
    }

    #[test]
    fn test_sanitize_removes_spaces() {
        assert_eq!(sanitize_vm_name("My VM Name"), "My-VM-Name");
    }

    #[test]
    fn test_sanitize_preserves_dashes_and_underscores() {
        assert_eq!(sanitize_vm_name("ubuntu-server_v2"), "ubuntu-server_v2");
    }

    #[test]
    fn test_sanitize_replaces_dots() {
        assert_eq!(sanitize_vm_name("vm.name.1"), "vm-name-1");
    }

    #[test]
    fn test_normalize_mib_default() {
        assert_eq!(normalize_memory_to_mib(4096, "byte * 2^20"), 4096);
    }

    #[test]
    fn test_normalize_gib() {
        assert_eq!(normalize_memory_to_mib(4, "byte * 2^30"), 4096);
    }

    #[test]
    fn test_normalize_gigabytes_string() {
        assert_eq!(normalize_memory_to_mib(2, "GigaBytes"), 2048);
    }

    #[test]
    fn test_normalize_kib() {
        assert_eq!(normalize_memory_to_mib(2_097_152, "byte * 2^10"), 2048);
    }

    #[test]
    fn test_fallback_name_from_virtual_system_id() {
        // OVF with no <Name> element — should fall back to ovf:id attribute
        let ovf = r#"<?xml version="1.0" encoding="UTF-8"?>
<Envelope xmlns="http://schemas.dmtf.org/ovf/envelope/1"
  xmlns:ovf="http://schemas.dmtf.org/ovf/envelope/1"
  xmlns:rasd="http://schemas.dmtf.org/wbem/wscim/1/cim-schema/2/CIM_ResourceAllocationSettingData">
  <References>
    <File ovf:href="disk.vmdk" ovf:id="file1"/>
  </References>
  <VirtualSystem ovf:id="fallback-name">
    <VirtualHardwareSection>
      <Item>
        <rasd:ResourceType>3</rasd:ResourceType>
        <rasd:VirtualQuantity>1</rasd:VirtualQuantity>
      </Item>
      <Item>
        <rasd:ResourceType>4</rasd:ResourceType>
        <rasd:VirtualQuantity>512</rasd:VirtualQuantity>
        <rasd:AllocationUnits>byte * 2^20</rasd:AllocationUnits>
      </Item>
    </VirtualHardwareSection>
  </VirtualSystem>
</Envelope>"#;
        let cfg = parse_ovf_str(ovf, false).unwrap();
        assert_eq!(cfg.name, "fallback-name");
    }

    #[test]
    fn test_invalid_xml_is_error() {
        let result = parse_ovf_str("<broken xml >>>", false);
        assert!(result.is_err());
    }
}
