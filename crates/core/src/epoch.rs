//! Enter마다 증가하는 epoch. 1부터 시작하고 session 안에서 재사용하지 않는다.

#[derive(Debug)]
pub struct EpochCounter {
    next: u64,
}

impl Default for EpochCounter {
    fn default() -> Self {
        Self { next: 1 }
    }
}

impl EpochCounter {
    pub fn new() -> Self {
        Self::default()
    }

    /// 새 epoch를 발급한다. u64 공간이 끝나면 `None` — 연결을 새로 만들어야 한다.
    pub fn issue(&mut self) -> Option<u64> {
        let epoch = self.next;
        self.next = self.next.checked_add(1)?;
        Some(epoch)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn starts_at_one_and_never_repeats() {
        let mut c = EpochCounter::new();
        assert_eq!(c.issue(), Some(1));
        assert_eq!(c.issue(), Some(2));
        assert_eq!(c.issue(), Some(3));
    }
}
