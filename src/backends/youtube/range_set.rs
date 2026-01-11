/// Tracks downloaded byte ranges for a streaming audio file.
/// Ranges are stored as sorted, non-overlapping (start, end) tuples.
/// Adjacent ranges are automatically merged.
#[derive(Debug, Clone, Default)]
pub struct RangeSet {
    ranges: Vec<(u64, u64)>,
}

impl RangeSet {
    /// Create empty `RangeSet`
    #[must_use]
    pub fn new() -> Self {
        Self { ranges: Vec::new() }
    }

    /// Add a range [start, end). Merges overlapping/adjacent ranges.
    ///
    /// # Panics
    ///
    /// Never panics. The unwrap is safe because `last_merge_idx` is always
    /// `Some` when `first_merge_idx` is `Some`.
    pub fn add_range(&mut self, start: u64, end: u64) {
        if start >= end {
            return;
        }

        let mut merge_start = start;
        let mut merge_end = end;

        // Find all ranges that overlap or are adjacent to [start, end)
        // Two ranges [a, b) and [c, d) overlap or are adjacent if:
        // - They overlap: a < d && c < b
        // - They are adjacent: b == c or d == a
        // Combined condition: a <= d && c <= b
        let mut first_merge_idx = None;
        let mut last_merge_idx = None;

        for (i, &(range_start, range_end)) in self.ranges.iter().enumerate() {
            if range_start <= merge_end && range_end >= merge_start {
                merge_start = merge_start.min(range_start);
                merge_end = merge_end.max(range_end);

                if first_merge_idx.is_none() {
                    first_merge_idx = Some(i);
                }
                last_merge_idx = Some(i);
            }
        }

        if let Some(first) = first_merge_idx {
            let last = last_merge_idx.unwrap();
            self.ranges.drain(first..=last);
            self.ranges.insert(first, (merge_start, merge_end));
        } else {
            let insert_pos =
                self.ranges.iter().position(|&(s, _)| s > merge_start).unwrap_or(self.ranges.len());
            self.ranges.insert(insert_pos, (merge_start, merge_end));
        }
    }

    /// Check if a specific byte offset is downloaded
    #[must_use]
    pub fn contains(&self, offset: u64) -> bool {
        // Use binary search for efficiency
        self.ranges
            .binary_search_by(|&(start, end)| {
                if offset < start {
                    std::cmp::Ordering::Greater
                } else if offset >= end {
                    std::cmp::Ordering::Less
                } else {
                    std::cmp::Ordering::Equal
                }
            })
            .is_ok()
    }

    /// Get the end of contiguous downloaded bytes starting from offset.
    /// Returns offset if that byte isn't downloaded.
    /// Returns the end of the contiguous range if downloaded.
    #[must_use]
    pub fn contiguous_from(&self, offset: u64) -> u64 {
        for &(start, end) in &self.ranges {
            if offset >= start && offset < end {
                return end;
            }
        }
        offset
    }

    /// Total bytes downloaded (sum of all ranges)
    #[must_use]
    pub fn total_bytes(&self) -> u64 {
        self.ranges.iter().map(|&(start, end)| end - start).sum()
    }

    /// Check if completely downloaded (single range covering
    /// `0..content_length`)
    #[must_use]
    pub fn is_complete(&self, content_length: u64) -> bool {
        self.ranges.len() == 1 && self.ranges[0] == (0, content_length)
    }

