pub mod generic;
pub mod login;

pub use generic::GenericRateLimiter;
pub use login::{LoginKeys, LoginRateLimiter};

use std::collections::HashMap;

/// Evict the least recently active keys so `map` stays bounded.
///
/// The limiters call this after every recorded attempt, so the common path must
/// stay O(1): the `collect` + `sort` only runs once the map has grown past `max`,
/// and it then trims to 90% of capacity, spreading the cost over the next tenth of
/// inserts. Sorting the whole map under the mutex on *every* insert let a flood of
/// distinct keys turn the limiter into a CPU amplifier.
pub(crate) fn evict_oldest(map: &mut HashMap<String, Vec<i64>>, max: usize) {
    if map.len() <= max {
        return;
    }
    let target = max - max / 10;
    let remove = map.len() - target;
    let mut by_age: Vec<(i64, String)> = map
        .iter()
        .map(|(key, timestamps)| {
            (
                timestamps.iter().copied().max().unwrap_or(i64::MIN),
                key.clone(),
            )
        })
        .collect();
    by_age.sort_unstable();
    for (_, key) in by_age.into_iter().take(remove) {
        map.remove(&key);
    }
}

#[cfg(test)]
mod tests {
    use super::evict_oldest;
    use std::collections::HashMap;

    fn entry(timestamp: i64) -> Vec<i64> {
        vec![timestamp]
    }

    #[test]
    fn eviction_trims_below_the_limit_so_it_is_amortized() {
        let max = 100;
        let mut map: HashMap<String, Vec<i64>> = (0..=max)
            .map(|i| (format!("k{i}"), entry(i as i64)))
            .collect();
        evict_oldest(&mut map, max);
        assert_eq!(
            map.len(),
            max - max / 10,
            "one batch trim keeps the next tenth of inserts cheap"
        );
        assert!(!map.contains_key("k0"), "the oldest key goes first");
        assert!(map.contains_key(&format!("k{max}")), "the newest key stays");

        // A map at exactly the limit is left alone.
        let mut at_limit: HashMap<String, Vec<i64>> = (0..max)
            .map(|i| (format!("k{i}"), entry(i as i64)))
            .collect();
        evict_oldest(&mut at_limit, max);
        assert_eq!(at_limit.len(), max);
    }
}
