//! application heartbeat와 peer lease. PROTOCOL.md §6.
//!
//! - 각 방향 250ms마다 Ping, 최대 4개 outstanding.
//! - lease는 Ready 시점에 시작하고, 마지막 유효 Pong 수신부터 1000ms.
//! - Pong은 발행 후 1000ms 이내여야 유효하다. 일반 input은 lease를 갱신하지 않는다.
//! - 상대는 Ping을 앞선 명령 처리 후 순서대로 응답하므로 Pong은 발행 순서(FIFO)로 와야 한다.

use std::collections::VecDeque;
use std::time::Duration;

use edgebridge_protocol::{Ping, Pong};

use crate::Millis;

pub const PING_INTERVAL: Duration = Duration::from_millis(250);
pub const LEASE: Duration = Duration::from_millis(1000);
pub const PONG_DEADLINE: Duration = Duration::from_millis(1000);
pub const MAX_OUTSTANDING: usize = 4;

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum HeartbeatError {
    #[error("peer lease expired")]
    Expired,
    #[error("pong token {0} does not match the oldest outstanding ping")]
    UnexpectedPong(u64),
    #[error("ping token {got} is not greater than previous {previous}")]
    NonIncreasingPing { previous: u64, got: u64 },
}

/// 내가 보낸 Ping과 상대 Pong으로 상대의 진행을 감시한다.
#[derive(Debug)]
pub struct Heartbeat {
    next_token: u64,
    outstanding: VecDeque<(u64, Millis)>,
    last_ping_at: Option<Millis>,
    lease_deadline: Millis,
}

impl Heartbeat {
    /// Ready 완료 시각에 만든다.
    pub fn start(now: Millis) -> Self {
        Self {
            next_token: 0,
            outstanding: VecDeque::with_capacity(MAX_OUTSTANDING),
            last_ping_at: None,
            lease_deadline: now + LEASE,
        }
    }

    pub fn lease_deadline(&self) -> Millis {
        self.lease_deadline
    }

    pub fn is_expired(&self, now: Millis) -> bool {
        now >= self.lease_deadline
    }

    /// 주기적으로 호출한다. lease가 만료되면 오류, Ping을 보낼 때가 되었으면 `Some`.
    pub fn poll(&mut self, now: Millis) -> Result<Option<Ping>, HeartbeatError> {
        if self.is_expired(now) {
            return Err(HeartbeatError::Expired);
        }
        let due = self
            .last_ping_at
            .is_none_or(|last| now.saturating_since(last) >= PING_INTERVAL);
        if !due || self.outstanding.len() >= MAX_OUTSTANDING {
            return Ok(None);
        }
        let token = self.next_token;
        self.next_token += 1;
        self.outstanding.push_back((token, now));
        self.last_ping_at = Some(now);
        Ok(Some(Ping { token }))
    }

    pub fn on_pong(&mut self, pong: Pong, now: Millis) -> Result<(), HeartbeatError> {
        if self.is_expired(now) {
            return Err(HeartbeatError::Expired);
        }
        match self.outstanding.front() {
            Some(&(token, issued)) if token == pong.token => {
                self.outstanding.pop_front();
                if now.saturating_since(issued) > PONG_DEADLINE {
                    return Err(HeartbeatError::Expired);
                }
                self.lease_deadline = now + LEASE;
                Ok(())
            }
            _ => Err(HeartbeatError::UnexpectedPong(pong.token)),
        }
    }
}

/// 상대 Ping에 대한 응답 측. token이 방향별로 증가하는지만 확인한다.
///
/// Pong은 반드시 앞선 명령을 처리한 actor/control thread에서 만들어야 한다 (network reader 금지).
#[derive(Debug, Default)]
pub struct PingResponder {
    last: Option<u64>,
}

