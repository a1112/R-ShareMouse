use rshare_core::perf::RollingLatencyHistogram;

#[global_allocator]
static ALLOC: dhat::Alloc = dhat::Alloc;

fn main() {
    let mut histogram = RollingLatencyHistogram::new(1_000_000).unwrap();
    for value in 1..=10_000 {
        histogram.record(value);
    }

    let profiler = dhat::Profiler::builder().testing().build();
    let mut measured_calls = 0_u64;
    for value in 1..=100_000 {
        histogram.record(value);
        measured_calls += 1;
    }
    let stats = dhat::HeapStats::get();
    assert_eq!(stats.total_blocks, 0, "recording allocated after warmup");
    drop(profiler);

    println!(
        "measured record calls: {measured_calls}; allocated blocks: {}",
        stats.total_blocks
    );
    assert_eq!(
        measured_calls, 100_000,
        "allocation gate must measure exactly 100,000 record calls"
    );
}
