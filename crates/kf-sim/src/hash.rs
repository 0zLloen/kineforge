//! Per-tick state hash (ARCHITECTURE.md §4, EXPERIMENT_PLAN E10): FNV-1a
//! 64 over a fixed little-endian serialisation. The writer has only
//! fixed-width methods — no `usize` — so a collection length cannot hash as
//! 4 bytes on wasm32 and 8 elsewhere; lengths are written as `u64`,
//! `Option`s as a `u8` tag (0 = None, 1 = Some) followed by the value,
//! `f64` as its bits.

/// FNV-1a 64 state.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Fnv(pub u64);

impl Fnv {
    /// FNV-1a 64 offset basis.
    pub const BASIS: u64 = 0xcbf2_9ce4_8422_2325;
    const PRIME: u64 = 0x0000_0100_0000_01b3;

    pub fn new() -> Fnv {
        Fnv(Fnv::BASIS)
    }

    pub fn bytes(&mut self, b: &[u8]) {
        for &x in b {
            self.0 = (self.0 ^ u64::from(x)).wrapping_mul(Fnv::PRIME);
        }
    }

    pub fn u8(&mut self, v: u8) {
        self.bytes(&[v]);
    }

    pub fn u16(&mut self, v: u16) {
        self.bytes(&v.to_le_bytes());
    }

    pub fn u32(&mut self, v: u32) {
        self.bytes(&v.to_le_bytes());
    }

    pub fn u64(&mut self, v: u64) {
        self.bytes(&v.to_le_bytes());
    }

    pub fn i64(&mut self, v: i64) {
        self.bytes(&v.to_le_bytes());
    }

    pub fn f64(&mut self, v: f64) {
        self.u64(v.to_bits());
    }

    pub fn bool(&mut self, v: bool) {
        self.u8(u8::from(v));
    }

    pub fn opt_u64(&mut self, v: Option<u64>) {
        match v {
            None => self.u8(0),
            Some(x) => {
                self.u8(1);
                self.u64(x);
            }
        }
    }

    pub fn opt_i64(&mut self, v: Option<i64>) {
        match v {
            None => self.u8(0),
            Some(x) => {
                self.u8(1);
                self.i64(x);
            }
        }
    }
}

impl Default for Fnv {
    fn default() -> Fnv {
        Fnv::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Published FNV-1a 64 test vectors (Fowler/Noll/Vo reference suite).
    #[test]
    fn reference_vectors() {
        let h = |s: &[u8]| {
            let mut f = Fnv::new();
            f.bytes(s);
            f.0
        };
        assert_eq!(h(b""), 0xcbf29ce484222325);
        assert_eq!(h(b"a"), 0xaf63dc4c8601ec8c);
        assert_eq!(h(b"foobar"), 0x85944171f73967e8);
    }

    #[test]
    fn widths_are_fixed() {
        let mut a = Fnv::new();
        a.u64(3);
        let mut b = Fnv::new();
        b.bytes(&[3, 0, 0, 0, 0, 0, 0, 0]);
        assert_eq!(a, b);
        let (mut n, mut s) = (Fnv::new(), Fnv::new());
        n.opt_u64(None);
        s.opt_u64(Some(0));
        assert_ne!(n, s);
    }
}
