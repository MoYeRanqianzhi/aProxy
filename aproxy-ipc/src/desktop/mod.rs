//! Application interface of the bundled desktop window. Versioned with that window,
//! not a stable third-party protocol. Wire DTOs, local records and error envelopes
//! may gain fields; projection at this boundary must not make those fields public.

mod codes;
mod frames;
pub use frames::*;
mod history;
pub use history::*;
mod ops;
pub use codes::*;
pub use ops::*;
use serde::{Deserialize, Serialize};

/// Unsigned bridge quantity. Saturate Rust outputs at the last exactly representable
/// JavaScript integer; reject, rather than round, numbers supplied by the page.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Ord, PartialOrd, Serialize, Deserialize)]
#[serde(try_from = "u64")]
pub struct SafeUint(u64);

impl SafeUint {
    /// Largest integer for which adding one can always be distinguished in JavaScript.
    pub const MAX: u64 = (1 << 53) - 1;

    /// Project a count, sequence, offset or elapsed duration from the instance.
    pub fn saturating(n: u64) -> Self {
        Self(n.min(Self::MAX))
    }

    /// Native value for range checks and bounded reads after decoding.
    pub fn get(self) -> u64 {
        self.0
    }
}

impl TryFrom<u64> for SafeUint {
    type Error = &'static str;
    fn try_from(n: u64) -> Result<Self, Self::Error> {
        if n <= Self::MAX {
            Ok(Self(n))
        } else {
            Err("not a safe integer")
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn counters_are_numbers_but_inputs_never_saturate() {
        assert_eq!(
            serde_json::to_value(SafeUint::saturating(u64::MAX)).unwrap(),
            SafeUint::MAX
        );
        for text in ["9007199254740992", "-1", "1.1", "\"17\"", "null"] {
            assert!(serde_json::from_str::<SafeUint>(text).is_err());
        }
        assert_eq!(serde_json::from_str::<SafeUint>("17").unwrap().get(), 17);
    }
}
