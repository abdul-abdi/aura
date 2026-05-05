import Darwin

/// Monotonic nanosecond timestamp since boot epoch.
///
/// Uses `mach_continuous_time` rather than `mach_absolute_time` because
/// the continuous variant keeps ticking during system sleep, which gives
/// correct ordering for log entries written across a sleep/wake cycle.
public enum MonotonicClock {
    public static func now() -> UInt64 {
        var info = mach_timebase_info_data_t()
        mach_timebase_info(&info)
        let ticks = mach_continuous_time()
        // Multiply first to reduce precision loss; both are UInt32 so
        // this is safe for practical tick counts.
        return ticks &* UInt64(info.numer) / UInt64(info.denom)
    }
}
