//! Small public types of the pairing API: what each device learns about the other one.

use zeroize::Zeroize;

use crate::device::DevicePublic;

/// The verification code (SAS) of a pairing session: 6 decimal digits derived from the session
/// key (`pairing-v1.md` §5.2, §5.3). It is shown on both devices and compared by eye.
///
/// It is never typed, sent or logged: `Debug` hides it and there is no `Display`.
#[derive(Clone, PartialEq, Eq)]
pub struct Sas(u32);

impl Sas {
    /// Number of digits.
    pub const DIGITS: usize = 6;
    /// Modulus of the reduction `u64be(s8) mod 10^6`.
    pub(super) const MODULUS: u64 = 1_000_000;

    pub(super) fn new(value: u32) -> Self {
        Self(value)
    }

    /// The value, `0..1_000_000`.
    pub fn value(&self) -> u32 {
        self.0
    }

    /// The six digits, leading zeros included: `"048291"`.
    pub fn digits(&self) -> String {
        format!("{:06}", self.0)
    }

    /// The digits grouped by three, as shown to the user: `"482 913"`.
    pub fn grouped(&self) -> String {
        let d = self.digits();
        format!("{} {}", &d[..3], &d[3..])
    }
}

impl Drop for Sas {
    fn drop(&mut self) {
        self.0.zeroize();
    }
}

impl std::fmt::Debug for Sas {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Sas(<redacted>)")
    }
}

/// What the inviter can transfer to a new device (estimate, `pairing-v1.md` §9 step 1).
/// Display only: nothing else may rely on these numbers.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HistoryOffer {
    /// Exportable items (readable, outside the outbox). `0` means "nothing to offer".
    pub items: u64,
    /// Approximate size in bytes (payloads and files present locally).
    pub bytes: u64,
}

/// A history transfer announced to the new device (`pairing-v1.md` §9 step 3).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HistoryAnnouncement {
    /// UUID of the transfer that follows: the inviter exports with exactly this id.
    pub transfer_id: String,
    /// Items, from the inviter's estimate (display only).
    pub items: u64,
    /// Bytes, from the inviter's estimate (display only).
    pub bytes: u64,
}

/// A device, as announced by itself over the authenticated pairing channel.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PeerInfo {
    /// Device name (1 to 64 characters).
    pub name: String,
    /// Platform (`^[a-z0-9-]{1,32}$`).
    pub platform: String,
    /// Its public keys, authentic once the PAKE has succeeded.
    pub public: DevicePublic,
}

/// What the new device learns about the inviter (`InviterHello`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InviterInfo {
    pub peer: PeerInfo,
    /// Members of the space, the inviter included ("Your space has 3 devices").
    pub member_count: Option<u16>,
    /// What the inviter offers to transfer, if anything.
    pub history_offer: Option<HistoryOffer>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sas_formats_with_leading_zeros_and_hides_itself() {
        let s = Sas::new(482_913);
        assert_eq!(s.digits(), "482913");
        assert_eq!(s.grouped(), "482 913");
        let z = Sas::new(48);
        assert_eq!(z.digits(), "000048");
        assert_eq!(z.grouped(), "000 048");
        assert_eq!(format!("{s:?}"), "Sas(<redacted>)");
        assert_eq!(s.value(), 482_913);
    }
}
