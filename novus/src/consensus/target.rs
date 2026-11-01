use crypto::{POW_HASH_SIZE, PoWHash};

const COMPACT_MANTISSA_MASK: u32 = 0x007f_ffff;
const COMPACT_SIGN_MASK: u32 = 0x0080_0000;

/// A canonical 256-bit proof-of-work target.
///
/// The value is stored in big-endian byte order.
///
/// A PoW hash is valid when:
///
/// ```text
/// hash <= target
/// ```
///
/// Therefore:
///
/// - smaller target = harder PoW
/// - larger target  = easier PoW
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct PoWTarget([u8; POW_HASH_SIZE]);

impl PoWTarget {
    /// Constructs a target from raw big-endian bytes.
    ///
    /// Returns `None` for the zero target because a zero target would make
    /// proof-of-work practically impossible and is not a valid consensus
    /// target.
    pub fn from_bytes(bytes: [u8; POW_HASH_SIZE]) -> Option<Self> {
        let target = Self(bytes);

        if target.is_zero() { None } else { Some(target) }
    }

    /// Returns the canonical big-endian target bytes.
    pub const fn as_bytes(&self) -> &[u8; POW_HASH_SIZE] {
        &self.0
    }

    /// Returns true when the target is zero.
    const fn is_zero(&self) -> bool {
        let mut index = 0;

        while index < POW_HASH_SIZE {
            if self.0[index] != 0 {
                return false;
            }

            index += 1;
        }

        true
    }

    /// Returns true when `hash` satisfies this proof-of-work target.
    ///
    /// Both the hash and target use canonical big-endian byte ordering,
    /// therefore lexicographic byte comparison is equivalent to numeric
    /// comparison.
    pub fn meets(self, hash: &PoWHash) -> bool {
        hash.as_bytes() <= self.as_bytes()
    }

    /// Scales this 256-bit target by a rational value:
    ///
    /// ```text
    /// target' = target * numerator / denominator
    /// ```
    ///
    /// This is used by the timestamp-based difficulty retarget.
    ///
    /// A 36-byte intermediate is used because multiplying a 256-bit target
    /// by a 32-bit numerator may temporarily require up to 288 bits.
    pub fn scale_ratio(self, numerator: u32, denominator: u32) -> Option<Self> {
        if numerator == 0 || denominator == 0 {
            return None;
        }

        let numerator = u64::from(numerator);
        let denominator = u64::from(denominator);

        let mut wide = [0_u8; POW_HASH_SIZE + 4];
        let mut carry = 0_u64;

        // -----------------------------------------------------------------
        // Multiply the 256-bit big-endian target by the numerator.
        // -----------------------------------------------------------------

        for index in (0..POW_HASH_SIZE).rev() {
            let product = u64::from(self.0[index])
                .checked_mul(numerator)?
                .checked_add(carry)?;

            wide[index + 4] = (product & 0xff) as u8;
            carry = product >> 8;
        }

        // Remaining carry fits within the four extra bytes because
        // numerator is at most u32::MAX.
        for index in (0..4).rev() {
            wide[index] = (carry & 0xff) as u8;
            carry >>= 8;
        }

        if carry != 0 {
            return None;
        }

        // -----------------------------------------------------------------
        // Divide the 288-bit intermediate by the denominator.
        // -----------------------------------------------------------------

        let mut remainder = 0_u64;

        for byte in &mut wide {
            let value = (remainder << 8) | u64::from(*byte);

            *byte = (value / denominator) as u8;
            remainder = value % denominator;
        }

        // -----------------------------------------------------------------
        // The result must fit back into a 256-bit target.
        // -----------------------------------------------------------------

        if wide[..4].iter().any(|byte| *byte != 0) {
            return None;
        }

        let mut bytes = [0_u8; POW_HASH_SIZE];
        bytes.copy_from_slice(&wide[4..]);

        // Integer division could theoretically reduce a very small target
        // to zero. Consensus must never use a zero target, so saturate at
        // the smallest valid target.
        if bytes.iter().all(|byte| *byte == 0) {
            bytes[POW_HASH_SIZE - 1] = 1;
        }

        Some(Self(bytes))
    }

    /// Decodes Bitcoin-style compact target representation.
    ///
    /// Layout:
    ///
    /// ```text
    /// [ exponent: 8 bits ][ sign: 1 bit ][ mantissa: 23 bits ]
    /// ```
    ///
    /// Novus targets are unsigned, therefore the compact sign bit is
    /// rejected.
    pub fn from_compact(compact: u32) -> Option<Self> {
        let size = (compact >> 24) as usize;
        let mantissa = compact & COMPACT_MANTISSA_MASK;

        // Negative compact targets are invalid.
        if compact & COMPACT_SIGN_MASK != 0 {
            return None;
        }

        if size == 0 || mantissa == 0 {
            return None;
        }

        // Bitcoin-style 256-bit overflow limits.
        //
        // Equivalent to Bitcoin Core's SetCompact overflow conditions.
        let overflow =
            size > 34 || (mantissa > 0xff && size > 33) || (mantissa > 0xffff && size > 32);

        if overflow {
            return None;
        }

        let mut bytes = [0_u8; POW_HASH_SIZE];

        if size <= 3 {
            let shift = 8 * (3 - size);
            let value = mantissa >> shift;

            for index in 0..size {
                let value_shift = 8 * (size - 1 - index);

                bytes[POW_HASH_SIZE - size + index] = ((value >> value_shift) & 0xff) as u8;
            }
        } else {
            let mantissa_bytes = [
                ((mantissa >> 16) & 0xff) as u8,
                ((mantissa >> 8) & 0xff) as u8,
                (mantissa & 0xff) as u8,
            ];

            let base = POW_HASH_SIZE as isize - size as isize;

            for (offset, byte) in mantissa_bytes.into_iter().enumerate() {
                let index = base + offset as isize;

                // Compact exponents 33 and 34 can place part of the
                // mantissa outside the 256-bit buffer. Those bytes must
                // be zero or the target would overflow.
                if index < 0 {
                    if byte != 0 {
                        return None;
                    }

                    continue;
                }

                if index >= POW_HASH_SIZE as isize {
                    continue;
                }

                bytes[index as usize] = byte;
            }
        }

        Self::from_bytes(bytes)
    }

