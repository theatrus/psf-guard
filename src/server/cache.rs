use anyhow::Result;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

pub struct CacheManager {
    cache_dir: PathBuf,
}

impl CacheManager {
    pub fn new(cache_dir: PathBuf) -> Self {
        Self { cache_dir }
    }

    pub fn get_cached_path(&self, category: &str, key: &str, extension: &str) -> PathBuf {
        let category_dir = self.cache_dir.join(category);
        let filename = format!("{}.{}", key, extension);
        category_dir.join(filename)
    }

    pub fn is_cached(&self, path: &Path) -> bool {
        path.exists()
    }

    pub fn get_cache_age(&self, path: &Path) -> Result<u64> {
        let metadata = std::fs::metadata(path)?;
        let modified = metadata.modified()?;
        let duration = SystemTime::now().duration_since(modified)?;
        Ok(duration.as_secs())
    }

    pub fn ensure_category_dir(&self, category: &str) -> Result<PathBuf> {
        let category_dir = self.cache_dir.join(category);
        std::fs::create_dir_all(&category_dir)?;
        Ok(category_dir)
    }

    pub fn cleanup_old_files(&self, category: &str, max_age_seconds: u64) -> Result<usize> {
        let category_dir = self.cache_dir.join(category);
        if !category_dir.exists() {
            return Ok(0);
        }

        let mut removed_count = 0;
        for entry in std::fs::read_dir(&category_dir)? {
            let entry = entry?;
            let path = entry.path();

            if let Ok(age) = self.get_cache_age(&path) {
                if age > max_age_seconds {
                    std::fs::remove_file(&path)?;
                    removed_count += 1;
                }
            }
        }

        Ok(removed_count)
    }

    pub fn get_total_size(&self) -> Result<u64> {
        let mut total_size = 0;
        self.walk_cache_dir(&self.cache_dir, &mut |entry| {
            if let Ok(metadata) = entry.metadata() {
                total_size += metadata.len();
            }
        })?;
        Ok(total_size)
    }

    #[allow(clippy::only_used_in_recursion)]
    fn walk_cache_dir<F>(&self, dir: &Path, callback: &mut F) -> Result<()>
    where
        F: FnMut(&std::fs::DirEntry),
    {
        if dir.is_dir() {
            for entry in std::fs::read_dir(dir)? {
                let entry = entry?;
                let path = entry.path();
                if path.is_dir() {
                    self.walk_cache_dir(&path, callback)?;
                } else {
                    callback(&entry);
                }
            }
        }
        Ok(())
    }
}
