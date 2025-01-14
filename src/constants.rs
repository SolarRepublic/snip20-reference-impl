#[cfg(test)]
pub const ADDRESS_BYTES_LEN: usize = 54;
#[cfg(not(test))]
pub const ADDRESS_BYTES_LEN: usize = 20;

/// canonical address bytes corresponding to the 33-byte null public key, in hexadecimal
#[cfg(test)]
pub const IMPOSSIBLE_ADDR: [u8; ADDRESS_BYTES_LEN] = [
    0x29, 0xCF, 0xC6, 0x37, 0x62, 0x55, 0xA7, 0x84, 0x51, 0xEE, 0xB4, 0xB1, 0x29, 0xED, 0x8E, 0xAC,
    0xFF, 0xA2, 0xFE, 0xEF, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
    0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
    0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
];
#[cfg(not(test))]
pub const IMPOSSIBLE_ADDR: [u8; ADDRESS_BYTES_LEN] = [
    0x29, 0xCF, 0xC6, 0x37, 0x62, 0x55, 0xA7, 0x84, 0x51, 0xEE, 0xB4, 0xB1, 0x29, 0xED, 0x8E, 0xAC,
    0xFF, 0xA2, 0xFE, 0xEF,
];
