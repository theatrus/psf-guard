#!/usr/bin/env python3

import os
import sys
import json
import subprocess
import shutil
from pathlib import Path
from collections import defaultdict
import re

class DylibProcessor:
    def __init__(self, binary_path, tauri_config, frameworks_dir, homebrew_prefix="/opt/homebrew"):
        self.binary_path = Path(binary_path)
        self.tauri_config = Path(tauri_config)
        self.frameworks_dir = Path(frameworks_dir)
        self.homebrew_prefix = Path(homebrew_prefix)
        
        # Performance optimizations
        self.visited = set()
        self.processing_stack = set()
        self.homebrew_index = {}  # lib_name -> full_path
        self.otool_cache = {}     # path -> otool results
        
    def build_homebrew_index(self):
        """Build a fast lookup index of all homebrew dylibs"""
        print("📋 Building homebrew library index...")
        
        try:
            # Find all .dylib files (both regular files and symlinks) in homebrew
            result = subprocess.run([
                'find', str(self.homebrew_prefix), '-name', '*.dylib', '(', '-type', 'f', '-o', '-type', 'l', ')'
            ], capture_output=True, text=True, timeout=30)
            
            if result.returncode == 0:
                for path in result.stdout.strip().split('\n'):
                    if path:  # Skip empty lines
                        lib_name = Path(path).name
                        # If multiple versions exist, prefer:
                        # 1. Symlinks in /opt/homebrew/lib (most accessible)
                        # 2. Files with /current/ in path 
                        # 3. Any other file
                        if (lib_name not in self.homebrew_index or 
                            '/opt/homebrew/lib/' in path or
                            '/current/' in path):
                            # Resolve symlinks to get the actual file path
                            try:
                                resolved_path = str(Path(path).resolve())
                                if Path(resolved_path).exists():
                                    self.homebrew_index[lib_name] = resolved_path
                                else:
                                    self.homebrew_index[lib_name] = path
                            except:
                                self.homebrew_index[lib_name] = path
                            
                print(f"✅ Indexed {len(self.homebrew_index)} homebrew dylibs")
            else:
                print(f"⚠️  Warning: Could not index homebrew libraries: {result.stderr}")
        except subprocess.TimeoutExpired:
            print("⚠️  Warning: Homebrew indexing timed out, falling back to slower method")
    
    def get_dependencies(self, binary_path):
        """Get dependencies using cached otool results"""
        path_str = str(binary_path)
        
        if path_str in self.otool_cache:
            return self.otool_cache[path_str]
        
        try:
            result = subprocess.run(['otool', '-L', path_str], 
                                  capture_output=True, text=True, timeout=10)
            
            if result.returncode != 0:
                self.otool_cache[path_str] = ([], [])
                return ([], [])
            
            lines = result.stdout.strip().split('\n')[1:]  # Skip first line (self-reference)
            
            homebrew_deps = []
            rpath_deps = []
            
            for line in lines:
                line = line.strip()
                if not line:
                    continue
                    
                # Extract the path (first part before compatibility info)
                dep_path = line.split(' (compatibility')[0].strip()
                
                if dep_path.startswith(str(self.homebrew_prefix)):
                    homebrew_deps.append(dep_path)
                elif dep_path.startswith('@rpath/') and dep_path.endswith('.dylib'):
                    rpath_deps.append(dep_path)
            
            self.otool_cache[path_str] = (homebrew_deps, rpath_deps)
            return (homebrew_deps, rpath_deps)
            
        except (subprocess.TimeoutExpired, subprocess.CalledProcessError) as e:
            print(f"⚠️  Error analyzing {path_str}: {e}")
            self.otool_cache[path_str] = ([], [])
            return ([], [])
    
    def resolve_rpath(self, rpath_dep):
        """Resolve @rpath dependency to actual homebrew path"""
        lib_name = Path(rpath_dep).name
        return self.homebrew_index.get(lib_name)
    
    def find_dependencies_recursive(self, binary_path, depth=0, max_depth=20):
        """Recursively find all homebrew dependencies with cycle detection"""
        
        if depth > max_depth:
            print(f"   ⚠️  Maximum recursion depth reached for: {binary_path}")
            return
        
        path_str = str(binary_path)
        
        # Cycle detection
        if path_str in self.processing_stack:
            if depth < 3:
                print(f"   🔄 Cycle detected, skipping: {Path(path_str).name}")
            return
        
        # Skip if already visited
        if path_str in self.visited:
            return
        
        # Mark as visited and add to processing stack
        self.visited.add(path_str)
        self.processing_stack.add(path_str)
        
        if depth < 3:
            indent = "  " * depth
            print(f"{indent}🔍 Processing: {Path(binary_path).name} (depth: {depth})")
        
        try:
            # Get dependencies
            homebrew_deps, rpath_deps = self.get_dependencies(binary_path)
            
            # Process direct homebrew dependencies
            for dep in homebrew_deps:
                if Path(dep).exists():
                    if depth < 3:
                        indent = "  " * depth
                        print(f"{indent}  📦 Found: {Path(dep).name}")
                    self.find_dependencies_recursive(dep, depth + 1, max_depth)
                else:
                    if depth < 3:
                        indent = "  " * depth
                        print(f"{indent}  ⚠️  Missing: {dep}")
            
            # Process @rpath dependencies
            for rpath_dep in rpath_deps:
                resolved_path = self.resolve_rpath(rpath_dep)
                if resolved_path and Path(resolved_path).exists():
                    if depth < 3:
                        indent = "  " * depth
                        print(f"{indent}  📦 Resolved @rpath: {Path(rpath_dep).name} -> {Path(resolved_path).name}")
                    self.find_dependencies_recursive(resolved_path, depth + 1, max_depth)
                else:
                    if depth < 3:
                        indent = "  " * depth
                        print(f"{indent}  ⚠️  Could not resolve @rpath: {rpath_dep}")
        
        finally:
            # Remove from processing stack when done
            self.processing_stack.discard(path_str)
    
    def copy_dylibs(self):
        """Copy all discovered dylibs to frameworks directory"""
        # Filter out non-dylib files and the main binary
        dylib_paths = [p for p in self.visited 
                      if Path(p).suffix == '.dylib' and 
                      Path(p).name != self.binary_path.name and
                      Path(p).exists()]
        
        # Remove duplicates based on filename (keep first occurrence)
        unique_dylibs = {}
        for dylib_path in dylib_paths:
            dylib_name = Path(dylib_path).name
            if dylib_name not in unique_dylibs:
                unique_dylibs[dylib_name] = dylib_path
        
        print(f"📥 Copying {len(unique_dylibs)} unique dylibs to {self.frameworks_dir}...")
        
        # Create frameworks directory
        self.frameworks_dir.mkdir(exist_ok=True)
        
        local_dylibs = []
        for dylib_name, dylib_path in unique_dylibs.items():
            local_path = self.frameworks_dir / dylib_name
            
            try:
                # Remove existing file if it exists to avoid permission issues
                if local_path.exists():
                    local_path.chmod(0o755)  # Make writable
                    local_path.unlink()      # Remove existing file
                
                print(f"   Copying: {dylib_name}")
                shutil.copy2(dylib_path, local_path)
                
                # Make the copied file writable for future install_name_tool operations
                local_path.chmod(0o755)
                
                local_dylibs.append(str(local_path.absolute()))
                
            except (OSError, PermissionError) as e:
                print(f"   ⚠️  Failed to copy {dylib_name}: {e}")
                continue
        
        print("✅ Copied all dylibs to local frameworks directory")
        return local_dylibs
    
    def rewrite_dependencies(self, local_dylibs):
        """Rewrite all dependency references to use relative paths"""
        print("🔧 Rewriting internal dylib references...")
        
        # Rewrite main binary
        print(f"   📝 Rewriting main binary: {self.binary_path.name}")
        for dylib_path in self.visited:
            if Path(dylib_path).exists():
                dylib_name = Path(dylib_path).name
                new_ref = f"@executable_path/../Frameworks/{dylib_name}"
                subprocess.run(['install_name_tool', '-change', dylib_path, new_ref, str(self.binary_path)], 
                             stderr=subprocess.DEVNULL)
        
        # Also handle @rpath references in main binary
        homebrew_deps, rpath_deps = self.get_dependencies(self.binary_path)
        for rpath_dep in rpath_deps:
            dylib_name = Path(rpath_dep).name
            if (self.frameworks_dir / dylib_name).exists():
                new_ref = f"@executable_path/../Frameworks/{dylib_name}"
                subprocess.run(['install_name_tool', '-change', rpath_dep, new_ref, str(self.binary_path)], 
                             stderr=subprocess.DEVNULL)
        
        # Rewrite each copied dylib
        for local_dylib in local_dylibs:
            local_path = Path(local_dylib)
            dylib_name = local_path.name
            print(f"   📝 Rewriting dylib: {dylib_name}")
            
            # Update the dylib's own ID
            subprocess.run(['install_name_tool', '-id', f"@loader_path/{dylib_name}", str(local_path)], 
                         stderr=subprocess.DEVNULL)
            
            # Update references to other homebrew dylibs
            homebrew_deps, rpath_deps = self.get_dependencies(local_path)
            
            for dep in homebrew_deps:
                dep_name = Path(dep).name
                if dep_name != dylib_name and (self.frameworks_dir / dep_name).exists():
                    subprocess.run(['install_name_tool', '-change', dep, f"@loader_path/{dep_name}", str(local_path)], 
                                 stderr=subprocess.DEVNULL)
            
            for rpath_dep in rpath_deps:
                dep_name = Path(rpath_dep).name
                if dep_name != dylib_name and (self.frameworks_dir / dep_name).exists():
                    subprocess.run(['install_name_tool', '-change', rpath_dep, f"@loader_path/{dep_name}", str(local_path)], 
                                 stderr=subprocess.DEVNULL)
        
        print("✅ Rewritten all dylib references")
    
    def update_tauri_config(self, local_dylibs):
        """Update Tauri configuration with local dylib paths"""
        print("📝 Updating Tauri configuration...")
        
        try:
            with open(self.tauri_config, 'r') as f:
                config = json.load(f)
            
            # Ensure the structure exists
            if 'bundle' not in config:
                config['bundle'] = {}
            if 'macOS' not in config['bundle']:
                config['bundle']['macOS'] = {}
            
            # Update frameworks array
            config['bundle']['macOS']['frameworks'] = local_dylibs
            
            with open(self.tauri_config, 'w') as f:
                json.dump(config, f, indent=2)
            
            print(f"✅ Updated frameworks in {self.tauri_config} with {len(local_dylibs)} local dylibs")
            
        except Exception as e:
            print(f"❌ Error updating Tauri config: {e}")
            return False
        
        return True
    
    def process(self):
        """Main processing function"""
        print("🔍 Detecting and processing all homebrew dylib dependencies for macOS packaging...")
        
        # Validate inputs
        if not self.binary_path.exists():
            print(f"❌ Binary not found at {self.binary_path}")
            print("   Make sure to build the release binary first with: cargo build --release --features tauri")
            return False
        
        if not self.tauri_config.exists():
            print(f"❌ Tauri macOS config not found at {self.tauri_config}")
            return False
        
        # Build homebrew index for fast lookups
        self.build_homebrew_index()
        
        # Find all dependencies
        print("📋 Recursively analyzing all homebrew dependencies...")
        self.find_dependencies_recursive(self.binary_path)
        
        if not self.visited:
            print("⚠️  No homebrew dylibs found in binary")
            print("   This is normal if OpenCV is statically linked or not used")
            return True
        
        print(f"✅ Found {len(self.visited)} unique homebrew dylibs")
        
        # Copy dylibs locally
        local_dylibs = self.copy_dylibs()
        
        # Rewrite dependency references
        self.rewrite_dependencies(local_dylibs)
        
        # Update Tauri config
        success = self.update_tauri_config(local_dylibs)
        
        if success:
            print("")
            print("🎉 OpenCV dylib processing complete!")
            print("📊 Summary:")
            print(f"   - Found {len(self.visited)} homebrew dependencies")
            print(f"   - Copied to: {self.frameworks_dir}/")
            print(f"   - Updated: {self.tauri_config}")
            print("   - Rewritten all internal references to use relative paths")
        
        return success

def main():
    # Configuration
    binary_path = "target/release/psf-guard"
    tauri_config = "tauri.macos.conf.json"
    frameworks_dir = "Frameworks"
    
    processor = DylibProcessor(binary_path, tauri_config, frameworks_dir)
    success = processor.process()
    
    sys.exit(0 if success else 1)

if __name__ == "__main__":
    main()