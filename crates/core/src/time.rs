//! monotonic 시각. wall-clock이 아니며 기기 간에 비교하지 않는다.

use std::ops::Add;
use std::time::Duration;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub struct Millis(pub u64);

impl Millis {
    pub const fn saturating_since(self, earlier: Millis) -> Duration {
        Duration::from_millis(self.0.saturating_sub(earlier.0))
    }
}

impl Add<Duration> for Millis {
    type Output = Millis;

    fn add(self, rhs: Duration) -> Millis {
        let ms = u64::try_from(rhs.as_millis()).unwrap_or(u64::MAX);
        Millis(self.0.saturating_add(ms))
    }
}
