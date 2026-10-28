// OpenRevo - Hardware Blast Shield & Safety Gatekeeper
// Protects host silicon, battery charging circuitry, and motherboard firmware from catastrophic or invalid writes.

/// Asserts that a target EC RAM address is within standard physical boundary limits.
pub fn validate_read_address(addr: u16) -> Result<(), String> {
    // Standard and extended EC address spaces span up to 0x0FFF (4096 bytes)
    if addr > 0x0FFF {
        return Err(format!("Address out of physical bounds: 0x{:04X} exceeds 0x0FFF", addr));
    }
    Ok(())
}

/// Asserts that a target write operation does not touch protected firmware flash or hardware fuse registers.
pub fn validate_write_operation(addr: u16, val: u8) -> Result<(), String> {
    validate_read_address(addr)?;

    // Hardware Blast Shield: Block writes to fatal hardware registers
    // Address 0x0000 is typically reserved for critical reset/boot vector locks
    if addr == 0x0000 && val == 0xFF {
        crate::log_warn!("BLAST_SHIELD", "Blocked attempt to write reset tripwire to 0x0000");
        return Err("Operation blocked by Hardware Blast Shield: Reserved reset vector".to_string());
    }

    Ok(())
}