    /// Get the first gap after offset.
    /// Returns (gap_start, gap_end) or None if no gap exists.
    #[must_use]
    pub fn first_gap_after(&self, offset: u64, file_size: u64) -> Option<(u64, u64)> {
        let contiguous_end = self.contiguous_from(offset);

        if contiguous_end == offset {
            for &(start, _) in &self.ranges {
                if start > offset {
                    return Some((offset, start));
                }
            }
            return Some((offset, file_size));
        }

        for &(start, _) in &self.ranges {
            if start > contiguous_end {
                return Some((contiguous_end, start));
            }
        }

        if contiguous_end < file_size {
            return Some((contiguous_end, file_size));
        }

        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_empty() {
        let rs = RangeSet::new();
        assert!(!rs.contains(0));
        assert_eq!(rs.contiguous_from(0), 0);
        assert_eq!(rs.total_bytes(), 0);
    }

    #[test]
    fn test_single_range() {
        let mut rs = RangeSet::new();
        rs.add_range(0, 100);
        assert!(rs.contains(0));
        assert!(rs.contains(99));
        assert!(!rs.contains(100));
        assert_eq!(rs.contiguous_from(0), 100);
        assert_eq!(rs.total_bytes(), 100);
    }

    #[test]
    fn test_merge_overlapping() {
        let mut rs = RangeSet::new();
        rs.add_range(0, 50);
        rs.add_range(30, 100); // Overlaps
        assert_eq!(rs.ranges.len(), 1);
        assert_eq!(rs.ranges[0], (0, 100));
    }

    #[test]
    fn test_merge_adjacent() {
        let mut rs = RangeSet::new();
        rs.add_range(0, 50);
        rs.add_range(50, 100); // Adjacent
        assert_eq!(rs.ranges.len(), 1);
        assert_eq!(rs.ranges[0], (0, 100));
    }

    #[test]
    fn test_gap() {
        let mut rs = RangeSet::new();
        rs.add_range(0, 50);
        rs.add_range(100, 150); // Gap at 50-100
        assert_eq!(rs.ranges.len(), 2);
        assert!(rs.contains(49));
        assert!(!rs.contains(50));
        assert!(!rs.contains(99));
        assert!(rs.contains(100));
        assert_eq!(rs.contiguous_from(0), 50);
        assert_eq!(rs.contiguous_from(100), 150);
    }

    #[test]
    fn test_fill_gap() {
        let mut rs = RangeSet::new();
        rs.add_range(0, 50);
        rs.add_range(100, 150);
        rs.add_range(50, 100); // Fill the gap
        assert_eq!(rs.ranges.len(), 1);
        assert_eq!(rs.ranges[0], (0, 150));
    }

    #[test]
    fn test_is_complete() {
        let mut rs = RangeSet::new();
        rs.add_range(0, 1000);
        assert!(rs.is_complete(1000));
        assert!(!rs.is_complete(1001));
    }

    #[test]
    fn test_out_of_order_inserts() {
        let mut rs = RangeSet::new();
        rs.add_range(100, 150);
        rs.add_range(0, 50);
        rs.add_range(50, 100);
        assert_eq!(rs.ranges.len(), 1);
        assert_eq!(rs.ranges[0], (0, 150));
    }

    #[test]
    fn test_multiple_overlaps() {
        let mut rs = RangeSet::new();
        rs.add_range(0, 30);
        rs.add_range(50, 80);
        rs.add_range(100, 130);
        // Add range that overlaps all three
        rs.add_range(20, 110);
        assert_eq!(rs.ranges.len(), 1);
        assert_eq!(rs.ranges[0], (0, 130));
    }

    #[test]
    fn test_invalid_range() {
        let mut rs = RangeSet::new();
        rs.add_range(100, 50); // Invalid: start >= end
        assert_eq!(rs.ranges.len(), 0);
        rs.add_range(50, 50); // Invalid: start == end
        assert_eq!(rs.ranges.len(), 0);
    }

    #[test]
    fn test_total_bytes_multiple_ranges() {
        let mut rs = RangeSet::new();
        rs.add_range(0, 100); // 100 bytes
        rs.add_range(200, 350); // 150 bytes
        rs.add_range(500, 600); // 100 bytes
        assert_eq!(rs.total_bytes(), 350);
    }

    #[test]
    fn test_contiguous_from_gap() {
        let mut rs = RangeSet::new();
        rs.add_range(0, 50);
        rs.add_range(100, 150);
        // Query at gap should return offset itself
        assert_eq!(rs.contiguous_from(75), 75);
    }

    #[test]
    fn test_contains_boundary() {
        let mut rs = RangeSet::new();
        rs.add_range(50, 100);
        assert!(!rs.contains(49)); // Just before start
        assert!(rs.contains(50)); // At start
        assert!(rs.contains(99)); // Just before end
        assert!(!rs.contains(100)); // At end (exclusive)
        assert!(!rs.contains(101)); // Just after end
    }

    #[test]
    fn test_merge_three_adjacent() {
        let mut rs = RangeSet::new();
        rs.add_range(0, 50);
        rs.add_range(100, 150);
        rs.add_range(200, 250);
        // Fill first gap
        rs.add_range(50, 100);
        assert_eq!(rs.ranges.len(), 2);
        assert_eq!(rs.ranges[0], (0, 150));
        assert_eq!(rs.ranges[1], (200, 250));
        // Fill second gap
        rs.add_range(150, 200);
        assert_eq!(rs.ranges.len(), 1);
        assert_eq!(rs.ranges[0], (0, 250));
    }

    #[test]
    fn test_insert_before_all() {
        let mut rs = RangeSet::new();
        rs.add_range(100, 150);
        rs.add_range(200, 250);
        rs.add_range(0, 50); // Insert before all existing ranges
        assert_eq!(rs.ranges.len(), 3);
        assert_eq!(rs.ranges[0], (0, 50));
        assert_eq!(rs.ranges[1], (100, 150));
        assert_eq!(rs.ranges[2], (200, 250));
    }

    #[test]
    fn test_insert_after_all() {
        let mut rs = RangeSet::new();
        rs.add_range(0, 50);
        rs.add_range(100, 150);
        rs.add_range(300, 350); // Insert after all existing ranges
        assert_eq!(rs.ranges.len(), 3);
        assert_eq!(rs.ranges[2], (300, 350));
    }

    #[test]
    fn test_duplicate_range() {
        let mut rs = RangeSet::new();
        rs.add_range(50, 100);
        rs.add_range(50, 100); // Exact duplicate
        assert_eq!(rs.ranges.len(), 1);
        assert_eq!(rs.ranges[0], (50, 100));
    }

    #[test]
    fn test_subset_range() {
        let mut rs = RangeSet::new();
        rs.add_range(0, 100);
        rs.add_range(25, 75); // Subset of existing range
        assert_eq!(rs.ranges.len(), 1);
        assert_eq!(rs.ranges[0], (0, 100));
    }

    #[test]
    fn test_superset_range() {
        let mut rs = RangeSet::new();
        rs.add_range(25, 75);
        rs.add_range(0, 100); // Superset of existing range
        assert_eq!(rs.ranges.len(), 1);
        assert_eq!(rs.ranges[0], (0, 100));
    }
}
