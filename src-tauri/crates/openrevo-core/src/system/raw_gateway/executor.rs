// OpenRevo - Raw Driver Gateway Execution Engine
// Bridges high-level plugin requests directly into Tongfang IOCTL, Baolongda Mailbox, Generic WMI, or ITE buses.

use super::types::{RawEcProtocol, RawEcReadRequest, RawEcWriteRequest, RawMailboxRequest};
use super::shield::{validate_read_address, validate_write_operation};

pub fn execute_raw_read(req: RawEcReadRequest) -> Result<u8, String> {
    validate_read_address(req.address)?;

    match req.protocol {
        RawEcProtocol::TongfangDirectEc => {
            crate::driver::wmi_ec::wmi_read_ec(req.address)
                .ok_or_else(|| format!("Tongfang DirectEC failed to read address 0x{:04X}", req.address))
        }
        RawEcProtocol::GenericAcpiWmi => {
            crate::driver::wmi_mulong::mulong_read_ec(req.address)
                .ok_or_else(|| format!("Generic WMI failed to read address 0x{:04X}", req.address))
        }
        RawEcProtocol::BldWmiMailbox => {
            Err("Baolongda protocol is packet-oriented. Use 'raw_mailbox_invoke' instead of single-byte read.".to_string())
        }
        RawEcProtocol::IteLightingBus => {
            Err("ITE Lighting Bus is packet-oriented. Use dedicated lighting streamer APIs.".to_string())
        }
    }
}

pub fn execute_raw_write(req: RawEcWriteRequest) -> Result<bool, String> {
    validate_write_operation(req.address, req.value)?;

    match req.protocol {
        RawEcProtocol::TongfangDirectEc => {
            if crate::driver::wmi_ec::wmi_write_ec(req.address, req.value) {
                crate::log_info!("RAW_GATEWAY", "Wrote 0x{:02X} to 0x{:04X} via Tongfang DirectEC", req.value, req.address);
                Ok(true)
            } else {
                Err(format!("Tongfang DirectEC failed to write 0x{:02X} to 0x{:04X}", req.value, req.address))
            }
        }
        RawEcProtocol::GenericAcpiWmi => {
            if crate::driver::wmi_mulong::mulong_write_ec(req.address, req.value) {
                crate::log_info!("RAW_GATEWAY", "Wrote 0x{:02X} to 0x{:04X} via Generic ACPI WMI", req.value, req.address);
                Ok(true)
            } else {
                Err(format!("Generic ACPI WMI failed to write 0x{:02X} to 0x{:04X}", req.value, req.address))
            }
        }
        RawEcProtocol::BldWmiMailbox => {
            Err("Baolongda protocol is packet-oriented. Use 'raw_mailbox_invoke' instead of single-byte write.".to_string())
        }
        RawEcProtocol::IteLightingBus => {
            Err("ITE Lighting Bus is packet-oriented. Use dedicated lighting streamer APIs.".to_string())
        }
    }
}

pub fn execute_raw_mailbox(req: RawMailboxRequest) -> Result<Vec<u8>, String> {
    if req.in_bytes.is_empty() {
        return Err("Mailbox input buffer cannot be empty".to_string());
    }
    if req.in_bytes.len() > 256 {
        return Err("Mailbox buffer exceeds maximum 256-byte security boundary".to_string());
    }

    crate::hal::declarative::invoke_raw_mailbox(&req.in_bytes)
}