impl PingResponder {
    pub fn on_ping(&mut self, ping: Ping) -> Result<Pong, HeartbeatError> {
        if let Some(previous) = self.last
            && ping.token <= previous
        {
            return Err(HeartbeatError::NonIncreasingPing {
                previous,
                got: ping.token,
            });
        }
        self.last = Some(ping.token);
        Ok(Pong { token: ping.token })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ms(v: u64) -> Millis {
        Millis(v)
    }

    #[test]
    fn issues_ping_every_250ms() {
        let mut hb = Heartbeat::start(ms(0));
        assert_eq!(hb.poll(ms(0)), Ok(Some(Ping { token: 0 })));
        assert_eq!(hb.poll(ms(249)), Ok(None));
        assert_eq!(hb.poll(ms(250)), Ok(Some(Ping { token: 1 })));
    }

    #[test]
    fn lease_expires_exactly_at_1000ms_without_pong() {
        let mut hb = Heartbeat::start(ms(0));
        assert!(hb.poll(ms(999)).is_ok());
        assert_eq!(hb.poll(ms(1000)), Err(HeartbeatError::Expired));
    }

    #[test]
    fn valid_pong_extends_lease_from_receipt_time() {
        let mut hb = Heartbeat::start(ms(0));
        hb.poll(ms(0)).unwrap();
        hb.on_pong(Pong { token: 0 }, ms(600)).unwrap();
        assert_eq!(hb.lease_deadline(), ms(1600));
        assert!(!hb.is_expired(ms(1599)));
        assert!(hb.is_expired(ms(1600)));
    }

    #[test]
    fn caps_outstanding_pings_at_four() {
        let mut hb = Heartbeat::start(ms(0));
        for (i, t) in [0, 250, 500, 750].into_iter().enumerate() {
            assert_eq!(hb.poll(ms(t)), Ok(Some(Ping { token: i as u64 })));
        }
        // 5번째는 outstanding이 가득 차서 보내지 않는다. lease는 아직 유효.
        assert_eq!(hb.poll(ms(999)), Ok(None));
    }

    #[test]
    fn rejects_unknown_duplicate_and_out_of_order_pong() {
        let mut hb = Heartbeat::start(ms(0));
        hb.poll(ms(0)).unwrap();
        hb.poll(ms(250)).unwrap();
        assert_eq!(
            hb.on_pong(Pong { token: 1 }, ms(300)),
            Err(HeartbeatError::UnexpectedPong(1))
        );

        let mut hb = Heartbeat::start(ms(0));
        hb.poll(ms(0)).unwrap();
        hb.on_pong(Pong { token: 0 }, ms(10)).unwrap();
        assert_eq!(
            hb.on_pong(Pong { token: 0 }, ms(20)),
            Err(HeartbeatError::UnexpectedPong(0))
        );
        assert_eq!(
            hb.on_pong(Pong { token: 99 }, ms(20)),
            Err(HeartbeatError::UnexpectedPong(99))
        );
    }

    #[test]
    fn late_pong_does_not_renew_lease() {
        let mut hb = Heartbeat::start(ms(0));
        hb.poll(ms(0)).unwrap();
        hb.poll(ms(250)).unwrap();
        hb.on_pong(Pong { token: 0 }, ms(200)).unwrap(); // lease -> 1200
        // token 1은 250ms에 발행. 1251ms 도착은 lease(1200)도 지났고 deadline도 넘었다.
        assert_eq!(
            hb.on_pong(Pong { token: 1 }, ms(1251)),
            Err(HeartbeatError::Expired)
        );
    }

    #[test]
    fn responder_requires_increasing_tokens() {
        let mut r = PingResponder::default();
        assert_eq!(r.on_ping(Ping { token: 0 }), Ok(Pong { token: 0 }));
        assert_eq!(r.on_ping(Ping { token: 1 }), Ok(Pong { token: 1 }));
        assert_eq!(
            r.on_ping(Ping { token: 1 }),
            Err(HeartbeatError::NonIncreasingPing {
                previous: 1,
                got: 1
            })
        );
    }
}
