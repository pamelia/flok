use std::time::Duration;

use anyhow::Result;
use flok_tui::{
    test_support::{run_app_perf_script, run_streaming_perf_script, PerfScriptEvent},
    UiEvent,
};

#[tokio::test]
async fn burst_of_100_deltas_renders_at_most_three_times() -> Result<()> {
    let script =
        (0..100).map(|_| (Duration::ZERO, UiEvent::TextDelta("abcdefghij".to_string()))).collect();

    let stats = run_streaming_perf_script(80, 24, script).await?;
    assert!(stats.render_calls <= 3, "render_calls={}", stats.render_calls);
    Ok(())
}

#[tokio::test]
async fn active_item_cache_hit_rate_stays_above_eighty_percent() -> Result<()> {
    let mut script = Vec::new();
    for _ in 0..100 {
        script.push((Duration::ZERO, PerfScriptEvent::Ui(UiEvent::TextDelta("abcdefghij".into()))));
        for _ in 0..5 {
            script.push((Duration::ZERO, PerfScriptEvent::Resize(80, 24)));
        }
    }

    let stats = run_app_perf_script(80, 24, script).await?;
    let total = stats.cache_hits + stats.cache_misses;
    assert!(total > 0, "cache stats should record active renders");
    let hit_rate = stats.cache_hits as f64 / total as f64;
    assert!(
        hit_rate > 0.80,
        "hit_rate={hit_rate:.2}, hits={}, misses={}",
        stats.cache_hits,
        stats.cache_misses
    );
    Ok(())
}
