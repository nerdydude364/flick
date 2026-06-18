use rand::RngExt;

/// In-place Fisher-Yates shuffle, same algorithm as `reshuffle`/`reshuffleImages`
/// in the Electron app's renderer/app.js.
pub fn fisher_yates(order: &mut [usize]) {
    let mut rng = rand::rng();
    for i in (1..order.len()).rev() {
        let j = rng.random_range(0..=i);
        order.swap(i, j);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shuffle_preserves_all_elements() {
        let mut order: Vec<usize> = (0..20).collect();
        fisher_yates(&mut order);
        let mut sorted = order.clone();
        sorted.sort();
        assert_eq!(sorted, (0..20).collect::<Vec<_>>());
    }
}
