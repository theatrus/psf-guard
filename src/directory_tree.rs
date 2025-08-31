use anyhow::Result;
use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

/// Represents a cached directory tree with file lookups
#[derive(Debug, Clone)]
pub struct DirectoryTree {
    /// Map from filename to all possible full paths
    file_map: HashMap<String, Vec<PathBuf>>,
    /// Map from directory path to its contents (for faster directory-specific lookups)
    dir_map: HashMap<PathBuf, Vec<PathBuf>>,
    /// When this tree was built
    created_at: SystemTime,
    /// Root directory that was scanned
    root: PathBuf,
}

impl DirectoryTree {
    /// Build a complete directory tree in memory from the given root
    pub fn build(root: &Path) -> Result<Self> {
        tracing::info!("🌳 Building directory tree cache for: {:?}", root);
        let start_time = std::time::Instant::now();
        
        let mut file_map: HashMap<String, Vec<PathBuf>> = HashMap::new();
        let mut dir_map: HashMap<PathBuf, Vec<PathBuf>> = HashMap::new();
        let mut total_files = 0;
        let mut total_dirs = 0;
        
        Self::scan_directory(root, &mut file_map, &mut dir_map, &mut total_files, &mut total_dirs)?;
        
        let elapsed = start_time.elapsed();
        tracing::info!(
            "🌳 Directory tree built in {:.2}s: {} files, {} directories", 
            elapsed.as_secs_f64(),
            total_files, 
            total_dirs
        );
        
        Ok(DirectoryTree {
            file_map,
            dir_map,
            created_at: SystemTime::now(),
            root: root.to_path_buf(),
        })
    }
    
    /// Recursively scan a directory and populate the maps
    fn scan_directory(
        dir: &Path, 
        file_map: &mut HashMap<String, Vec<PathBuf>>,
        dir_map: &mut HashMap<PathBuf, Vec<PathBuf>>,
        total_files: &mut usize,
        total_dirs: &mut usize,
    ) -> Result<()> {
        // Skip certain directories to avoid unwanted areas
        if let Some(dir_name) = dir.file_name() {
            let name = dir_name.to_string_lossy();
            if matches!(name.as_ref(), "DARK" | "FLAT" | "BIAS" | ".git" | "node_modules" | "target" | ".cache") {
                tracing::trace!("⏭️  Skipping directory: {:?}", dir);
                return Ok(());
            }
        }
        
        let entries = match fs::read_dir(dir) {
            Ok(entries) => entries,
            Err(e) => {
                tracing::trace!("⚠️  Cannot read directory {:?}: {}", dir, e);
                return Ok(()); // Continue with other directories
            }
        };
        
        let mut dir_contents = Vec::new();
        *total_dirs += 1;
        
        for entry in entries {
            let entry = match entry {
                Ok(entry) => entry,
                Err(e) => {
                    tracing::trace!("⚠️  Error reading entry in {:?}: {}", dir, e);
                    continue;
                }
            };
            
            let path = entry.path();
            dir_contents.push(path.clone());
            
            if path.is_dir() {
                // Recurse into subdirectories
                Self::scan_directory(&path, file_map, dir_map, total_files, total_dirs)?;
            } else {
                // Add file to filename map
                if let Some(filename) = path.file_name().and_then(|n| n.to_str()) {
                    file_map.entry(filename.to_string())
                        .or_insert_with(Vec::new)
                        .push(path.clone());
                    *total_files += 1;
                }
            }
        }
        
        dir_map.insert(dir.to_path_buf(), dir_contents);
        Ok(())
    }
    
    /// Find all paths for a given filename
    pub fn find_file(&self, filename: &str) -> Option<&Vec<PathBuf>> {
        self.file_map.get(filename)
    }
    
    /// Find files matching a pattern in the filename
    pub fn find_files_matching<F>(&self, predicate: F) -> Vec<&PathBuf>
    where
        F: Fn(&str) -> bool,
    {
        self.file_map
            .iter()
            .filter_map(|(filename, paths)| {
                if predicate(filename) {
                    Some(paths.iter())
                } else {
                    None
                }
            })
            .flatten()
            .collect()
    }
    
