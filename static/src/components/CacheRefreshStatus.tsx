import { useState, useEffect } from 'react';
import { useQuery, useQueryClient } from '@tanstack/react-query';
import './CacheRefreshStatus.css';

interface CacheRefreshProgress {
  is_refreshing: boolean;
  stage: string;
  progress_percentage: number;
  elapsed_seconds: number | null;
  directories_total: number;
  directories_processed: number;
  current_directory_name: string | null;
  files_scanned: number;
  projects_total: number;
  projects_processed: number;
  current_project_name: string | null;
  targets_total: number;
  targets_processed: number;
  files_found: number;
  files_missing: number;
}

interface CacheRefreshStatusProps {
  className?: string;
}

const STAGE_LABELS: Record<string, string> = {
  idle: 'Idle',
  initializing_directory_tree: 'Scanning directories',
  loading_projects: 'Loading projects',
  processing_projects: 'Processing projects',
  processing_targets: 'Processing targets',
  updating_cache: 'Updating cache',
  completed: 'Completed'
};

async function fetchCacheProgress(): Promise<CacheRefreshProgress> {
  const response = await fetch('/api/cache-progress');
  if (!response.ok) {
    throw new Error('Failed to fetch cache progress');
  }
  const result = await response.json();
  return result.data;
}

export default function CacheRefreshStatus({ className = '' }: CacheRefreshStatusProps) {
  const queryClient = useQueryClient();
  const [isVisible, setIsVisible] = useState(false);
  const [animationPhase, setAnimationPhase] = useState<'fade-in' | 'visible' | 'fade-out'>('fade-in');
  const [wasRefreshing, setWasRefreshing] = useState(false);
  const [recentDirectories, setRecentDirectories] = useState<string[]>([]);
  
  // Poll for cache refresh status
  const { data: progress } = useQuery({
    queryKey: ['cache-progress'],
    queryFn: fetchCacheProgress,
    refetchInterval: 1000, // Poll every second
    refetchIntervalInBackground: true,
  });

  // Track recent directories for smart truncation
  useEffect(() => {
    if (progress?.current_directory_name && progress?.is_refreshing) {
      setRecentDirectories(prev => {
        const newDirs = [...prev];
        // Add new directory if it's different from the last one
        if (newDirs.length === 0 || newDirs[newDirs.length - 1] !== progress.current_directory_name) {
          newDirs.push(progress.current_directory_name!);
          // Keep only last 15 directories for analysis
          if (newDirs.length > 15) {
            newDirs.shift();
          }
        }
        return newDirs;
      });
    }
  }, [progress?.current_directory_name, progress?.is_refreshing]);

  // Detect refresh completion and invalidate caches
  useEffect(() => {
    if (wasRefreshing && !progress?.is_refreshing) {
      // Refresh just completed - invalidate all relevant caches
      console.log('🔄 Cache refresh completed, invalidating queries...');
      
      // Invalidate all queries that depend on file existence data
      queryClient.invalidateQueries({ queryKey: ['projects'] });
      queryClient.invalidateQueries({ queryKey: ['targets'] });
      queryClient.invalidateQueries({ queryKey: ['all-images'] });
      queryClient.invalidateQueries({ queryKey: ['projects-overview'] });
      queryClient.invalidateQueries({ queryKey: ['targets-overview'] });
      queryClient.invalidateQueries({ queryKey: ['overall-stats'] });
      
      // Also invalidate any image queries
      queryClient.invalidateQueries({ queryKey: ['images'] });
      
      setWasRefreshing(false);
      // Clear directory history when refresh completes
      setRecentDirectories([]);
    } else if (progress?.is_refreshing) {
      setWasRefreshing(true);
    }
  }, [progress?.is_refreshing, wasRefreshing, queryClient]);

  // Show/hide logic based on refresh state
  useEffect(() => {
    if (progress?.is_refreshing && !isVisible) {
      // Start showing
      setIsVisible(true);
      setAnimationPhase('fade-in');
      setTimeout(() => setAnimationPhase('visible'), 300);
    } else if (!progress?.is_refreshing && isVisible) {
      // Start hiding after a brief delay to show completion
      setTimeout(() => {
        setAnimationPhase('fade-out');
        setTimeout(() => setIsVisible(false), 300);
      }, progress?.stage === 'completed' ? 2000 : 500);
    }
  }, [progress?.is_refreshing, isVisible, progress?.stage]);

  if (!isVisible || !progress) {
    return null;
  }

  const formatElapsedTime = (seconds: number | null): string => {
    if (!seconds) return '';
    if (seconds < 60) return `${Math.round(seconds)}s`;
    const minutes = Math.floor(seconds / 60);
    const remainingSeconds = Math.round(seconds % 60);
    return `${minutes}m ${remainingSeconds}s`;
  };

  const smartTruncatePath = (path: string, recentPaths: string[], maxLength: number = 35): string => {
    if (path.length <= maxLength) {
      return path;
    }
    
    // Handle both Unix and Windows paths
    const separator = path.includes('\\') ? '\\' : '/';
    const parts = path.split(separator).filter(Boolean);
    
    if (parts.length <= 2) {
      return path.length > maxLength ? path.substring(0, maxLength - 3) + '...' : path;
    }
    
    // Find the most distinctive part based on recent directories
    const mostDistinctivePart = findMostDistinctivePart(path, recentPaths);
    
    if (mostDistinctivePart) {
      // Try to show the distinctive part in context
      const distinctiveIndex = parts.findIndex(part => part === mostDistinctivePart);
      
      if (distinctiveIndex !== -1) {
        const first = parts[0];
        const last = parts[parts.length - 1];
        const distinctive = parts[distinctiveIndex];
        
        // If distinctive part is first or last, use simple truncation
        if (distinctiveIndex === 0 || distinctiveIndex === parts.length - 1) {
          return simpleMiddleTruncate(path);
        }
        
        // Try to show: first/.../distinctive/.../last
        const ellipsis = '...';
        const template = `${first}${separator}${ellipsis}${separator}${distinctive}${separator}${ellipsis}${separator}${last}`;
        
        if (template.length <= maxLength) {
          return template;
        }
        
        // Try: .../distinctive/.../last
        const shorterTemplate = `${ellipsis}${separator}${distinctive}${separator}${ellipsis}${separator}${last}`;
        if (shorterTemplate.length <= maxLength) {
          return shorterTemplate;
        }
        
        // Fall back to showing just the distinctive part with context
        const contextTemplate = `${ellipsis}${separator}${distinctive}${separator}${ellipsis}`;
        if (contextTemplate.length <= maxLength) {
          return contextTemplate;
        }
      }
    }
    
    // Fall back to simple middle truncation
    return simpleMiddleTruncate(path);
  };

  const simpleMiddleTruncate = (path: string): string => {
    const separator = path.includes('\\') ? '\\' : '/';
    const parts = path.split(separator).filter(Boolean);
    const first = parts[0];
    const last = parts[parts.length - 1];
    const ellipsis = '...';
    
    return `${first}${separator}${ellipsis}${separator}${last}`;
  };

  const findMostDistinctivePart = (currentPath: string, recentPaths: string[]): string | null => {
    if (recentPaths.length < 2) return null;
    
    const separator = currentPath.includes('\\') ? '\\' : '/';
    const currentParts = currentPath.split(separator).filter(Boolean);
    
    // Analyze parts frequency in recent paths
    const partFrequency = new Map<string, number>();
    const allRecentParts = new Set<string>();
    
    recentPaths.forEach(path => {
      const parts = path.split(separator).filter(Boolean);
      parts.forEach(part => allRecentParts.add(part));
    });
    
    currentParts.forEach(part => {
      let count = 0;
      recentPaths.forEach(path => {
        if (path.includes(part)) count++;
      });
      partFrequency.set(part, count);
    });
    
    // Find part that appears least frequently (most distinctive)
    let minFreq = Infinity;
    let mostDistinctive: string | null = null;
    
    currentParts.forEach(part => {
      const freq = partFrequency.get(part) || 0;
      // Prefer longer, more specific parts when frequency is equal
      if (freq < minFreq || (freq === minFreq && part.length > (mostDistinctive?.length || 0))) {
        minFreq = freq;
        mostDistinctive = part;
      }
    });
    
    return mostDistinctive;
  };

  const getProgressDetails = (): string => {
    if (progress.stage === 'processing_projects' && progress.projects_total > 0) {
      return `${progress.projects_processed}/${progress.projects_total} projects`;
    }
    if (progress.stage === 'processing_targets' && progress.targets_total > 0) {
      return `${progress.targets_processed}/${progress.targets_total} targets`;
    }
    if (progress.current_project_name) {
      return progress.current_project_name;
    }
    return '';
  };

  const getDirectoryProgress = (): { display: string; full: string; counts: string } | null => {
    if (progress.stage === 'initializing_directory_tree' && progress.current_directory_name) {
      const fullPath = progress.current_directory_name;
      const truncatedPath = smartTruncatePath(fullPath, recentDirectories, 45);
      
      // Combine directory counts into compact format
      const parts = [];
      if (progress.directories_processed > 0) parts.push(`${progress.directories_processed}d`);
      if (progress.files_scanned > 0) parts.push(`${progress.files_scanned}f`);
      const counts = parts.join('/');
      
      return { 
        display: truncatedPath, 
        full: fullPath,
        counts 
      };
    }
    return null;
  };

  const getFileStats = (): string => {
    if (progress.files_found > 0 || progress.files_missing > 0) {
      return ` (${progress.files_found} found, ${progress.files_missing} missing)`;
    }
    return '';
  };

  return (
    <div className={`cache-refresh-status ${animationPhase} ${className}`}>
      <div className="cache-status-content">
        {progress.stage !== 'completed' && (
          <div className="loading-spinner" />
        )}
        
        <div className="cache-status-main">
          <div className="cache-status-label">
            {STAGE_LABELS[progress.stage] || progress.stage}
            {progress.stage === 'completed' && ' ✓'}
          </div>
          
          {progress.progress_percentage > 0 && (
            <div className="cache-progress-bar">
              <div 
                className="cache-progress-fill"
                style={{ width: `${Math.min(progress.progress_percentage, 100)}%` }}
              />
              <span className="cache-progress-text">
                {Math.round(progress.progress_percentage)}%
              </span>
            </div>
          )}
        </div>
        
        <div className="cache-status-details">
          {(() => {
            const dirProgress = getDirectoryProgress();
            if (dirProgress) {
              return (
                <>
                  <div className="directory-progress-row">
                    <div 
                      className="current-directory"
                      title={dirProgress.full}
                    >
                      {dirProgress.display}
                    </div>
                    {dirProgress.counts && (
                      <span className="directory-counts">
                        {dirProgress.counts}
                      </span>
                    )}
                  </div>
                  <div className="progress-stats-row">
                    {getFileStats()}
                    {progress.elapsed_seconds && (
                      <span className="cache-elapsed-time">
                        ({formatElapsedTime(progress.elapsed_seconds)})
                      </span>
                    )}
                  </div>
                </>
              );
            } else {
              // Non-directory stages
              return (
                <div className="progress-info-row">
                  {getProgressDetails()}
                  {getFileStats()}
                  {progress.elapsed_seconds && (
                    <span className="cache-elapsed-time">
                      ({formatElapsedTime(progress.elapsed_seconds)})
                    </span>
                  )}
                </div>
              );
            }
          })()}
        </div>
      </div>
    </div>
  );
}