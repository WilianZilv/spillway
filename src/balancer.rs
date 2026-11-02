//! Destination selection: smooth weighted round-robin (nginx style) where each
//! destination's weight is its free space times an operator multiplier.
//!
//! Compared to the old claim/quota scheme this is per-file and work-conserving:
//! a destination whose transfer slots are all busy is skipped for that pick, so
//! a fast destination naturally absorbs more when a slow one is saturated, while
//! over time the split converges to the free-space proportions.

use std::collections::HashMap;

use crate::config::GIB;

#[derive(Debug, Clone)]
pub struct Candidate {
    pub id: String,
    pub free_bytes: u64,
    pub weight_mult: f64,
    pub inflight: u32,
    pub max_inflight: u32,
}

impl Candidate {
    pub fn weight(&self) -> f64 {
        // Never exactly zero: an eligible destination with tiny free space still gets a turn.
        (self.free_bytes as f64 / GIB).max(0.001) * self.weight_mult
    }
    fn has_slot(&self) -> bool {
        self.inflight < self.max_inflight
    }
}

#[derive(Default)]
pub struct Swrr {
    current: HashMap<String, f64>,
}

impl Swrr {
    /// Picks the index of the next destination, or None when nothing should be
    /// dispatched right now.
    ///
    /// * strict (`work_conserving == false`): the weighted turn is taken among all
    ///   candidates; if it belongs to a saturated destination we wait for its slot.
    ///   This keeps the split exactly proportional to free space.
    /// * work-conserving: saturated destinations are skipped, so every free slot
    ///   is used (used under RAM pressure, when draining fast matters most).
    ///
    /// `avoid` is skipped unless it is the only option.
    pub fn pick(&mut self, cands: &[Candidate], avoid: Option<&str>, work_conserving: bool) -> Option<usize> {
        let eligible: Vec<usize> = if work_conserving {
            (0..cands.len()).filter(|&i| cands[i].has_slot()).collect()
        } else {
            (0..cands.len()).collect()
        };
        if eligible.is_empty() {
            return None;
        }
        let pool: Vec<usize> = match avoid {
            Some(a) if eligible.iter().any(|&i| cands[i].id != a && (work_conserving || cands[i].has_slot())) => {
                eligible.into_iter().filter(|&i| cands[i].id != a).collect()
            }
            _ => eligible,
        };
        self.current.retain(|k, _| cands.iter().any(|c| &c.id == k));

        let total: f64 = pool.iter().map(|&i| cands[i].weight()).sum();
        let mut best: Option<(usize, f64)> = None;
        for &i in &pool {
            let c = self.current.get(&cands[i].id).copied().unwrap_or(0.0) + cands[i].weight();
            if best.map_or(true, |(_, v)| c > v) {
                best = Some((i, c));
            }
        }
        let (idx, _) = best?;
        if !cands[idx].has_slot() {
            // strict mode: it's this destination's turn but it is busy — wait.
            return None;
        }
        for &i in &pool {
            *self.current.entry(cands[i].id.clone()).or_insert(0.0) += cands[i].weight();
        }
        *self.current.get_mut(&cands[idx].id).unwrap() -= total;
        Some(idx)
    }
}

/// Expected share of new files per destination (for display), ignoring momentary saturation.
pub fn shares(cands: &[Candidate]) -> Vec<f64> {
    let total: f64 = cands.iter().map(|c| c.weight()).sum();
    cands.iter().map(|c| if total > 0.0 { c.weight() / total } else { 0.0 }).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cand(id: &str, free_gb: f64, inflight: u32) -> Candidate {
        Candidate { id: id.into(), free_bytes: (free_gb * GIB) as u64, weight_mult: 1.0, inflight, max_inflight: 4 }
    }

    #[test]
    fn split_follows_free_space() {
        let cands = vec![cand("a", 300.0, 0), cand("b", 100.0, 0)];
        let mut s = Swrr::default();
        let mut n = [0; 2];
        for _ in 0..4000 {
            n[s.pick(&cands, None, false).unwrap()] += 1;
        }
        assert_eq!(n, [3000, 1000]);
    }

    #[test]
    fn smooth_not_bursty() {
        let cands = vec![cand("a", 200.0, 0), cand("b", 100.0, 0)];
        let mut s = Swrr::default();
        let seq: Vec<usize> = (0..6).map(|_| s.pick(&cands, None, false).unwrap()).collect();
        // never more than two consecutive picks of the heavier one
        assert_eq!(seq, vec![0, 1, 0, 0, 1, 0]);
    }

    #[test]
    fn saturated_destination_is_skipped_when_work_conserving() {
        let cands = vec![cand("a", 1000.0, 4), cand("b", 1.0, 0)];
        let mut s = Swrr::default();
        for _ in 0..10 {
            assert_eq!(s.pick(&cands, None, true), Some(1));
        }
        let all_busy = vec![cand("a", 1.0, 4), cand("b", 1.0, 4)];
        assert_eq!(s.pick(&all_busy, None, true), None);
    }

    #[test]
    fn strict_mode_waits_for_the_weighted_turn() {
        let mut cands = vec![cand("a", 300.0, 4), cand("b", 100.0, 0)];
        let mut s = Swrr::default();
        // a's turn but a is busy: nothing is dispatched, state is not consumed
        assert_eq!(s.pick(&cands, None, false), None);
        assert_eq!(s.pick(&cands, None, false), None);
        cands[0].inflight = 0;
        let mut n = [0; 2];
        for _ in 0..400 {
            n[s.pick(&cands, None, false).unwrap()] += 1;
        }
        assert_eq!(n, [300, 100]);
    }

    #[test]
    fn avoid_unless_only_option() {
        let cands = vec![cand("a", 1000.0, 0), cand("b", 1.0, 0)];
        let mut s = Swrr::default();
        for _ in 0..10 {
            assert_eq!(s.pick(&cands, Some("a"), false), Some(1));
        }
        let only = vec![cand("a", 10.0, 0)];
        assert_eq!(s.pick(&only, Some("a"), false), Some(0));
    }

    #[test]
    fn weight_multiplier_and_shares() {
        let mut cands = vec![cand("a", 100.0, 0), cand("b", 100.0, 0)];
        cands[1].weight_mult = 3.0;
        let sh = shares(&cands);
        assert!((sh[0] - 0.25).abs() < 1e-9 && (sh[1] - 0.75).abs() < 1e-9);
        let mut s = Swrr::default();
        let mut n = [0; 2];
        for _ in 0..400 {
            n[s.pick(&cands, None, false).unwrap()] += 1;
        }
        assert_eq!(n, [100, 300]);
    }

    #[test]
    fn removed_candidates_forget_state() {
        let mut s = Swrr::default();
        s.pick(&[cand("a", 1.0, 0), cand("b", 1.0, 0)], None, false);
        s.pick(&[cand("b", 1.0, 0)], None, false);
        assert!(!s.current.contains_key("a"));
    }
}
