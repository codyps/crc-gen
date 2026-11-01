use crc_gen::{BitOrder, CrcSpec};

pub fn mask(bits: u8) -> u64 {
    u64::MAX >> (64 - bits)
}

// Independent oracle: carry-less long division on a u128 polynomial, rather
// than the generator's symbolic shift-register recurrence.
pub fn reference(spec: CrcSpec, crc: u64, data: u64) -> u64 {
    let width = spec.width();
    let bits = spec.data_bits();
    let mut crc = crc & mask(width);
    let mut data = data & mask(bits);
    if spec.order() == BitOrder::LsbFirst {
        crc = crc.reverse_bits() >> (64 - width);
        data = data.reverse_bits() >> (64 - bits);
    }
    let mut numerator = ((crc as u128) << bits) ^ ((data as u128) << width);
    let divisor = (1u128 << width) | spec.polynomial() as u128;
    for bit in (u16::from(width)..u16::from(width) + u16::from(bits)).rev() {
        if numerator & (1u128 << bit) != 0 {
            numerator ^= divisor << (bit - u16::from(width));
        }
    }
    let mut result = numerator as u64;
    if spec.order() == BitOrder::LsbFirst {
        result = result.reverse_bits() >> (64 - width);
    }
    result
}
