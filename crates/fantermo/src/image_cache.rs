//! 端末の中の画像のテクスチャを覚えておく入れ物。テクスチャの作り方は知らない（`renderer`）。

use std::collections::HashMap;

use fantermo_core::frame::ImageKey;

/// 画像の入れ物。端末（`owner`）ごとに、画像の鍵（[`ImageKey`]）で値を覚える。合計のバイト数が上限を
/// 超えたら、いま描いていないものを古いものから捨てる。
pub struct ImageCache<T> {
    entries: HashMap<ImageKey, Entry<T>>,
    /// `entries` の `bytes` の合計。
    bytes: u64,
    limit: u64,
    /// 描いているフレームの番号。
    frame: u64,
}

struct Entry<T> {
    owner: usize,
    value: T,
    bytes: u64,
    /// 最後に使ったフレームの番号。
    last_used: u64,
}

impl<T> ImageCache<T> {
    pub fn new(limit: u64) -> Self {
        Self {
            entries: HashMap::new(),
            bytes: 0,
            limit,
            frame: 0,
        }
    }

    /// `key` があれば、このフレームで使うものとして印を付けて `true` を返す。
    pub fn touch(&mut self, key: &ImageKey) -> bool {
        let Some(entry) = self.entries.get_mut(key) else {
            return false;
        };
        entry.last_used = self.frame;
        true
    }

    pub fn get(&self, key: &ImageKey) -> Option<&T> {
        self.entries.get(key).map(|entry| &entry.value)
    }

    /// `owner` の端末の `key` の値を、このフレームで使うものとして覚える。
    pub fn insert(&mut self, owner: usize, key: ImageKey, value: T, bytes: u64) {
        self.bytes += bytes;
        let entry = Entry {
            owner,
            value,
            bytes,
            last_used: self.frame,
        };
        if let Some(old) = self.entries.insert(key, entry) {
            self.bytes -= old.bytes;
        }
    }

    /// `owner` の端末のもののうち、`keep` が偽のものを捨てる。捨てた数を返す。
    pub fn retain_owner(&mut self, owner: usize, mut keep: impl FnMut(&ImageKey) -> bool) -> usize {
        self.remove_where(|key, entry| entry.owner == owner && !keep(key))
    }

    /// `owner` の端末のものをすべて捨てる。捨てた数を返す。
    pub fn remove_owner(&mut self, owner: usize) -> usize {
        self.remove_where(|_, entry| entry.owner == owner)
    }

    /// フレームの区切り。上限を超えていれば、このフレームで使っていないものを古いものから捨てる。捨てた
    /// 数を返す。このフレームで使うものだけで上限を超えているときは、それらは残す。
    pub fn finish_frame(&mut self) -> usize {
        let mut unused: Vec<(u64, ImageKey)> = self
            .entries
            .iter()
            .filter(|(_, entry)| entry.last_used != self.frame)
            .map(|(key, entry)| (entry.last_used, *key))
            .collect();
        unused.sort_unstable_by_key(|(last_used, _)| *last_used);
        let mut evicted = 0;
        for (_, key) in unused {
            if self.bytes <= self.limit {
                break;
            }
            if let Some(entry) = self.entries.remove(&key) {
                self.bytes -= entry.bytes;
                evicted += 1;
            }
        }
        self.frame += 1;
        evicted
    }

    fn remove_where(&mut self, mut remove: impl FnMut(&ImageKey, &Entry<T>) -> bool) -> usize {
        let before = self.entries.len();
        let mut bytes = self.bytes;
        self.entries.retain(|key, entry| {
            let removed = remove(key, entry);
            if removed {
                bytes -= entry.bytes;
            }
            !removed
        });
        self.bytes = bytes;
        before - self.entries.len()
    }
}

#[cfg(test)]
mod tests {
    use fantermo_core::frame::ImageSource;

    use super::*;

    fn key(id: u32) -> ImageKey {
        ImageKey {
            source: ImageSource::Kitty,
            id,
            generation: 1,
        }
    }

    #[test]
    fn values_are_found_by_their_key() {
        let mut cache = ImageCache::new(100);
        cache.insert(0, key(1), "a", 10);
        assert!(cache.touch(&key(1)));
        assert_eq!(cache.get(&key(1)), Some(&"a"));
        assert!(!cache.touch(&key(2)));
        assert_eq!(cache.get(&key(2)), None);
        // 同じ番号でも世代が違えば別のもの。
        let newer = ImageKey {
            generation: 2,
            ..key(1)
        };
        assert_eq!(cache.get(&newer), None);
    }

    #[test]
    fn over_the_limit_the_oldest_unused_values_go_first() {
        let mut cache = ImageCache::new(25);
        cache.insert(0, key(1), (), 10);
        assert_eq!(cache.finish_frame(), 0);
        cache.insert(0, key(2), (), 10);
        assert_eq!(cache.finish_frame(), 0);
        cache.insert(0, key(3), (), 10);
        assert_eq!(cache.finish_frame(), 1);
        assert_eq!(cache.get(&key(1)), None);
        assert!(cache.get(&key(2)).is_some());
        assert!(cache.get(&key(3)).is_some());
    }

    #[test]
    fn values_used_in_this_frame_stay_over_the_limit() {
        let mut cache = ImageCache::new(5);
        cache.insert(0, key(1), (), 10);
        assert_eq!(cache.finish_frame(), 0);
        assert!(cache.touch(&key(1)));
        cache.insert(0, key(2), (), 10);
        assert_eq!(cache.finish_frame(), 0);
        // 次のフレームで使わなければ捨てる。使っているものが上限を超えていても、残りは残す。
        assert!(cache.touch(&key(2)));
        assert_eq!(cache.finish_frame(), 1);
        assert_eq!(cache.get(&key(1)), None);
        assert!(cache.get(&key(2)).is_some());
    }

    #[test]
    fn removing_gives_the_bytes_back() {
        let mut cache = ImageCache::new(25);
        cache.insert(0, key(1), (), 10);
        cache.insert(1, key(2), (), 10);
        cache.insert(1, key(3), (), 10);
        cache.insert(1, key(4), (), 10);
        assert_eq!(cache.retain_owner(1, |key| key.id != 2), 1);
        assert!(cache.get(&key(2)).is_none());
        assert!(cache.get(&key(1)).is_some());
        assert_eq!(cache.remove_owner(1), 2);
        assert!(cache.get(&key(1)).is_some());
        // 上限まで空いた。
        cache.insert(0, key(5), (), 15);
        assert_eq!(cache.finish_frame(), 0);
    }

    #[test]
    fn inserting_again_replaces_the_bytes() {
        let mut cache = ImageCache::new(15);
        cache.insert(0, key(1), (), 10);
        cache.insert(0, key(1), (), 10);
        assert_eq!(cache.finish_frame(), 0);
        assert!(cache.get(&key(1)).is_some());
    }
}
