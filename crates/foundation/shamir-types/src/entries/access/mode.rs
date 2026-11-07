/// Permission class for mode-bit queries.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PermClass {
    Owner,
    Group,
    Other,
}

/// Permission bit (read / write / execute).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Perm {
    Read,
    Write,
    Execute,
}

/// POSIX-style 12-bit mode helpers (`rwxrwxrwx` + setuid/setgid/sticky).
///
/// Bit layout (same as Unix `mode_t`, low 12 bits):
///
/// ```text
/// 11 10  9  8  7  6  5  4  3  2  1  0
/// s  s  t  rwx rwx rwx
/// │  │  │  │   │   └── other: r(2) w(1) x(0)
/// │  │  │  │   └────── group: r(5) w(4) x(3)
/// │  │  │  └────────── owner:  r(8) w(7) x(6)
/// │  │  └──────────────── sticky
/// │  └─────────────────── setgid
/// └────────────────────── setuid (0o4000)
/// ```
pub struct Mode;

/// setuid bit (bit 11, `0o4000`).
pub const MODE_SETUID: u16 = 0o4000;

impl Mode {
    /// Bit positions for r/w/x within a 3-bit class slot.
    const R: u16 = 0o4;
    const W: u16 = 0o2;
    const X: u16 = 0o1;

    /// Full rwx for one class.
    pub const RWX: u16 = Self::R | Self::W | Self::X;

    /// Open mode: owner/group/other all rwx (`0o777`).
    pub const OPEN: u16 = 0o777;

    /// Shift amount for each permission class.
    fn shift(class: PermClass) -> u16 {
        match class {
            PermClass::Other => 0,
            PermClass::Group => 3,
            PermClass::Owner => 6,
        }
    }

    /// Build a combined mode from per-class rwx flags.
    pub fn from_rwx(owner: bool, group: bool, other: bool) -> u16 {
        let mut mode: u16 = 0;
        if owner {
            mode |= Self::RWX << Self::shift(PermClass::Owner);
        }
        if group {
            mode |= Self::RWX << Self::shift(PermClass::Group);
        }
        if other {
            mode |= Self::RWX << Self::shift(PermClass::Other);
        }
        mode
    }

    /// Check whether a specific permission bit is set for a class.
    pub fn is_set(mode: u16, class: PermClass, perm: Perm) -> bool {
        let bit = match perm {
            Perm::Read => Self::R,
            Perm::Write => Self::W,
            Perm::Execute => Self::X,
        };
        (mode >> Self::shift(class)) & bit == bit
    }

    /// Check the setuid flag (bit 11).
    pub fn is_setuid(mode: u16) -> bool {
        mode & MODE_SETUID != 0
    }

    /// Set or clear the setuid flag.
    pub fn with_setuid(mode: u16, set: bool) -> u16 {
        if set {
            mode | MODE_SETUID
        } else {
            mode & !MODE_SETUID
        }
    }
}