    /// Get all FITS files in the tree
    pub fn get_fits_files(&self) -> Vec<&PathBuf> {
        self.find_files_matching(|filename| {
            filename.ends_with(".fits") 
                || filename.ends_with(".fit") 
                || filename.ends_with(".FIT") 
                || filename.ends_with(".FITS")
                || filename.ends_with(".fts")
        })
    }
    
    /// Get contents of a specific directory
    pub fn get_directory_contents(&self, dir: &Path) -> Option<&Vec<PathBuf>> {
        self.dir_map.get(dir)
    }
    
    /// Get all filenames in the tree (for debugging/stats)
    pub fn get_all_filenames(&self) -> Vec<&String> {
        self.file_map.keys().collect()
    }
    
    /// Get statistics about the cached tree
    pub fn stats(&self) -> DirectoryTreeStats {
        DirectoryTreeStats {
            total_files: self.file_map.values().map(|v| v.len()).sum(),
            unique_filenames: self.file_map.len(),
            total_directories: self.dir_map.len(),
            age: self.created_at.elapsed().unwrap_or(Duration::from_secs(0)),
            root: self.root.clone(),
        }
    }
    
    /// Check if the cache is older than the given duration
    pub fn is_older_than(&self, max_age: Duration) -> bool {
        self.created_at.elapsed().unwrap_or(Duration::from_secs(0)) > max_age
    }
}

/// Statistics about a directory tree cache
#[derive(Debug)]
pub struct DirectoryTreeStats {
    pub total_files: usize,
    pub unique_filenames: usize,
    pub total_directories: usize,
    pub age: Duration,
    pub root: PathBuf,
}

impl DirectoryTreeStats {
    pub fn format_age(&self) -> String {
        let secs = self.age.as_secs();
        if secs < 60 {
            format!("{}s", secs)
        } else if secs < 3600 {
            format!("{}m{}s", secs / 60, secs % 60)
        } else {
            format!("{}h{}m", secs / 3600, (secs % 3600) / 60)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;
    
    #[test]
    fn test_directory_tree_basic() -> Result<()> {
        // Create a temporary directory structure for testing
        let temp_dir = TempDir::new()?;
        let root = temp_dir.path();
        
        // Create some test files and directories
        fs::create_dir_all(root.join("subdir1"))?;
        fs::create_dir_all(root.join("subdir2/nested"))?;
        fs::write(root.join("file1.fits"), "test")?;
        fs::write(root.join("subdir1/file2.fit"), "test")?;
        fs::write(root.join("subdir2/nested/file3.txt"), "test")?;
        
        // Build the tree
        let tree = DirectoryTree::build(root)?;
        
        // Test file finding
        assert!(tree.find_file("file1.fits").is_some());
        assert!(tree.find_file("file2.fit").is_some());
        assert!(tree.find_file("file3.txt").is_some());
        assert!(tree.find_file("nonexistent.fits").is_none());
        
        // Test FITS file finding
        let fits_files = tree.get_fits_files();
        assert_eq!(fits_files.len(), 2); // file1.fits and file2.fit
        
        // Test stats
        let stats = tree.stats();
        assert_eq!(stats.total_files, 3);
        assert_eq!(stats.unique_filenames, 3);
        
        Ok(())
    }
    
    #[test]
    fn test_directory_tree_skipped_dirs() -> Result<()> {
        let temp_dir = TempDir::new()?;
        let root = temp_dir.path();
        
        // Create directories that should be skipped
        fs::create_dir_all(root.join("DARK"))?;
        fs::create_dir_all(root.join(".git"))?;
        fs::create_dir_all(root.join("valid"))?;
        
        // Create files in each
        fs::write(root.join("DARK/dark1.fits"), "test")?;
        fs::write(root.join(".git/config"), "test")?;
        fs::write(root.join("valid/good.fits"), "test")?;
        
        let tree = DirectoryTree::build(root)?;
        
        // Should only find the file in the valid directory
        assert!(tree.find_file("good.fits").is_some());
        assert!(tree.find_file("dark1.fits").is_none());
        assert!(tree.find_file("config").is_none());
        
        Ok(())
    }
}