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
  
  // Poll for cache refresh status
  const { data: progress } = useQuery({
    queryKey: ['cache-progress'],
    queryFn: fetchCacheProgress,
    refetchInterval: 1000, // Poll every second
    refetchIntervalInBackground: true,
  });

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

  const getProgressDetails = (): string => {
    if (progress.stage === 'initializing_directory_tree' && progress.directories_total > 0) {
      const dirName = progress.current_directory_name ? 
        ` (${progress.current_directory_name.split('/').pop()})` : '';
      return `${progress.directories_processed}/${progress.directories_total} directories${dirName}`;
    }
    if (progress.stage === 'processing_projects' && progress.projects_total > 0) {
      return `${progress.projects_processed}/${progress.projects_total} projects`;
    }
    if (progress.stage === 'processing_targets' && progress.targets_total > 0) {
      return `${progress.targets_processed}/${progress.targets_total} targets`;
    }
    if (progress.current_project_name) {
      return progress.current_project_name;
    }
    if (progress.current_directory_name && progress.stage === 'initializing_directory_tree') {
      return progress.current_directory_name.split('/').pop() || '';
    }
    return '';
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
          {getProgressDetails()}
          {getFileStats()}
          {progress.elapsed_seconds && (
            <span className="cache-elapsed-time">
              ({formatElapsedTime(progress.elapsed_seconds)})
            </span>
          )}
        </div>
      </div>
    </div>
  );
}