    /// Encodes this target into Bitcoin-style compact representation.
    ///
    /// Compact encoding preserves only the most significant 23 bits, so
    /// arbitrary 256-bit targets may lose low-order precision.
    pub fn to_compact(self) -> u32 {
        let Some(first_nonzero) = self.0.iter().position(|byte| *byte != 0) else {
            // This should be unreachable for normally constructed targets.
            return 0;
        };

        let mut size = (POW_HASH_SIZE - first_nonzero) as u32;

        let mut mantissa = if size <= 3 {
            let mut value = 0_u32;

            for byte in &self.0[first_nonzero..] {
                value = (value << 8) | u32::from(*byte);
            }

            value << (8 * (3 - size))
        } else {
            let first = u32::from(self.0[first_nonzero]);

            let second = self
                .0
                .get(first_nonzero + 1)
                .copied()
                .map(u32::from)
                .unwrap_or(0);

            let third = self
                .0
                .get(first_nonzero + 2)
                .copied()
                .map(u32::from)
                .unwrap_or(0);

            (first << 16) | (second << 8) | third
        };

        // Bit 23 is reserved as the compact sign bit.
        //
        // If the mantissa would set it, shift the mantissa down by one
        // byte and increase the exponent.
        if mantissa & COMPACT_SIGN_MASK != 0 {
            mantissa >>= 8;
            size += 1;
        }

        (size << 24) | (mantissa & COMPACT_MANTISSA_MASK)
    }
}

/// Returns true when `hash` satisfies `target`.
#[inline]
pub fn hash_meets_target(hash: &PoWHash, target: PoWTarget) -> bool {
    target.meets(hash)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bitcoin_genesis_compact_target_roundtrip() {
        let bits = 0x1d00_ffff;

        let target = PoWTarget::from_compact(bits).expect("valid compact target");

        assert_eq!(target.to_compact(), bits);

        assert_eq!(
            target.as_bytes(),
            &[
                0x00, 0x00, 0x00, 0x00, 0xff, 0xff, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
                0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
                0x00, 0x00, 0x00, 0x00,
            ]
        );
    }

    #[test]
    fn novus_pow_limit_roundtrip() {
        let bits = 0x207f_ffff;

        let target = PoWTarget::from_compact(bits).expect("valid Novus PoW limit");

        assert_eq!(target.to_compact(), bits);
    }

    #[test]
    fn rejects_negative_compact_target() {
        assert!(PoWTarget::from_compact(0x1d80_ffff).is_none());
    }

    #[test]
    fn rejects_zero_compact_target() {
        assert!(PoWTarget::from_compact(0).is_none());
    }

    #[test]
    fn rejects_zero_raw_target() {
        assert!(PoWTarget::from_bytes([0_u8; POW_HASH_SIZE]).is_none());
    }

    #[test]
    fn accepts_smallest_nonzero_raw_target() {
        let mut bytes = [0_u8; POW_HASH_SIZE];
        bytes[POW_HASH_SIZE - 1] = 1;

        assert!(PoWTarget::from_bytes(bytes).is_some());
    }

    #[test]
    fn target_scaling_harder_reduces_target() {
        let target = PoWTarget::from_compact(0x207f_ffff).expect("valid target");

        let harder = target
            .scale_ratio(95, 100)
            .expect("target scaling succeeds");

        assert!(harder < target);
    }

    #[test]
    fn target_scaling_easier_increases_target() {
        let target = PoWTarget::from_compact(0x2007_ffff).expect("valid target");

        let easier = target
            .scale_ratio(105, 100)
            .expect("target scaling succeeds");

        assert!(easier > target);
    }

    #[test]
    fn target_scaling_identity_preserves_target() {
        let target = PoWTarget::from_compact(0x2007_ffff).expect("valid target");

        let same = target
            .scale_ratio(100, 100)
            .expect("target scaling succeeds");

        assert_eq!(same, target);
    }

    #[test]
    fn scale_ratio_supports_timestamp_retarget_values() {
        let target = PoWTarget::from_compact(0x207f_ffff).expect("valid target");

        assert_eq!(target.scale_ratio(3600, 3600), Some(target));

        assert!(target.scale_ratio(1800, 3600).expect("harder target") < target);

        assert!(target.scale_ratio(7200, 3600).expect("easier target") > target);
    }

    #[test]
    fn compact_roundtrip_is_canonical() {
        let values = [0x1d00_ffff, 0x207f_ffff, 0x2007_ffff, 0x1f12_3456];

        for bits in values {
            let target = PoWTarget::from_compact(bits).expect("valid compact target");

            assert_eq!(target.to_compact(), bits);
        }
    }

    #[test]
    fn smallest_target_survives_scaling() {
        let mut bytes = [0_u8; POW_HASH_SIZE];
        bytes[POW_HASH_SIZE - 1] = 1;

        let target = PoWTarget::from_bytes(bytes).expect("valid minimum target");

        let scaled = target.scale_ratio(1, 2).expect("scaling succeeds");

        assert_eq!(scaled, target);
    }
}
