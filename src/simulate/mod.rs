//! Network-condition simulation, applied per request in this order:
//! error switch → rate limit → latency → body. Abort/hang must preempt
//! everything; a throttled request shouldn't pay the latency cost.

pub mod error_mode;
pub mod latency;
pub mod rate_limit;
