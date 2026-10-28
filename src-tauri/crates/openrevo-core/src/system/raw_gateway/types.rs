// OpenRevo - Raw EC & Hardware Driver Gateway Protocol Types
// Provides unified abstractions over Tongfang IOCTL, Baolongda Mailbox, Generic WMI, and ITE HID buses.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "type")]
pub enum RawEcProtocol {
    /// 1. Tongfang DirectEC IOCTL (High-speed Ring0 native transport)
    TongfangDirectEc,
    /// 2. Baolongda / JiaoLong WMI Mailbox Protocol (Multi-byte buffer handshake)
    BldWmiMailbox,
    /// 3. Generic ACPI WMI MULONG Transport
    GenericAcpiWmi,
    /// 4. ITE Dedicated RGB Lighting HID Direct Bus
    IteLightingBus,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RawEcReadRequest {
    pub protocol: RawEcProtocol,
    pub address: u16,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RawEcWriteRequest {
    pub protocol: RawEcProtocol,
    pub address: u16,
    pub value: u8,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RawMailboxRequest {
    pub in_bytes: Vec<u8>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RawEcCapabilities {
    pub available_protocols: Vec<RawEcProtocol>,
    pub is_direct_ec_available: bool,
    pub is_bld_mailbox_available: bool,
    pub is_generic_wmi_available: bool,
    pub is_ite_hid_available: bool,
}
